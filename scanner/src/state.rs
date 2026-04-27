use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result, bail};
use ethers::{
    abi::{Abi, AbiParser},
    contract::Contract,
    providers::{Http, Provider},
    types::{Address, U256},
};

use crate::config::{PoolDef, ScannerConfig, TokenDef};

pub type RpcProvider = Provider<Http>;

#[derive(Clone, Debug)]
pub struct SwapStateUpdate {
    pub pool: Address,
    pub block_number: u64,
    pub log_index: u64,
    pub sqrt_price_x96: U256,
    pub liquidity: u128,
    pub tick: i32,
}

#[derive(Clone, Debug)]
pub struct PoolState {
    pub pool: PoolDef,
    pub sqrt_price_x96: U256,
    pub tick: i32,
    pub tick_spacing: i32,
    pub nearest_initialized_lower_tick: Option<i32>,
    pub nearest_initialized_upper_tick: Option<i32>,
    pub liquidity: u128,
    pub last_updated_block: u64,
    pub last_updated_log_index: u64,
}

#[derive(Debug)]
pub struct ScannerState {
    pub pools: HashMap<Address, PoolState>,
}

impl ScannerState {
    pub async fn bootstrap(
        provider: Arc<RpcProvider>,
        config: &ScannerConfig,
        at_block: u64,
    ) -> Result<Self> {
        let mut pools = HashMap::new();

        for pool in &config.pools {
            let state = load_pool_state(provider.clone(), pool, at_block).await?;
            pools.insert(pool.address, state);
        }

        Ok(Self { pools })
    }

    pub fn apply_swap_update(&mut self, pool: &PoolDef, update: &SwapStateUpdate) -> bool {
        let should_apply = self
            .pools
            .get(&update.pool)
            .map(|state| {
                update.block_number > state.last_updated_block
                    || (update.block_number == state.last_updated_block
                        && update.log_index > state.last_updated_log_index)
            })
            .unwrap_or(true);

        if !should_apply {
            return false;
        }

        let next_pool = self
            .pools
            .get(&update.pool)
            .map(|state| state.pool.clone())
            .unwrap_or_else(|| pool.clone());

        self.pools.insert(
            update.pool,
            PoolState {
                pool: next_pool,
                sqrt_price_x96: update.sqrt_price_x96,
                tick: update.tick,
                tick_spacing: self
                    .pools
                    .get(&update.pool)
                    .map(|state| state.tick_spacing)
                    .unwrap_or(1),
                nearest_initialized_lower_tick: self
                    .pools
                    .get(&update.pool)
                    .and_then(|state| state.nearest_initialized_lower_tick),
                nearest_initialized_upper_tick: self
                    .pools
                    .get(&update.pool)
                    .and_then(|state| state.nearest_initialized_upper_tick),
                liquidity: update.liquidity,
                last_updated_block: update.block_number,
                last_updated_log_index: update.log_index,
            },
        );

        true
    }
}

impl PoolState {
    pub fn spot_rate(
        &self,
        token_in: Address,
        token_out: Address,
        token_map: &HashMap<Address, TokenDef>,
    ) -> Option<f64> {
        let token0 = token_map.get(&self.pool.token0)?;
        let token1 = token_map.get(&self.pool.token1)?;
        let sqrt_price = self.sqrt_price_x96.to_string().parse::<f64>().ok()?;
        let raw_price_1_per_0 = (sqrt_price * sqrt_price) / 2f64.powi(192);
        let decimal_adjustment = 10f64.powi(token0.decimals as i32 - token1.decimals as i32);
        let price_1_per_0 = raw_price_1_per_0 * decimal_adjustment;

        if token_in == self.pool.token0 && token_out == self.pool.token1 {
            Some(price_1_per_0)
        } else if token_in == self.pool.token1 && token_out == self.pool.token0 {
            Some(1.0 / price_1_per_0)
        } else {
            None
        }
    }
}

async fn load_pool_state(
    provider: Arc<RpcProvider>,
    pool: &PoolDef,
    at_block: u64,
) -> Result<PoolState> {
    let contract = pool_contract(provider.clone(), pool.address)?;

    let token0: Address = contract
        .method::<_, Address>("token0", ())?
        .call()
        .await
        .with_context(|| format!("token0() failed for {}", pool.name))?;
    let token1: Address = contract
        .method::<_, Address>("token1", ())?
        .call()
        .await
        .with_context(|| format!("token1() failed for {}", pool.name))?;
    let fee: u32 = contract
        .method::<_, u32>("fee", ())?
        .call()
        .await
        .with_context(|| format!("fee() failed for {}", pool.name))?;
    let tick_spacing: i32 = contract
        .method::<_, i32>("tickSpacing", ())?
        .call()
        .await
        .with_context(|| format!("tickSpacing() failed for {}", pool.name))?;
    let config_matches_pair = (token0 == pool.token0 && token1 == pool.token1)
        || (token0 == pool.token1 && token1 == pool.token0);

    if !config_matches_pair || fee != pool.fee {
        bail!(
            "static pool config mismatch for {}: expected pair ({:?}, {:?}) fee {}, got pair ({:?}, {:?}) fee {}",
            pool.name,
            pool.token0,
            pool.token1,
            pool.fee,
            token0,
            token1,
            fee
        );
    }

    let mut normalized_pool = pool.clone();
    normalized_pool.token0 = token0;
    normalized_pool.token1 = token1;
    normalized_pool.fee = fee;

    load_dynamic_pool_state(contract, normalized_pool, tick_spacing, at_block).await
}

async fn load_dynamic_pool_state(
    contract: Contract<RpcProvider>,
    pool: PoolDef,
    tick_spacing: i32,
    at_block: u64,
) -> Result<PoolState> {
    let liquidity: u128 = contract
        .method::<_, u128>("liquidity", ())?
        .call()
        .await
        .with_context(|| format!("liquidity() failed for {}", pool.name))?;
    let (sqrt_price_x96, tick, _, _, _, _, _): (U256, i32, u16, u16, u16, u8, bool) = contract
        .method::<_, (U256, i32, u16, u16, u16, u8, bool)>("slot0", ())?
        .call()
        .await
        .with_context(|| format!("slot0() failed for {}", pool.name))?;
    let (nearest_initialized_lower_tick, nearest_initialized_upper_tick) =
        load_initialized_tick_bounds(&contract, tick, tick_spacing)
            .await
            .with_context(|| format!("failed to load initialized tick bounds for {}", pool.name))?;

    Ok(PoolState {
        pool,
        sqrt_price_x96,
        tick,
        tick_spacing,
        nearest_initialized_lower_tick,
        nearest_initialized_upper_tick,
        liquidity,
        last_updated_block: at_block,
        last_updated_log_index: u64::MAX,
    })
}

fn pool_contract(provider: Arc<RpcProvider>, address: Address) -> Result<Contract<RpcProvider>> {
    Ok(Contract::new(address, pool_abi()?, provider))
}

fn pool_abi() -> Result<Abi> {
    AbiParser::default()
        .parse(&[
            "function token0() view returns (address)",
            "function token1() view returns (address)",
            "function fee() view returns (uint24)",
            "function tickSpacing() view returns (int24)",
            "function tickBitmap(int16) view returns (uint256)",
            "function liquidity() view returns (uint128)",
            "function slot0() view returns (uint160 sqrtPriceX96, int24 tick, uint16 observationIndex, uint16 observationCardinality, uint16 observationCardinalityNext, uint8 feeProtocol, bool unlocked)",
        ])
        .context("failed to build Uniswap v3 pool ABI")
}

async fn load_initialized_tick_bounds(
    contract: &Contract<RpcProvider>,
    tick: i32,
    tick_spacing: i32,
) -> Result<(Option<i32>, Option<i32>)> {
    let spacing = tick_spacing.max(1);
    let compressed = tick.div_euclid(spacing);
    let current_word = compressed >> 8;
    let current_bit = compressed.rem_euclid(256);

    let mut lower = None;
    let mut upper = None;

    for distance in 0..=8i32 {
        if lower.is_none() {
            let word = current_word - distance;
            let bitmap: U256 = contract
                .method::<_, U256>("tickBitmap", word as i16)?
                .call()
                .await?;
            lower = find_lower_initialized_tick(
                bitmap,
                word,
                if distance == 0 { current_bit } else { 255 },
                spacing,
            );
        }

        if upper.is_none() {
            let word = current_word + distance;
            let bitmap: U256 = contract
                .method::<_, U256>("tickBitmap", word as i16)?
                .call()
                .await?;
            upper = find_upper_initialized_tick(
                bitmap,
                word,
                if distance == 0 { current_bit + 1 } else { 0 },
                spacing,
            );
        }

        if lower.is_some() && upper.is_some() {
            break;
        }
    }

    Ok((lower, upper))
}

fn find_lower_initialized_tick(
    bitmap: U256,
    word: i32,
    start_bit: i32,
    spacing: i32,
) -> Option<i32> {
    if start_bit < 0 {
        return None;
    }

    for bit in (0..=start_bit as usize).rev() {
        if bitmap.bit(bit) {
            return Some(((word << 8) + bit as i32) * spacing);
        }
    }

    None
}

fn find_upper_initialized_tick(
    bitmap: U256,
    word: i32,
    start_bit: i32,
    spacing: i32,
) -> Option<i32> {
    if start_bit > 255 {
        return None;
    }

    for bit in start_bit as usize..256 {
        if bitmap.bit(bit) {
            return Some(((word << 8) + bit as i32) * spacing);
        }
    }

    None
}
