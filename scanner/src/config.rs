use std::{collections::HashMap, env, time::Duration};

use anyhow::{Context, Result};
use ethers::types::Address;

#[derive(Clone, Debug)]
pub struct TokenDef {
    pub symbol: &'static str,
    pub address: Address,
    pub decimals: u8,
    pub quote_amount_in: u128,
}

#[derive(Clone, Debug)]
pub struct PoolDef {
    pub name: &'static str,
    pub address: Address,
    pub token0: Address,
    pub token1: Address,
    pub fee: u32,
    pub reserve_usd_hint: f64,
}

#[derive(Clone, Debug)]
pub struct ScannerConfig {
    pub rpc_url: String,
    pub log_rpc_url: String,
    pub log_ws_url: Option<String>,
    pub quote_rpc_url: String,
    pub poll_interval: Duration,
    pub max_log_block_range: u64,
    pub ws_reconnect_delay: Duration,
    pub rpc_retry_delay: Duration,
    pub min_candidate_edge_bps: f64,
    pub min_local_edge_bps: f64,
    pub rough_shortlist_size: usize,
    pub local_size_bps: Vec<u32>,
    pub exact_quote_enabled: bool,
    pub max_exact_quotes_per_block: usize,
    pub execution_slippage_bps: u32,
    pub log_execution_calldata: bool,
    pub debug_summary_enabled: bool,
    pub debug_summary_interval: Duration,
    pub start_from_latest: bool,
    pub tokens: Vec<TokenDef>,
    pub pools: Vec<PoolDef>,
}

impl ScannerConfig {
    pub fn from_env() -> Result<Self> {
        let rpc_url =
            env::var("HTTP_RPC_URL").unwrap_or_else(|_| "https://arb1.arbitrum.io/rpc".to_string());
        let log_rpc_url = env::var("LOG_RPC_URL").unwrap_or_else(|_| rpc_url.clone());
        let log_ws_url = env::var("LOG_WS_URL")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let quote_rpc_url = env::var("QUOTE_RPC_URL").unwrap_or_else(|_| rpc_url.clone());
        let poll_interval_ms = env::var("SCANNER_POLL_MS")
            .unwrap_or_else(|_| "1500".to_string())
            .parse::<u64>()
            .context("failed to parse SCANNER_POLL_MS")?;
        let max_log_block_range = env::var("SCANNER_MAX_LOG_BLOCK_RANGE")
            .unwrap_or_else(|_| "10".to_string())
            .parse::<u64>()
            .context("failed to parse SCANNER_MAX_LOG_BLOCK_RANGE")?;
        let ws_reconnect_delay_ms = env::var("SCANNER_WS_RECONNECT_MS")
            .unwrap_or_else(|_| "2000".to_string())
            .parse::<u64>()
            .context("failed to parse SCANNER_WS_RECONNECT_MS")?;
        let rpc_retry_delay_ms = env::var("SCANNER_RPC_RETRY_MS")
            .unwrap_or_else(|_| "2000".to_string())
            .parse::<u64>()
            .context("failed to parse SCANNER_RPC_RETRY_MS")?;
        let min_candidate_edge_bps = env::var("SCANNER_MIN_EDGE_BPS")
            .unwrap_or_else(|_| "0".to_string())
            .parse::<f64>()
            .context("failed to parse SCANNER_MIN_EDGE_BPS")?;
        let min_local_edge_bps = env::var("SCANNER_MIN_LOCAL_EDGE_BPS")
            .unwrap_or_else(|_| "0".to_string())
            .parse::<f64>()
            .context("failed to parse SCANNER_MIN_LOCAL_EDGE_BPS")?;
        let rough_shortlist_size = env::var("SCANNER_ROUGH_SHORTLIST_SIZE")
            .unwrap_or_else(|_| "5".to_string())
            .parse::<usize>()
            .context("failed to parse SCANNER_ROUGH_SHORTLIST_SIZE")?;
        let local_size_bps = env::var("SCANNER_LOCAL_SIZE_BPS")
            .unwrap_or_else(|_| "2500,5000,10000,20000,40000".to_string());
        let local_size_bps = parse_size_bps_list(&local_size_bps)?;
        let exact_quote_enabled = env::var("SCANNER_EXACT_QUOTE_ENABLED")
            .ok()
            .map(|raw| matches!(raw.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
            .unwrap_or(false);
        let max_exact_quotes_per_block = env::var("SCANNER_MAX_EXACT_QUOTES_PER_BLOCK")
            .unwrap_or_else(|_| "2".to_string())
            .parse::<usize>()
            .context("failed to parse SCANNER_MAX_EXACT_QUOTES_PER_BLOCK")?;
        let execution_slippage_bps = env::var("SCANNER_EXECUTION_SLIPPAGE_BPS")
            .unwrap_or_else(|_| "25".to_string())
            .parse::<u32>()
            .context("failed to parse SCANNER_EXECUTION_SLIPPAGE_BPS")?;
        let log_execution_calldata = env::var("SCANNER_LOG_EXECUTION_CALLDATA")
            .ok()
            .map(|raw| matches!(raw.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
            .unwrap_or(false);
        let debug_summary_enabled = env::var("SCANNER_DEBUG_SUMMARY_ENABLED")
            .ok()
            .map(|raw| matches!(raw.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
            .unwrap_or(false);
        let debug_summary_interval_secs = env::var("SCANNER_DEBUG_SUMMARY_INTERVAL_SECS")
            .unwrap_or_else(|_| "300".to_string())
            .parse::<u64>()
            .context("failed to parse SCANNER_DEBUG_SUMMARY_INTERVAL_SECS")?;
        let start_from_latest = env::var("SCANNER_START_FROM_LATEST")
            .ok()
            .map(|raw| matches!(raw.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
            .unwrap_or(true);

        Ok(Self {
            rpc_url,
            log_rpc_url,
            log_ws_url,
            quote_rpc_url,
            poll_interval: Duration::from_millis(poll_interval_ms),
            max_log_block_range,
            ws_reconnect_delay: Duration::from_millis(ws_reconnect_delay_ms),
            rpc_retry_delay: Duration::from_millis(rpc_retry_delay_ms),
            min_candidate_edge_bps,
            min_local_edge_bps,
            rough_shortlist_size,
            local_size_bps,
            exact_quote_enabled,
            max_exact_quotes_per_block,
            execution_slippage_bps,
            log_execution_calldata,
            debug_summary_enabled,
            debug_summary_interval: Duration::from_secs(debug_summary_interval_secs.max(1)),
            start_from_latest,
            tokens: core_tokens(),
            pools: core_pools(),
        })
    }

    pub fn pool_addresses(&self) -> Vec<Address> {
        self.pools.iter().map(|pool| pool.address).collect()
    }

    pub fn token_map(&self) -> HashMap<Address, TokenDef> {
        self.tokens
            .iter()
            .cloned()
            .map(|token| (token.address, token))
            .collect()
    }
}

fn core_tokens() -> Vec<TokenDef> {
    vec![
        TokenDef {
            symbol: "USDC",
            address: addr("0xaf88d065e77c8cC2239327C5EDb3A432268e5831"),
            decimals: 6,
            quote_amount_in: 1_000_000_000,
        },
        TokenDef {
            symbol: "USDT0",
            address: addr("0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9"),
            decimals: 6,
            quote_amount_in: 1_000_000_000,
        },
        TokenDef {
            symbol: "WETH",
            address: addr("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1"),
            decimals: 18,
            quote_amount_in: 500_000_000_000_000_000,
        },
        TokenDef {
            symbol: "WBTC",
            address: addr("0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f"),
            decimals: 8,
            quote_amount_in: 2_000_000,
        },
        TokenDef {
            symbol: "ARB",
            address: addr("0x912CE59144191C1204E64559FE8253a0e49E6548"),
            decimals: 18,
            quote_amount_in: 1_000_000_000_000_000_000_000,
        },
    ]
}

fn core_pools() -> Vec<PoolDef> {
    vec![
        PoolDef {
            name: "USDC/WETH 0.05%",
            address: addr("0xc6962004f452be9203591991d15f6b388e09e8d0"),
            token0: addr("0xaf88d065e77c8cC2239327C5EDb3A432268e5831"),
            token1: addr("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1"),
            fee: 500,
            reserve_usd_hint: 56_476_113.408,
        },
        PoolDef {
            name: "WBTC/WETH 0.05%",
            address: addr("0x2f5e87c9312fa29aed5c179e456625d79015299c"),
            token0: addr("0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f"),
            token1: addr("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1"),
            fee: 500,
            reserve_usd_hint: 49_365_684.631_4,
        },
        PoolDef {
            name: "WBTC/USDT0 0.05%",
            address: addr("0x5969efdde3cf5c0d9a88ae51e47d721096a97203"),
            token0: addr("0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f"),
            token1: addr("0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9"),
            fee: 500,
            reserve_usd_hint: 13_039_186.268_2,
        },
        PoolDef {
            name: "WETH/USDT0 0.05%",
            address: addr("0x641c00a822e8b671738d32a431a4fb6074e5c79d"),
            token0: addr("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1"),
            token1: addr("0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9"),
            fee: 500,
            reserve_usd_hint: 12_571_140.261_9,
        },
        PoolDef {
            name: "USDC/WBTC 0.05%",
            address: addr("0x0e4831319a50228b9e450861297ab92dee15b44f"),
            token0: addr("0xaf88d065e77c8cC2239327C5EDb3A432268e5831"),
            token1: addr("0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f"),
            fee: 500,
            reserve_usd_hint: 8_730_801.690_6,
        },
        PoolDef {
            name: "USDC/WETH 0.3%",
            address: addr("0xc473e2aee3441bf9240be85eb122abb059a3b57c"),
            token0: addr("0xaf88d065e77c8cC2239327C5EDb3A432268e5831"),
            token1: addr("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1"),
            fee: 3000,
            reserve_usd_hint: 7_958_999.565_8,
        },
        PoolDef {
            name: "USDC/USDT0 0.01%",
            address: addr("0xbe3ad6a5669dc0b8b12febc03608860c31e2eef6"),
            token0: addr("0xaf88d065e77c8cC2239327C5EDb3A432268e5831"),
            token1: addr("0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9"),
            fee: 100,
            reserve_usd_hint: 2_561_040.728_1,
        },
        PoolDef {
            name: "ARB/WETH 0.05%",
            address: addr("0xc6f780497a95e246eb9449f5e4770916dcd6396a"),
            token0: addr("0x912CE59144191C1204E64559FE8253a0e49E6548"),
            token1: addr("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1"),
            fee: 500,
            reserve_usd_hint: 1_835_160.894_7,
        },
        PoolDef {
            name: "ARB/USDC 0.3%",
            address: addr("0xaebdca1bc8d89177ebe2308d62af5e74885dccc3"),
            token0: addr("0x912CE59144191C1204E64559FE8253a0e49E6548"),
            token1: addr("0xaf88d065e77c8cC2239327C5EDb3A432268e5831"),
            fee: 3000,
            reserve_usd_hint: 710_265.271_5,
        },
    ]
}

fn addr(raw: &str) -> Address {
    raw.parse().expect("invalid static address")
}

fn parse_size_bps_list(raw: &str) -> Result<Vec<u32>> {
    let mut values = Vec::new();

    for part in raw.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }

        let value = trimmed
            .parse::<u32>()
            .with_context(|| format!("failed to parse size bps value `{trimmed}`"))?;
        if value == 0 {
            continue;
        }

        values.push(value);
    }

    if values.is_empty() {
        values.push(10_000);
    }

    values.sort_unstable();
    values.dedup();
    Ok(values)
}
