use std::sync::Arc;

use anyhow::{Context, Result};
use ethers::{
    providers::{Middleware, Provider, Ws},
    types::{Filter, H256, Log, U256, ValueOrArray},
    utils::keccak256,
};
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::{
    config::ScannerConfig,
    state::{LiquidityStateUpdate, PoolStateEvent, RpcProvider, SwapStateUpdate},
};

pub async fn poll_pool_events(
    provider: Arc<RpcProvider>,
    config: &ScannerConfig,
    from_block: u64,
    to_block: u64,
) -> Result<Vec<PoolStateEvent>> {
    if from_block > to_block {
        return Ok(Vec::new());
    }

    let mut updates = Vec::new();
    let mut range_start = from_block;

    while range_start <= to_block {
        let range_end = range_start
            .saturating_add(config.max_log_block_range.saturating_sub(1))
            .min(to_block);

        let filter = Filter::new()
            .address(config.pool_addresses())
            .from_block(range_start)
            .to_block(range_end)
            .topic0(ValueOrArray::Array(vec![
                swap_topic(),
                mint_topic(),
                burn_topic(),
            ]));

        let logs = provider.get_logs(&filter).await.with_context(|| {
            format!("failed to fetch logs for blocks {range_start}..={range_end}")
        })?;

        updates.extend(logs.into_iter().filter_map(decode_pool_event));
        range_start = range_end.saturating_add(1);
    }

    updates.sort_by_key(|update| (update.block_number(), update.log_index()));
    Ok(updates)
}

pub fn spawn_live_pool_watcher(config: ScannerConfig) -> Option<mpsc::Receiver<PoolStateEvent>> {
    let ws_url = config.log_ws_url.clone()?;
    let (sender, receiver) = mpsc::channel(1024);

    tokio::spawn(async move {
        loop {
            match run_live_pool_watcher(&ws_url, &config, sender.clone()).await {
                Ok(()) => warn!("ws log stream ended, reconnecting"),
                Err(error) => warn!(error = %error, "ws watcher failed, reconnecting"),
            }

            tokio::time::sleep(config.ws_reconnect_delay).await;
        }
    });

    Some(receiver)
}

async fn run_live_pool_watcher(
    ws_url: &str,
    config: &ScannerConfig,
    sender: mpsc::Sender<PoolStateEvent>,
) -> Result<()> {
    let ws = Ws::connect(ws_url)
        .await
        .with_context(|| format!("failed to connect log websocket provider at {ws_url}"))?;
    let provider = Provider::new(ws);
    let filter = pool_event_filter(config);
    let mut stream = provider
        .subscribe_logs(&filter)
        .await
        .context("failed to subscribe to pool logs")?;

    info!("ws pool log subscription active");

    while let Some(log) = stream.next().await {
        if let Some(event) = decode_pool_event(log)
            && sender.send(event).await.is_err()
        {
            return Ok(());
        }
    }

    Ok(())
}

fn swap_topic() -> H256 {
    H256::from(keccak256(
        "Swap(address,address,int256,int256,uint160,uint128,int24)",
    ))
}

fn mint_topic() -> H256 {
    H256::from(keccak256(
        "Mint(address,address,int24,int24,uint128,uint256,uint256)",
    ))
}

fn burn_topic() -> H256 {
    H256::from(keccak256(
        "Burn(address,int24,int24,uint128,uint256,uint256)",
    ))
}

fn pool_event_filter(config: &ScannerConfig) -> Filter {
    Filter::new()
        .address(config.pool_addresses())
        .topic0(ValueOrArray::Array(vec![
            swap_topic(),
            mint_topic(),
            burn_topic(),
        ]))
}

fn decode_pool_event(log: Log) -> Option<PoolStateEvent> {
    let topic = *log.topics.first()?;

    if topic == swap_topic() {
        return decode_swap_update(log).map(PoolStateEvent::Swap);
    }

    if topic == mint_topic() {
        return decode_liquidity_update(log, true).map(PoolStateEvent::Liquidity);
    }

    if topic == burn_topic() {
        return decode_liquidity_update(log, false).map(PoolStateEvent::Liquidity);
    }

    None
}

fn decode_swap_update(log: Log) -> Option<SwapStateUpdate> {
    let block_number = log.block_number?.as_u64();
    let log_index = log.log_index?.as_u64();
    let data = log.data.0;

    if data.len() != 32 * 5 {
        return None;
    }

    let sqrt_price_x96 = U256::from_big_endian(&data[64..96]);
    let liquidity_raw = U256::from_big_endian(&data[96..128]);
    if liquidity_raw.bits() > 128 {
        return None;
    }

    Some(SwapStateUpdate {
        pool: log.address,
        block_number,
        log_index,
        sqrt_price_x96,
        liquidity: liquidity_raw.as_u128(),
        tick: decode_i24(&data[128..160])?,
    })
}

fn decode_liquidity_update(log: Log, is_mint: bool) -> Option<LiquidityStateUpdate> {
    let block_number = log.block_number?.as_u64();
    let log_index = log.log_index?.as_u64();
    let tick_lower = decode_i24(log.topics.get(2)?.as_bytes())?;
    let tick_upper = decode_i24(log.topics.get(3)?.as_bytes())?;
    let data = log.data.0;

    let amount_offset = if is_mint { 32 } else { 0 };
    if data.len() < amount_offset + 32 {
        return None;
    }

    let amount = U256::from_big_endian(&data[amount_offset..amount_offset + 32]);
    if amount.bits() > 127 {
        return None;
    }

    let amount = amount.as_u128() as i128;
    let liquidity_delta = if is_mint { amount } else { -amount };

    Some(LiquidityStateUpdate {
        pool: log.address,
        block_number,
        log_index,
        tick_lower,
        tick_upper,
        liquidity_delta,
    })
}

fn decode_i24(word: &[u8]) -> Option<i32> {
    if word.len() != 32 {
        return None;
    }

    let mut bytes = [0u8; 4];
    if word[0] & 0x80 != 0 {
        bytes[0] = 0xff;
    }
    bytes[1..4].copy_from_slice(&word[29..32]);
    Some(i32::from_be_bytes(bytes))
}
