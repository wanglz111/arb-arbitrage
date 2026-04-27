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
    state::{RpcProvider, SwapStateUpdate},
};

pub async fn poll_changed_pools(
    provider: Arc<RpcProvider>,
    config: &ScannerConfig,
    from_block: u64,
    to_block: u64,
) -> Result<Vec<SwapStateUpdate>> {
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
            .topic0(ValueOrArray::Value(swap_topic()));

        let logs = provider.get_logs(&filter).await.with_context(|| {
            format!("failed to fetch logs for blocks {range_start}..={range_end}")
        })?;

        updates.extend(logs.into_iter().filter_map(decode_swap_update));
        range_start = range_end.saturating_add(1);
    }

    updates.sort_by_key(|update| (update.block_number, update.log_index));
    Ok(updates)
}

pub fn spawn_live_swap_watcher(config: ScannerConfig) -> Option<mpsc::Receiver<SwapStateUpdate>> {
    let ws_url = config.log_ws_url.clone()?;
    let (sender, receiver) = mpsc::channel(1024);

    tokio::spawn(async move {
        loop {
            match run_live_swap_watcher(&ws_url, &config, sender.clone()).await {
                Ok(()) => warn!("ws log stream ended, reconnecting"),
                Err(error) => warn!(error = %error, "ws watcher failed, reconnecting"),
            }

            tokio::time::sleep(config.ws_reconnect_delay).await;
        }
    });

    Some(receiver)
}

async fn run_live_swap_watcher(
    ws_url: &str,
    config: &ScannerConfig,
    sender: mpsc::Sender<SwapStateUpdate>,
) -> Result<()> {
    let ws = Ws::connect(ws_url)
        .await
        .with_context(|| format!("failed to connect log websocket provider at {ws_url}"))?;
    let provider = Provider::new(ws);
    let filter = swap_filter(config);
    let mut stream = provider
        .subscribe_logs(&filter)
        .await
        .context("failed to subscribe to swap logs")?;

    info!("ws log subscription active");

    while let Some(log) = stream.next().await {
        if let Some(event) = decode_swap_update(log)
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

fn swap_filter(config: &ScannerConfig) -> Filter {
    Filter::new()
        .address(config.pool_addresses())
        .topic0(ValueOrArray::Value(swap_topic()))
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
