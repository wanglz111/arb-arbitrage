use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    env,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use ethers::{
    abi::{Abi, AbiParser},
    contract::Contract,
    providers::{Http, Provider},
    types::{Address, U256},
};
use futures_util::{StreamExt, stream};
use tokio::time::sleep;
use tracing::info;

use crate::config::{PoolDef, ScannerConfig, TokenDef};

pub type RpcProvider = Provider<Http>;
const DEFAULT_TICK_BITMAP_WORD_SCAN_RADIUS: i32 = 12;

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
pub struct LiquidityStateUpdate {
    pub pool: Address,
    pub block_number: u64,
    pub log_index: u64,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub liquidity_delta: i128,
}

#[derive(Clone, Debug)]
pub enum PoolStateEvent {
    Swap(SwapStateUpdate),
    Liquidity(LiquidityStateUpdate),
}

impl PoolStateEvent {
    pub fn pool(&self) -> Address {
        match self {
            Self::Swap(update) => update.pool,
            Self::Liquidity(update) => update.pool,
        }
    }

    pub fn block_number(&self) -> u64 {
        match self {
            Self::Swap(update) => update.block_number,
            Self::Liquidity(update) => update.block_number,
        }
    }

    pub fn log_index(&self) -> u64 {
        match self {
            Self::Swap(update) => update.log_index,
            Self::Liquidity(update) => update.log_index,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PoolState {
    pub pool: PoolDef,
    pub sqrt_price_x96: U256,
    pub tick: i32,
    pub tick_spacing: i32,
    pub nearest_initialized_lower_tick: Option<i32>,
    pub nearest_initialized_upper_tick: Option<i32>,
    pub initialized_ticks: BTreeMap<i32, i128>,
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

    pub fn apply_pool_event(&mut self, pool: &PoolDef, event: &PoolStateEvent) -> bool {
        if !self.should_apply(event.pool(), event.block_number(), event.log_index()) {
            return false;
        }

        match event {
            PoolStateEvent::Swap(update) => self.apply_swap_update_unchecked(pool, update),
            PoolStateEvent::Liquidity(update) => {
                self.apply_liquidity_update_unchecked(pool, update)
            }
        }

        true
    }

    fn should_apply(&self, pool: Address, block_number: u64, log_index: u64) -> bool {
        self.pools
            .get(&pool)
            .map(|state| {
                block_number > state.last_updated_block
                    || (block_number == state.last_updated_block
                        && log_index > state.last_updated_log_index)
            })
            .unwrap_or(true)
    }

    fn apply_swap_update_unchecked(&mut self, pool: &PoolDef, update: &SwapStateUpdate) {
        let existing = self.pools.get(&update.pool);
        let next_pool = existing
            .map(|state| state.pool.clone())
            .unwrap_or_else(|| pool.clone());
        let tick_spacing = existing.map(|state| state.tick_spacing).unwrap_or(1);
        let initialized_ticks = existing
            .map(|state| state.initialized_ticks.clone())
            .unwrap_or_default();
        let (nearest_initialized_lower_tick, nearest_initialized_upper_tick) =
            initialized_tick_bounds(&initialized_ticks, update.tick);

        self.pools.insert(
            update.pool,
            PoolState {
                pool: next_pool,
                sqrt_price_x96: update.sqrt_price_x96,
                tick: update.tick,
                tick_spacing,
                nearest_initialized_lower_tick,
                nearest_initialized_upper_tick,
                initialized_ticks,
                liquidity: update.liquidity,
                last_updated_block: update.block_number,
                last_updated_log_index: update.log_index,
            },
        );
    }

    fn apply_liquidity_update_unchecked(&mut self, pool: &PoolDef, update: &LiquidityStateUpdate) {
        let mut state = self
            .pools
            .get(&update.pool)
            .cloned()
            .unwrap_or_else(|| PoolState {
                pool: pool.clone(),
                sqrt_price_x96: U256::zero(),
                tick: 0,
                tick_spacing: 1,
                nearest_initialized_lower_tick: None,
                nearest_initialized_upper_tick: None,
                initialized_ticks: BTreeMap::new(),
                liquidity: 0,
                last_updated_block: 0,
                last_updated_log_index: 0,
            });

        apply_liquidity_net_delta(
            &mut state.initialized_ticks,
            update.tick_lower,
            update.liquidity_delta,
        );
        apply_liquidity_net_delta(
            &mut state.initialized_ticks,
            update.tick_upper,
            -update.liquidity_delta,
        );

        if update.tick_lower <= state.tick && state.tick < update.tick_upper {
            state.liquidity = apply_active_liquidity_delta(state.liquidity, update.liquidity_delta);
        }

        let (lower, upper) = initialized_tick_bounds(&state.initialized_ticks, state.tick);
        state.nearest_initialized_lower_tick = lower;
        state.nearest_initialized_upper_tick = upper;
        state.last_updated_block = update.block_number;
        state.last_updated_log_index = update.log_index;

        self.pools.insert(update.pool, state);
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
    info!(pool = pool.name, address = ?pool.address, "loading pool dynamic state");
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
    info!(
        pool = pool.name,
        tick,
        liquidity = liquidity.to_string(),
        "loading initialized ticks"
    );
    let initialized_ticks = load_initialized_ticks(&contract, tick, tick_spacing)
        .await
        .with_context(|| format!("failed to load initialized ticks for {}", pool.name))?;
    let (nearest_initialized_lower_tick, nearest_initialized_upper_tick) =
        initialized_tick_bounds(&initialized_ticks, tick);
    info!(
        pool = pool.name,
        initialized_ticks = initialized_ticks.len(),
        nearest_initialized_lower_tick,
        nearest_initialized_upper_tick,
        "loaded pool state"
    );

    Ok(PoolState {
        pool,
        sqrt_price_x96,
        tick,
        tick_spacing,
        nearest_initialized_lower_tick,
        nearest_initialized_upper_tick,
        initialized_ticks,
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
            "function ticks(int24) view returns (uint128 liquidityGross, int128 liquidityNet, uint256 feeGrowthOutside0X128, uint256 feeGrowthOutside1X128, int56 tickCumulativeOutside, uint160 secondsPerLiquidityOutsideX128, uint32 secondsOutside, bool initialized)",
            "function liquidity() view returns (uint128)",
            "function slot0() view returns (uint160 sqrtPriceX96, int24 tick, uint16 observationIndex, uint16 observationCardinality, uint16 observationCardinalityNext, uint8 feeProtocol, bool unlocked)",
        ])
        .context("failed to build Uniswap v3 pool ABI")
}

async fn load_initialized_ticks(
    contract: &Contract<RpcProvider>,
    tick: i32,
    tick_spacing: i32,
) -> Result<BTreeMap<i32, i128>> {
    let spacing = tick_spacing.max(1);
    let compressed = tick.div_euclid(spacing);
    let current_word = compressed >> 8;
    let scan_radius = tick_bitmap_word_scan_radius();
    let mut words = BTreeSet::new();

    for distance in 0..=scan_radius {
        words.insert(current_word - distance);
        words.insert(current_word + distance);
    }

    let mut initialized_ticks = BTreeMap::new();
    info!(
        tick,
        tick_spacing = spacing,
        current_word,
        scan_radius,
        bitmap_words = words.len(),
        "scanning tick bitmap words"
    );
    let mut initialized_tick_indexes = Vec::new();

    for word in words {
        let bitmap: U256 = contract
            .method::<_, U256>("tickBitmap", word as i16)?
            .call()
            .await?;

        for bit in 0..256usize {
            if !bitmap.bit(bit) {
                continue;
            }

            let tick = ((word << 8) + bit as i32) * spacing;
            initialized_tick_indexes.push(tick);
        }
    }

    let tick_load_concurrency = tick_load_concurrency();
    info!(
        initialized_tick_indexes = initialized_tick_indexes.len(),
        tick_load_concurrency, "loading initialized tick details"
    );

    let loaded_ticks = stream::iter(initialized_tick_indexes)
        .map(|tick| {
            let contract = contract.clone();
            async move {
                load_tick_detail_with_retry(contract, tick)
                    .await
                    .map(|(liquidity_net, initialized)| (tick, liquidity_net, initialized))
            }
        })
        .buffer_unordered(tick_load_concurrency)
        .collect::<Vec<_>>()
        .await;

    for loaded in loaded_ticks {
        let (tick, liquidity_net, initialized) = loaded?;
        if initialized && liquidity_net != 0 {
            initialized_ticks.insert(tick, liquidity_net);
        }
    }

    Ok(initialized_ticks)
}

async fn load_tick_detail_with_retry(
    contract: Contract<RpcProvider>,
    tick: i32,
) -> Result<(i128, bool)> {
    let max_attempts = tick_load_retry_attempts();
    let retry_delay = tick_load_retry_delay();

    for attempt in 1..=max_attempts {
        let result = async {
            let (_, liquidity_net, _, _, _, _, _, initialized): (
                u128,
                i128,
                U256,
                U256,
                i64,
                U256,
                u32,
                bool,
            ) = contract.method::<_, _>("ticks", tick)?.call().await?;
            Ok::<_, anyhow::Error>((liquidity_net, initialized))
        }
        .await;

        match result {
            Ok(detail) => return Ok(detail),
            Err(error) if attempt < max_attempts && is_retryable_rpc_error(&error) => {
                sleep(retry_delay.saturating_mul(attempt)).await;
            }
            Err(error) => return Err(error),
        }
    }

    unreachable!("tick retry loop always returns")
}

fn is_retryable_rpc_error(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    message.contains("429")
        || message.contains("Too Many Requests")
        || message.contains("exceeded")
        || message.contains("rate limit")
        || message.contains("EOF while parsing")
        || message.contains("Deserialization Error")
        || message.contains("connection")
        || message.contains("timeout")
}

fn tick_bitmap_word_scan_radius() -> i32 {
    env::var("SCANNER_TICK_BITMAP_WORD_SCAN_RADIUS")
        .ok()
        .and_then(|value| value.trim().parse::<i32>().ok())
        .filter(|value| *value >= 0)
        .unwrap_or(DEFAULT_TICK_BITMAP_WORD_SCAN_RADIUS)
}

fn tick_load_concurrency() -> usize {
    env::var("SCANNER_TICK_LOAD_CONCURRENCY")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(4)
}

fn tick_load_retry_attempts() -> u32 {
    env::var("SCANNER_TICK_LOAD_RETRY_ATTEMPTS")
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(5)
}

fn tick_load_retry_delay() -> Duration {
    let millis = env::var("SCANNER_TICK_LOAD_RETRY_MS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(500);
    Duration::from_millis(millis)
}

fn initialized_tick_bounds(
    initialized_ticks: &BTreeMap<i32, i128>,
    tick: i32,
) -> (Option<i32>, Option<i32>) {
    let lower = initialized_ticks
        .range(..=tick)
        .next_back()
        .map(|(tick, _)| *tick);
    let upper = initialized_ticks
        .range((tick + 1)..)
        .next()
        .map(|(tick, _)| *tick);

    (lower, upper)
}

fn apply_liquidity_net_delta(ticks: &mut BTreeMap<i32, i128>, tick: i32, delta: i128) {
    let next = ticks.get(&tick).copied().unwrap_or_default() + delta;
    if next == 0 {
        ticks.remove(&tick);
    } else {
        ticks.insert(tick, next);
    }
}

fn apply_active_liquidity_delta(liquidity: u128, delta: i128) -> u128 {
    if delta >= 0 {
        liquidity.saturating_add(delta as u128)
    } else {
        liquidity.saturating_sub((-delta) as u128)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(raw: &str) -> Address {
        raw.parse().expect("invalid test address")
    }

    fn pool_def(pool: Address, token0: Address, token1: Address) -> PoolDef {
        PoolDef {
            name: "test",
            address: pool,
            token0,
            token1,
            fee: 500,
            reserve_usd_hint: 1_000_000.0,
        }
    }

    fn pool_state(pool: PoolDef) -> PoolState {
        PoolState {
            pool,
            sqrt_price_x96: U256::from_dec_str("79228162514264337593543950336").expect("q96"),
            tick: 0,
            tick_spacing: 10,
            nearest_initialized_lower_tick: Some(-10),
            nearest_initialized_upper_tick: Some(10),
            initialized_ticks: BTreeMap::from([(-10, 100), (10, -100)]),
            liquidity: 100,
            last_updated_block: 0,
            last_updated_log_index: 0,
        }
    }

    #[test]
    fn mint_and_burn_update_initialized_ticks_and_active_liquidity() {
        let pool_address = addr("0x1000000000000000000000000000000000000001");
        let pool = pool_def(
            pool_address,
            addr("0x2000000000000000000000000000000000000001"),
            addr("0x2000000000000000000000000000000000000002"),
        );
        let mut state = ScannerState {
            pools: HashMap::from([(pool_address, pool_state(pool.clone()))]),
        };

        let mint = PoolStateEvent::Liquidity(LiquidityStateUpdate {
            pool: pool_address,
            block_number: 1,
            log_index: 0,
            tick_lower: -5,
            tick_upper: 5,
            liquidity_delta: 25,
        });

        assert!(state.apply_pool_event(&pool, &mint));
        let updated = state.pools.get(&pool_address).expect("pool");
        assert_eq!(updated.liquidity, 125);
        assert_eq!(updated.initialized_ticks.get(&-5), Some(&25));
        assert_eq!(updated.initialized_ticks.get(&5), Some(&-25));
        assert_eq!(updated.nearest_initialized_lower_tick, Some(-5));
        assert_eq!(updated.nearest_initialized_upper_tick, Some(5));

        let burn = PoolStateEvent::Liquidity(LiquidityStateUpdate {
            pool: pool_address,
            block_number: 1,
            log_index: 1,
            tick_lower: -5,
            tick_upper: 5,
            liquidity_delta: -10,
        });

        assert!(state.apply_pool_event(&pool, &burn));
        let updated = state.pools.get(&pool_address).expect("pool");
        assert_eq!(updated.liquidity, 115);
        assert_eq!(updated.initialized_ticks.get(&-5), Some(&15));
        assert_eq!(updated.initialized_ticks.get(&5), Some(&-15));

        assert!(!state.apply_pool_event(&pool, &mint));
    }
}
