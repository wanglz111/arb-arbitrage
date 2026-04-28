use std::{collections::HashMap, env, path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use ethers::types::Address;

#[derive(Clone, Debug)]
pub struct TokenDef {
    pub symbol: &'static str,
    pub address: Address,
    pub decimals: u8,
    pub quote_amount_in: u128,
    pub usd_price_hint: f64,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionCallMode {
    Direct,
    Route,
}

impl ExecutionCallMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Route => "route",
        }
    }

    fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "direct" | "execute" => Ok(Self::Direct),
            "route" | "execute_route" | "executeroute" => Ok(Self::Route),
            value => bail!("invalid SCANNER_EXECUTION_CALL_MODE `{value}`"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ScannerConfig {
    pub rpc_url: String,
    pub log_rpc_url: String,
    pub log_ws_url: Option<String>,
    pub quote_rpc_url: String,
    pub poll_interval: Duration,
    pub ws_backfill_interval: Duration,
    pub max_log_block_range: u64,
    pub ws_reconnect_delay: Duration,
    pub rpc_retry_delay: Duration,
    pub min_candidate_edge_bps: f64,
    pub min_local_edge_bps: f64,
    pub min_profit_usd: f64,
    pub rough_shortlist_size: usize,
    pub local_size_bps: Vec<u32>,
    pub exact_quote_enabled: bool,
    pub max_exact_quotes_per_block: usize,
    pub execution_slippage_bps: u32,
    pub log_execution_calldata: bool,
    pub execution_call_enabled: bool,
    pub require_execution_call: bool,
    pub execution_call_rpc_url: String,
    pub executor_address: Option<Address>,
    pub executor_caller: Option<Address>,
    pub execution_call_mode: ExecutionCallMode,
    pub max_execution_calls_per_block: usize,
    pub debug_summary_enabled: bool,
    pub debug_jsonl_path: PathBuf,
    pub route_catalog_path: PathBuf,
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
        let execution_call_rpc_url =
            env::var("EXECUTION_CALL_RPC_URL").unwrap_or_else(|_| quote_rpc_url.clone());
        let poll_interval_ms = env::var("SCANNER_POLL_MS")
            .unwrap_or_else(|_| "1500".to_string())
            .parse::<u64>()
            .context("failed to parse SCANNER_POLL_MS")?;
        let ws_backfill_interval_ms = env::var("SCANNER_WS_BACKFILL_POLL_MS")
            .unwrap_or_else(|_| "60000".to_string())
            .parse::<u64>()
            .context("failed to parse SCANNER_WS_BACKFILL_POLL_MS")?;
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
        let min_profit_usd = env::var("SCANNER_MIN_PROFIT_USD")
            .unwrap_or_else(|_| "1".to_string())
            .parse::<f64>()
            .context("failed to parse SCANNER_MIN_PROFIT_USD")?;
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
        let execution_call_enabled = parse_bool_env("SCANNER_EXECUTION_CALL_ENABLED", false);
        let require_execution_call = parse_bool_env("SCANNER_REQUIRE_EXECUTION_CALL", false);
        let executor_address = parse_optional_address_env(&["SCANNER_EXECUTOR_ADDRESS"])?;
        let executor_caller = parse_optional_address_env(&[
            "SCANNER_EXECUTOR_CALLER",
            "SCANNER_EXECUTOR_OWNER_ADDRESS",
        ])?;
        let execution_call_mode = ExecutionCallMode::parse(
            &env::var("SCANNER_EXECUTION_CALL_MODE").unwrap_or_else(|_| "direct".to_string()),
        )?;
        let max_execution_calls_per_block = env::var("SCANNER_MAX_EXECUTION_CALLS_PER_BLOCK")
            .unwrap_or_else(|_| "1".to_string())
            .parse::<usize>()
            .context("failed to parse SCANNER_MAX_EXECUTION_CALLS_PER_BLOCK")?;
        let debug_summary_enabled = env::var("SCANNER_DEBUG_SUMMARY_ENABLED")
            .ok()
            .map(|raw| matches!(raw.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
            .unwrap_or(false);
        let debug_jsonl_path = env::var("SCANNER_DEBUG_JSONL_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("data/directional-candidates.jsonl"));
        let route_catalog_path = env::var("SCANNER_ROUTE_CATALOG_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("data/route-catalog.jsonl"));
        let start_from_latest = env::var("SCANNER_START_FROM_LATEST")
            .ok()
            .map(|raw| matches!(raw.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
            .unwrap_or(true);

        if require_execution_call && !execution_call_enabled {
            bail!(
                "SCANNER_REQUIRE_EXECUTION_CALL=true requires SCANNER_EXECUTION_CALL_ENABLED=true"
            );
        }
        if execution_call_enabled && executor_address.is_none() {
            bail!("SCANNER_EXECUTION_CALL_ENABLED=true requires SCANNER_EXECUTOR_ADDRESS");
        }
        if execution_call_enabled && executor_caller.is_none() {
            bail!(
                "SCANNER_EXECUTION_CALL_ENABLED=true requires SCANNER_EXECUTOR_CALLER or SCANNER_EXECUTOR_OWNER_ADDRESS"
            );
        }

        Ok(Self {
            rpc_url,
            log_rpc_url,
            log_ws_url,
            quote_rpc_url,
            poll_interval: Duration::from_millis(poll_interval_ms),
            ws_backfill_interval: Duration::from_millis(ws_backfill_interval_ms),
            max_log_block_range,
            ws_reconnect_delay: Duration::from_millis(ws_reconnect_delay_ms),
            rpc_retry_delay: Duration::from_millis(rpc_retry_delay_ms),
            min_candidate_edge_bps,
            min_local_edge_bps,
            min_profit_usd,
            rough_shortlist_size,
            local_size_bps,
            exact_quote_enabled,
            max_exact_quotes_per_block,
            execution_slippage_bps,
            log_execution_calldata,
            execution_call_enabled,
            require_execution_call,
            execution_call_rpc_url,
            executor_address,
            executor_caller,
            execution_call_mode,
            max_execution_calls_per_block,
            debug_summary_enabled,
            debug_jsonl_path,
            route_catalog_path,
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
            usd_price_hint: 1.0,
        },
        TokenDef {
            symbol: "USDT0",
            address: addr("0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9"),
            decimals: 6,
            quote_amount_in: 1_000_000_000,
            usd_price_hint: 1.0,
        },
        TokenDef {
            symbol: "WETH",
            address: addr("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1"),
            decimals: 18,
            quote_amount_in: 500_000_000_000_000_000,
            usd_price_hint: 3_000.0,
        },
        TokenDef {
            symbol: "WBTC",
            address: addr("0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f"),
            decimals: 8,
            quote_amount_in: 2_000_000,
            usd_price_hint: 100_000.0,
        },
        TokenDef {
            symbol: "cbBTC",
            address: addr("0xcbB7C0000aB88B473b1f5aFd9ef808440eed33Bf"),
            decimals: 8,
            quote_amount_in: 2_000_000,
            usd_price_hint: 100_000.0,
        },
        TokenDef {
            symbol: "ARB",
            address: addr("0x912CE59144191C1204E64559FE8253a0e49E6548"),
            decimals: 18,
            quote_amount_in: 1_000_000_000_000_000_000_000,
            usd_price_hint: 1.0,
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
            name: "WBTC/cbBTC 0.01%",
            address: addr("0x9B42809aaaE8d088eE01FE637E948784730F0386"),
            token0: addr("0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f"),
            token1: addr("0xcbB7C0000aB88B473b1f5aFd9ef808440eed33Bf"),
            fee: 100,
            reserve_usd_hint: 1_000_000.0,
        },
        PoolDef {
            name: "WBTC/cbBTC 0.05%",
            address: addr("0xE9f9F89bf71548Fefc9b70453B785515B3B98e45"),
            token0: addr("0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f"),
            token1: addr("0xcbB7C0000aB88B473b1f5aFd9ef808440eed33Bf"),
            fee: 500,
            reserve_usd_hint: 1_000_000.0,
        },
        PoolDef {
            name: "USDC/cbBTC 0.01%",
            address: addr("0x78d218D8549D5AB2E25fB7166219baBb3E9446C5"),
            token0: addr("0xaf88d065e77c8cC2239327C5EDb3A432268e5831"),
            token1: addr("0xcbB7C0000aB88B473b1f5aFd9ef808440eed33Bf"),
            fee: 100,
            reserve_usd_hint: 1_000_000.0,
        },
        PoolDef {
            name: "USDT0/cbBTC 0.01%",
            address: addr("0x56dBe966Ea9A9Ce3C449724D00F5DC619f74762D"),
            token0: addr("0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9"),
            token1: addr("0xcbB7C0000aB88B473b1f5aFd9ef808440eed33Bf"),
            fee: 100,
            reserve_usd_hint: 1_000_000.0,
        },
        PoolDef {
            name: "WETH/cbBTC 0.05%",
            address: addr("0xb48B15861f9c5b513690fAD7240d741cb40798dE"),
            token0: addr("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1"),
            token1: addr("0xcbB7C0000aB88B473b1f5aFd9ef808440eed33Bf"),
            fee: 500,
            reserve_usd_hint: 1_000_000.0,
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

fn parse_bool_env(key: &str, default: bool) -> bool {
    env::var(key)
        .ok()
        .map(|raw| matches!(raw.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(default)
}

fn parse_optional_address_env(keys: &[&str]) -> Result<Option<Address>> {
    for key in keys {
        let Ok(raw) = env::var(key) else {
            continue;
        };
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }

        return trimmed
            .parse::<Address>()
            .with_context(|| format!("failed to parse {key}"))
            .map(Some);
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::ExecutionCallMode;

    #[test]
    fn execution_call_mode_accepts_direct_and_route_aliases() {
        assert_eq!(
            ExecutionCallMode::parse("direct").expect("direct"),
            ExecutionCallMode::Direct
        );
        assert_eq!(
            ExecutionCallMode::parse("executeRoute").expect("route"),
            ExecutionCallMode::Route
        );
    }
}
