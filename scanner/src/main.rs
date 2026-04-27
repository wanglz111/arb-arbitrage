mod config;
mod execute;
mod graph;
mod path;
mod quote;
mod simulate;
mod state;
mod watcher;

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use dotenvy::from_path;
use ethers::providers::{Http, Middleware, Provider};
use ethers::types::{Address, U256};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::{
    config::{ScannerConfig, TokenDef},
    execute::{ExecutionBuilder, ExecutionPlan},
    graph::TriangleGraph,
    quote::{ExactQuoteResult, QuoteEngine},
    simulate::LocalQuoteResult,
    state::{RpcProvider, ScannerState, SwapStateUpdate},
};

#[derive(Clone, Debug)]
struct RoughCandidate {
    canonical_key: String,
    triangle_label: String,
    fee_label: String,
    triangle: graph::TrianglePath,
    rough_edge_bps: f64,
    local_quote: Option<LocalQuoteResult>,
}

#[derive(Debug)]
struct QuoteThrottle {
    block_number: u64,
    used_quotes: usize,
    seen_keys: HashSet<String>,
}

impl QuoteThrottle {
    fn new() -> Self {
        Self {
            block_number: 0,
            used_quotes: 0,
            seen_keys: HashSet::new(),
        }
    }

    fn should_quote(&mut self, block_number: u64, canonical_key: &str, budget: usize) -> bool {
        if self.block_number != block_number {
            self.block_number = block_number;
            self.used_quotes = 0;
            self.seen_keys.clear();
        }

        if self.seen_keys.contains(canonical_key) || self.used_quotes >= budget {
            return false;
        }

        self.seen_keys.insert(canonical_key.to_string());
        self.used_quotes += 1;
        true
    }
}

#[derive(Clone, Debug)]
struct MaxEdgeObservation {
    triangle_label: String,
    start_token: Address,
    edge_bps: f64,
}

#[derive(Clone, Debug)]
struct MaxProfitObservation {
    triangle_label: String,
    gross_profit: f64,
}

#[derive(Default, Debug)]
struct DebugSummaryCollector {
    positive_candidates: usize,
    max_local_edge: Option<MaxEdgeObservation>,
    max_local_gross_profit_by_token: HashMap<Address, MaxProfitObservation>,
    triangle_hits: HashMap<String, usize>,
}

impl DebugSummaryCollector {
    fn record_candidate(&mut self, candidate: &RoughCandidate) {
        let Some(local_quote) = candidate.local_quote.as_ref() else {
            return;
        };
        if !local_quote.gross_profit.is_finite() || local_quote.gross_profit <= 0.0 {
            return;
        }

        self.positive_candidates += 1;

        if self
            .max_local_edge
            .as_ref()
            .map(|current| local_quote.edge_bps > current.edge_bps)
            .unwrap_or(true)
        {
            self.max_local_edge = Some(MaxEdgeObservation {
                triangle_label: candidate.triangle_label.clone(),
                start_token: candidate.triangle.start_token,
                edge_bps: local_quote.edge_bps,
            });
        }

        let profit_entry = self
            .max_local_gross_profit_by_token
            .entry(candidate.triangle.start_token)
            .or_insert_with(|| MaxProfitObservation {
                triangle_label: candidate.triangle_label.clone(),
                gross_profit: local_quote.gross_profit,
            });
        if local_quote.gross_profit > profit_entry.gross_profit {
            *profit_entry = MaxProfitObservation {
                triangle_label: candidate.triangle_label.clone(),
                gross_profit: local_quote.gross_profit,
            };
        }

        *self
            .triangle_hits
            .entry(candidate.triangle_label.clone())
            .or_default() += 1;
    }

    fn log_and_reset(&mut self, interval: Duration, token_map: &HashMap<Address, TokenDef>) {
        let snapshot = std::mem::take(self);

        if snapshot.positive_candidates == 0 {
            info!(
                window_secs = interval.as_secs(),
                positive_candidates = 0,
                max_local_edge_bps = "n/a",
                max_local_edge_triangle = "n/a",
                max_local_edge_token = "n/a",
                max_local_gross_profit = "n/a",
                most_common_triangle = "n/a",
                most_common_triangle_hits = 0,
                "debug candidate summary"
            );
            return;
        }

        let max_local_edge = snapshot
            .max_local_edge
            .as_ref()
            .map(|observation| format!("{:.2}", observation.edge_bps))
            .unwrap_or_else(|| "n/a".to_string());
        let max_local_edge_triangle = snapshot
            .max_local_edge
            .as_ref()
            .map(|observation| observation.triangle_label.clone())
            .unwrap_or_else(|| "n/a".to_string());
        let max_local_edge_token = snapshot
            .max_local_edge
            .as_ref()
            .and_then(|observation| token_map.get(&observation.start_token))
            .map(|token| token.symbol)
            .unwrap_or("UNKNOWN");
        let max_local_gross_profit =
            format_profit_summary(&snapshot.max_local_gross_profit_by_token, token_map);
        let (most_common_triangle, most_common_triangle_hits) = snapshot
            .triangle_hits
            .into_iter()
            .max_by_key(|(_, hits)| *hits)
            .unwrap_or_else(|| ("n/a".to_string(), 0usize));

        info!(
            window_secs = interval.as_secs(),
            positive_candidates = snapshot.positive_candidates,
            max_local_edge_bps = max_local_edge,
            max_local_edge_triangle = max_local_edge_triangle,
            max_local_edge_token = max_local_edge_token,
            max_local_gross_profit = max_local_gross_profit,
            most_common_triangle = most_common_triangle,
            most_common_triangle_hits = most_common_triangle_hits,
            "debug candidate summary"
        );
    }
}

struct CandidateProcessor<'a> {
    config: &'a ScannerConfig,
    graph: &'a TriangleGraph,
    token_map: &'a HashMap<Address, TokenDef>,
    quote_engine: &'a Option<QuoteEngine>,
    execution_builder: &'a ExecutionBuilder,
}

impl<'a> CandidateProcessor<'a> {
    async fn apply_swap_update(
        &self,
        source: &'static str,
        state: &mut ScannerState,
        update: SwapStateUpdate,
        quote_throttle: &mut QuoteThrottle,
        debug_summary: &mut DebugSummaryCollector,
    ) {
        let Some(pool) = self
            .config
            .pools
            .iter()
            .find(|pool| pool.address == update.pool)
        else {
            return;
        };

        info!(
            source = source,
            pool = pool.name,
            block = update.block_number,
            log_index = update.log_index,
            "received swap event"
        );

        if !state.apply_swap_update(pool, &update) {
            if let Some(pool_state) = state.pools.get(&update.pool) {
                info!(
                    source = source,
                    pool = pool.name,
                    block = update.block_number,
                    log_index = update.log_index,
                    current_block = pool_state.last_updated_block,
                    current_log_index = pool_state.last_updated_log_index,
                    "skipped stale event"
                );
            }
            return;
        }

        if let Some(pool_state) = state.pools.get(&update.pool) {
            info!(
                source = source,
                pool = pool_state.pool.name,
                tick = pool_state.tick,
                liquidity = pool_state.liquidity.to_string(),
                updated_block = pool_state.last_updated_block,
                log_index = pool_state.last_updated_log_index,
                reserve_hint_usd = format!("{:.0}", pool_state.pool.reserve_usd_hint),
                "applied swap event to pool state"
            );
        }

        let mut changed_pools = HashSet::new();
        changed_pools.insert(update.pool);
        self.process_affected_triangles(
            state,
            &changed_pools,
            update.block_number,
            quote_throttle,
            debug_summary,
        )
        .await;
    }

    async fn process_affected_triangles(
        &self,
        state: &ScannerState,
        changed_pools: &HashSet<Address>,
        block_number: u64,
        quote_throttle: &mut QuoteThrottle,
        debug_summary: &mut DebugSummaryCollector,
    ) {
        let shortlist = build_shortlist(
            self.graph,
            state,
            self.token_map,
            changed_pools,
            self.config,
        );

        for candidate in shortlist {
            let local_execution_plan = candidate.local_quote.as_ref().and_then(|quote| {
                self.execution_builder
                    .build_plan(
                        &candidate.triangle,
                        U256::from(quote.amount_in_raw),
                        U256::from(quote.amount_out_raw_floor),
                    )
                    .map_err(|error| {
                        warn!(
                            triangle = candidate.triangle_label,
                            error = %error,
                            "failed to build local execution plan"
                        );
                        error
                    })
                    .ok()
            });
            let quote_result = if let Some(engine) = self.quote_engine {
                if should_exact_quote(&candidate, self.config.min_local_edge_bps)
                    && quote_throttle.should_quote(
                        block_number,
                        &candidate.canonical_key,
                        self.config.max_exact_quotes_per_block,
                    )
                {
                    let Some(local_quote) = candidate.local_quote.as_ref() else {
                        continue;
                    };

                    match engine
                        .quote_triangle_amount(
                            &candidate.triangle,
                            U256::from(local_quote.amount_in_raw),
                        )
                        .await
                    {
                        Ok(result) => Some(result),
                        Err(error) => {
                            warn!(
                                triangle = candidate.triangle_label,
                                error = %error,
                                "exact quote failed"
                            );
                            None
                        }
                    }
                } else {
                    None
                }
            } else {
                None
            };

            let exact_execution_plan = quote_result.as_ref().and_then(|result| {
                self.execution_builder
                    .build_plan(&candidate.triangle, result.amount_in, result.amount_out)
                    .map_err(|error| {
                        warn!(
                            triangle = candidate.triangle_label,
                            error = %error,
                            "failed to build exact execution plan"
                        );
                        error
                    })
                    .ok()
            });
            let execution_source = if exact_execution_plan.is_some() {
                "exact"
            } else if local_execution_plan.is_some() {
                "local"
            } else {
                "none"
            };
            let execution_plan = exact_execution_plan
                .as_ref()
                .or(local_execution_plan.as_ref());

            if self.config.debug_summary_enabled {
                debug_summary.record_candidate(&candidate);
            } else {
                info_triangle(
                    &candidate,
                    self.token_map,
                    quote_result.as_ref(),
                    execution_plan,
                    execution_source,
                    self.config.log_execution_calldata,
                );
            }
        }
    }
}

#[derive(Debug)]
struct BuildMetadata {
    version: &'static str,
    git_sha: String,
    git_ref: String,
    created_at: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    load_env_file();
    let build_metadata = build_metadata();

    let config = ScannerConfig::from_env()?;
    let token_map = config.token_map();
    let graph = TriangleGraph::build(&config.pools);
    let provider = Arc::new(build_provider(&config.rpc_url)?);
    let log_provider = Arc::new(build_provider(&config.log_rpc_url)?);
    let quote_provider = Arc::new(build_provider(&config.quote_rpc_url)?);
    let quote_engine = if config.exact_quote_enabled {
        Some(QuoteEngine::new(quote_provider)?)
    } else {
        None
    };
    let execution_builder = ExecutionBuilder::new(&graph.triangles, config.execution_slippage_bps)?;
    let processor = CandidateProcessor {
        config: &config,
        graph: &graph,
        token_map: &token_map,
        quote_engine: &quote_engine,
        execution_builder: &execution_builder,
    };
    let mut live_receiver = watcher::spawn_live_swap_watcher(config.clone());
    let mut quote_throttle = QuoteThrottle::new();
    let mut debug_summary = DebugSummaryCollector::default();
    let tracked_reserve_hint_usd: f64 = config.pools.iter().map(|pool| pool.reserve_usd_hint).sum();
    let mut debug_summary_interval = config
        .debug_summary_enabled
        .then(|| tokio::time::interval(config.debug_summary_interval));
    if let Some(interval) = debug_summary_interval.as_mut() {
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
    }

    info!(
        pools = config.pools.len(),
        triangles = graph.triangles.len(),
        poll_ms = config.poll_interval.as_millis(),
        max_log_block_range = config.max_log_block_range,
        ws_live_logs = config.log_ws_url.is_some(),
        rpc_retry_ms = config.rpc_retry_delay.as_millis(),
        min_candidate_edge_bps = format!("{:.2}", config.min_candidate_edge_bps),
        min_local_edge_bps = format!("{:.2}", config.min_local_edge_bps),
        rough_shortlist_size = config.rough_shortlist_size,
        local_size_bps = format_size_bps_list(&config.local_size_bps),
        exact_quote_enabled = config.exact_quote_enabled,
        max_exact_quotes_per_block = config.max_exact_quotes_per_block,
        execution_slippage_bps = config.execution_slippage_bps,
        log_execution_calldata = config.log_execution_calldata,
        debug_summary_enabled = config.debug_summary_enabled,
        debug_summary_interval_secs = config.debug_summary_interval.as_secs(),
        tracked_reserve_hint_usd = format!("{tracked_reserve_hint_usd:.0}"),
        scanner_version = build_metadata.version,
        image_git_sha = build_metadata.git_sha.as_str(),
        image_git_ref = build_metadata.git_ref.as_str(),
        image_created_at = build_metadata.created_at.as_str(),
        "scanner starting"
    );

    let latest_block =
        fetch_latest_block_with_retry(provider.clone(), config.rpc_retry_delay).await;
    let mut next_block = if config.start_from_latest {
        latest_block.saturating_add(1)
    } else {
        latest_block
    };

    let mut state = ScannerState::bootstrap(provider.clone(), &config, latest_block).await?;
    info!(
        block = latest_block,
        tracked_pools = state.pools.len(),
        "bootstrap complete"
    );

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                if config.debug_summary_enabled {
                    debug_summary.log_and_reset(config.debug_summary_interval, &token_map);
                }
                info!("received ctrl-c, shutting down");
                break;
            }
            _ = recv_debug_summary_tick(&mut debug_summary_interval) => {
                debug_summary.log_and_reset(config.debug_summary_interval, &token_map);
            }
            Some(update) = recv_live_event(&mut live_receiver) => {
                processor
                    .apply_swap_update(
                        "live",
                        &mut state,
                        update,
                        &mut quote_throttle,
                        &mut debug_summary,
                    )
                .await;
            }
            _ = tokio::time::sleep(config.poll_interval) => {
                let latest_block = fetch_latest_block_with_retry(provider.clone(), config.rpc_retry_delay).await;

                if latest_block < next_block {
                    continue;
                }

                let changed_pools = watcher::poll_changed_pools(
                    log_provider.clone(),
                    &config,
                    next_block,
                    latest_block,
                )
                .await;

                let updates = match changed_pools {
                    Ok(updates) => updates,
                    Err(error) => {
                        warn!(
                            from_block = next_block,
                            to_block = latest_block,
                            error = %error,
                            "failed to backfill swap logs"
                        );
                        next_block = latest_block.saturating_add(1);
                        continue;
                    }
                };

                if updates.is_empty() {
                    next_block = latest_block.saturating_add(1);
                    continue;
                }

                let unique_pools: HashSet<_> = updates.iter().map(|update| update.pool).collect();
                info!(
                    from_block = next_block,
                    to_block = latest_block,
                    swap_events = updates.len(),
                    changed_pools = unique_pools.len(),
                    "detected backfill swap activity"
                );

                for update in updates {
                    processor
                        .apply_swap_update(
                            "backfill",
                            &mut state,
                            update,
                            &mut quote_throttle,
                            &mut debug_summary,
                        )
                        .await;
                }
                next_block = latest_block.saturating_add(1);
            }
        }
    }

    Ok(())
}

fn load_env_file() {
    let repo_env = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("scanner crate should have a parent directory")
        .join(".env");

    if repo_env.exists() {
        let _ = from_path(&repo_env);
    }
}

fn build_provider(rpc_url: &str) -> Result<RpcProvider> {
    Provider::<Http>::try_from(rpc_url)
        .context("failed to create HTTP provider")
        .map(|provider| provider.interval(std::time::Duration::from_millis(250)))
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "scanner=info".into());

    tracing_subscriber::fmt().with_env_filter(filter).init();
}

fn build_metadata() -> BuildMetadata {
    BuildMetadata {
        version: env!("CARGO_PKG_VERSION"),
        git_sha: read_build_env("SCANNER_BUILD_GIT_SHA"),
        git_ref: read_build_env("SCANNER_BUILD_GIT_REF"),
        created_at: read_build_env("SCANNER_BUILD_CREATED"),
    }
}

fn read_build_env(key: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

async fn recv_live_event(
    receiver: &mut Option<mpsc::Receiver<SwapStateUpdate>>,
) -> Option<SwapStateUpdate> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending::<Option<SwapStateUpdate>>().await,
    }
}

async fn recv_debug_summary_tick(interval: &mut Option<tokio::time::Interval>) {
    match interval {
        Some(interval) => {
            interval.tick().await;
        }
        None => std::future::pending::<()>().await,
    }
}

async fn fetch_latest_block_with_retry(
    provider: Arc<RpcProvider>,
    retry_delay: std::time::Duration,
) -> u64 {
    loop {
        match provider.get_block_number().await {
            Ok(block) => return block.as_u64(),
            Err(error) => {
                warn!(error = %error, retry_ms = retry_delay.as_millis(), "failed to fetch latest block, retrying");
                tokio::time::sleep(retry_delay).await;
            }
        }
    }
}

fn build_shortlist(
    graph: &TriangleGraph,
    state: &ScannerState,
    token_map: &HashMap<Address, TokenDef>,
    changed_pools: &HashSet<Address>,
    config: &ScannerConfig,
) -> Vec<RoughCandidate> {
    let mut best_by_key = HashMap::new();

    for pool in changed_pools {
        for triangle in graph.affected_triangles(*pool) {
            match graph.rough_cycle_edge_bps(triangle, state, token_map) {
                Some(rough_edge_bps) => {
                    if rough_edge_bps < config.min_candidate_edge_bps {
                        continue;
                    }

                    let canonical = triangle.canonical_view();
                    let display = triangle.display_view(token_map);
                    let candidate = RoughCandidate {
                        canonical_key: canonical.dedupe_key.clone(),
                        triangle_label: display.label,
                        fee_label: display.fee_label,
                        triangle: triangle.clone(),
                        rough_edge_bps,
                        local_quote: simulate::find_best_local_size(
                            triangle,
                            state,
                            token_map,
                            &config.local_size_bps,
                        ),
                    };

                    if let Some(local_quote) = &candidate.local_quote
                        && !passes_local_candidate_filter(
                            local_quote,
                            config.min_local_edge_bps,
                            config.exact_quote_enabled,
                        )
                    {
                        continue;
                    }

                    match best_by_key.get(&canonical.dedupe_key) {
                        Some(existing) if !prefer_candidate(&candidate, existing, token_map) => {
                            continue;
                        }
                        _ => {
                            best_by_key.insert(canonical.dedupe_key, candidate);
                        }
                    }
                }
                None => warn!(triangle = %triangle.id, "unable to score triangle"),
            }
        }
    }

    let mut candidates: Vec<_> = best_by_key.into_values().collect();
    candidates.sort_by(|left, right| {
        candidate_score(right)
            .partial_cmp(&candidate_score(left))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                preferred_quote_rank(token_map, left.triangle.start_token)
                    .cmp(&preferred_quote_rank(token_map, right.triangle.start_token))
            })
    });
    candidates.truncate(config.rough_shortlist_size);
    candidates
}

fn prefer_candidate(
    candidate: &RoughCandidate,
    existing: &RoughCandidate,
    token_map: &HashMap<Address, TokenDef>,
) -> bool {
    let candidate_rank = preferred_quote_rank(token_map, candidate.triangle.start_token);
    let existing_rank = preferred_quote_rank(token_map, existing.triangle.start_token);
    let candidate_edge = candidate_score(candidate);
    let existing_edge = candidate_score(existing);

    candidate_rank < existing_rank
        || (candidate_rank == existing_rank && candidate_edge > existing_edge)
}

fn preferred_quote_rank(token_map: &HashMap<Address, TokenDef>, token: Address) -> usize {
    match token_map.get(&token).map(|def| def.symbol) {
        Some("USDC") => 0,
        Some("USDT0") => 1,
        Some("WETH") => 2,
        Some("WBTC") => 3,
        Some("ARB") => 4,
        _ => 100,
    }
}

fn info_triangle(
    candidate: &RoughCandidate,
    token_map: &HashMap<Address, TokenDef>,
    quote_result: Option<&ExactQuoteResult>,
    execution_plan: Option<&ExecutionPlan>,
    execution_source: &'static str,
    log_execution_calldata: bool,
) {
    let token = token_map
        .get(&candidate.triangle.start_token)
        .map(|token| token.symbol)
        .unwrap_or("UNKNOWN");

    if let Some(result) = quote_result {
        info!(
            triangle = candidate.triangle_label,
            fees = candidate.fee_label,
            start_token = token,
            rough_edge_bps = format!("{:.2}", candidate.rough_edge_bps),
            local_edge_bps = candidate
                .local_quote
                .as_ref()
                .map(|quote| format!("{:.2}", quote.edge_bps))
                .unwrap_or_else(|| "n/a".to_string()),
            local_crosses_tick = candidate
                .local_quote
                .as_ref()
                .map(|quote| quote.crosses_tick)
                .unwrap_or(false),
            crossed_tick_legs = candidate
                .local_quote
                .as_ref()
                .map(|quote| quote.crossed_tick_legs)
                .unwrap_or(0),
            max_headroom_ratio = candidate
                .local_quote
                .as_ref()
                .map(|quote| format!("{:.4}", quote.max_headroom_ratio))
                .unwrap_or_else(|| "n/a".to_string()),
            local_amount_in = candidate
                .local_quote
                .as_ref()
                .map(|quote| format_amount_f64(
                    quote.amount_in,
                    token_map,
                    candidate.triangle.start_token
                ))
                .unwrap_or_else(|| "n/a".to_string()),
            local_amount_out = candidate
                .local_quote
                .as_ref()
                .map(|quote| format_amount_f64(
                    quote.amount_out,
                    token_map,
                    candidate.triangle.start_token
                ))
                .unwrap_or_else(|| "n/a".to_string()),
            local_gross_profit = candidate
                .local_quote
                .as_ref()
                .map(|quote| format_amount_f64(
                    quote.gross_profit,
                    token_map,
                    candidate.triangle.start_token
                ))
                .unwrap_or_else(|| "n/a".to_string()),
            local_size_bps = candidate
                .local_quote
                .as_ref()
                .map(|quote| quote.size_bps)
                .unwrap_or_default(),
            local_search_samples = candidate
                .local_quote
                .as_ref()
                .map(|quote| quote.search_samples)
                .unwrap_or_default(),
            local_refinement_samples = candidate
                .local_quote
                .as_ref()
                .map(|quote| quote.refinement_samples)
                .unwrap_or_default(),
            exact_edge_bps = format!("{:.2}", exact_edge_bps(result)),
            amount_in = format_amount(&result.amount_in, token_map, candidate.triangle.start_token),
            amount_out = format_amount(
                &result.amount_out,
                token_map,
                candidate.triangle.start_token
            ),
            gas_estimate = result.gas_estimate.to_string(),
            execution_loan_token = execution_plan
                .map(|plan| {
                    token_map
                        .get(&plan.loan_token)
                        .map(|token| token.symbol.to_string())
                        .unwrap_or_else(|| "UNKNOWN".to_string())
                })
                .unwrap_or_else(|| "n/a".to_string()),
            execution_source = execution_source,
            execution_amount_in = execution_plan
                .map(|plan| format_amount(&plan.amount_in, token_map, plan.loan_token))
                .unwrap_or_else(|| "n/a".to_string()),
            execution_expected_amount_out = execution_plan
                .map(|plan| format_amount(&plan.expected_amount_out, token_map, plan.loan_token))
                .unwrap_or_else(|| "n/a".to_string()),
            execution_slippage_bps = execution_plan
                .map(|plan| plan.slippage_bps)
                .unwrap_or_default(),
            execution_amount_out_minimum = execution_plan
                .map(|plan| format_amount(
                    &plan.amount_out_minimum,
                    token_map,
                    candidate.triangle.start_token
                ))
                .unwrap_or_else(|| "n/a".to_string()),
            execution_path_bytes = execution_plan
                .map(|plan| plan.path.len())
                .unwrap_or_default(),
            execution_calldata_bytes = execution_plan
                .map(|plan| plan.execute_calldata.len())
                .unwrap_or_default(),
            execution_call_data = if log_execution_calldata {
                execution_plan
                    .map(|plan| plan.calldata_hex())
                    .unwrap_or_else(|| "n/a".to_string())
            } else {
                "disabled".to_string()
            },
            quoted = true,
            "affected triangle rescored"
        );
        return;
    }

    info!(
        triangle = candidate.triangle_label,
        fees = candidate.fee_label,
        start_token = token,
        rough_edge_bps = format!("{:.2}", candidate.rough_edge_bps),
        local_edge_bps = candidate
            .local_quote
            .as_ref()
            .map(|quote| format!("{:.2}", quote.edge_bps))
            .unwrap_or_else(|| "n/a".to_string()),
        local_crosses_tick = candidate
            .local_quote
            .as_ref()
            .map(|quote| quote.crosses_tick)
            .unwrap_or(false),
        crossed_tick_legs = candidate
            .local_quote
            .as_ref()
            .map(|quote| quote.crossed_tick_legs)
            .unwrap_or(0),
        max_headroom_ratio = candidate
            .local_quote
            .as_ref()
            .map(|quote| format!("{:.4}", quote.max_headroom_ratio))
            .unwrap_or_else(|| "n/a".to_string()),
        local_amount_in = candidate
            .local_quote
            .as_ref()
            .map(|quote| format_amount_f64(
                quote.amount_in,
                token_map,
                candidate.triangle.start_token
            ))
            .unwrap_or_else(|| "n/a".to_string()),
        local_amount_out = candidate
            .local_quote
            .as_ref()
            .map(|quote| format_amount_f64(
                quote.amount_out,
                token_map,
                candidate.triangle.start_token
            ))
            .unwrap_or_else(|| "n/a".to_string()),
        local_gross_profit = candidate
            .local_quote
            .as_ref()
            .map(|quote| format_amount_f64(
                quote.gross_profit,
                token_map,
                candidate.triangle.start_token
            ))
            .unwrap_or_else(|| "n/a".to_string()),
        local_size_bps = candidate
            .local_quote
            .as_ref()
            .map(|quote| quote.size_bps)
            .unwrap_or_default(),
        execution_source = execution_source,
        execution_loan_token = execution_plan
            .map(|plan| {
                token_map
                    .get(&plan.loan_token)
                    .map(|token| token.symbol.to_string())
                    .unwrap_or_else(|| "UNKNOWN".to_string())
            })
            .unwrap_or_else(|| "n/a".to_string()),
        execution_amount_in = execution_plan
            .map(|plan| format_amount(&plan.amount_in, token_map, plan.loan_token))
            .unwrap_or_else(|| "n/a".to_string()),
        execution_expected_amount_out = execution_plan
            .map(|plan| format_amount(&plan.expected_amount_out, token_map, plan.loan_token))
            .unwrap_or_else(|| "n/a".to_string()),
        execution_slippage_bps = execution_plan
            .map(|plan| plan.slippage_bps)
            .unwrap_or_default(),
        execution_amount_out_minimum = execution_plan
            .map(|plan| format_amount(
                &plan.amount_out_minimum,
                token_map,
                candidate.triangle.start_token
            ))
            .unwrap_or_else(|| "n/a".to_string()),
        execution_path_bytes = execution_plan
            .map(|plan| plan.path.len())
            .unwrap_or_default(),
        execution_calldata_bytes = execution_plan
            .map(|plan| plan.execute_calldata.len())
            .unwrap_or_default(),
        execution_call_data = if log_execution_calldata {
            execution_plan
                .map(|plan| plan.calldata_hex())
                .unwrap_or_else(|| "n/a".to_string())
        } else {
            "disabled".to_string()
        },
        local_search_samples = candidate
            .local_quote
            .as_ref()
            .map(|quote| quote.search_samples)
            .unwrap_or_default(),
        local_refinement_samples = candidate
            .local_quote
            .as_ref()
            .map(|quote| quote.refinement_samples)
            .unwrap_or_default(),
        quoted = false,
        "affected triangle rescored"
    );
}

fn should_exact_quote(candidate: &RoughCandidate, min_local_edge_bps: f64) -> bool {
    candidate
        .local_quote
        .as_ref()
        .map(|quote| quote.crosses_tick || quote.edge_bps >= min_local_edge_bps)
        .unwrap_or(false)
}

fn passes_local_candidate_filter(
    quote: &LocalQuoteResult,
    min_local_edge_bps: f64,
    exact_quote_enabled: bool,
) -> bool {
    if exact_quote_enabled && quote.crosses_tick {
        return true;
    }

    quote.edge_bps >= min_local_edge_bps
}

fn candidate_score(candidate: &RoughCandidate) -> f64 {
    candidate
        .local_quote
        .as_ref()
        .map(|quote| quote.edge_bps)
        .unwrap_or(candidate.rough_edge_bps)
}

fn exact_edge_bps(result: &ExactQuoteResult) -> f64 {
    if result.amount_in.is_zero() {
        return 0.0;
    }

    let amount_in = u256_to_f64(&result.amount_in);
    let amount_out = u256_to_f64(&result.amount_out);
    ((amount_out / amount_in) - 1.0) * 10_000.0
}

fn format_amount(amount: &U256, token_map: &HashMap<Address, TokenDef>, token: Address) -> String {
    let decimals = token_map.get(&token).map(|def| def.decimals).unwrap_or(18);
    ethers::utils::format_units(*amount, usize::from(decimals))
        .unwrap_or_else(|_| amount.to_string())
}

fn format_amount_f64(
    amount: f64,
    token_map: &HashMap<Address, TokenDef>,
    token: Address,
) -> String {
    let decimals = i32::from(token_map.get(&token).map(|def| def.decimals).unwrap_or(18));
    let scaled = amount / 10f64.powi(decimals);

    if scaled.is_finite() {
        format!("{scaled:.6}")
    } else {
        "n/a".to_string()
    }
}

fn format_profit_summary(
    by_token: &HashMap<Address, MaxProfitObservation>,
    token_map: &HashMap<Address, TokenDef>,
) -> String {
    let mut tokens: Vec<_> = by_token.keys().copied().collect();
    tokens.sort_by_key(|token| preferred_quote_rank(token_map, *token));

    tokens
        .into_iter()
        .filter_map(|token| {
            let observation = by_token.get(&token)?;
            let symbol = token_map
                .get(&token)
                .map(|def| def.symbol)
                .unwrap_or("UNKNOWN");
            Some(format!(
                "{symbol}:{profit} via {triangle}",
                profit = format_amount_f64(observation.gross_profit, token_map, token),
                triangle = observation.triangle_label,
            ))
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn u256_to_f64(value: &U256) -> f64 {
    value.to_string().parse::<f64>().unwrap_or(0.0)
}

fn format_size_bps_list(values: &[u32]) -> String {
    values
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}
