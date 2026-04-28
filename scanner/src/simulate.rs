use std::collections::{BTreeMap, BTreeSet, HashMap};

use ethers::types::Address;

use crate::{
    config::TokenDef,
    graph::TrianglePath,
    state::{PoolState, ScannerState},
};

const Q96_F64: f64 = 79_228_162_514_264_337_593_543_950_336.0;
const LOCAL_REFINE_ROUNDS: usize = 2;
const LOCAL_TERNARY_ROUNDS: usize = 12;
const MAX_TICK_CROSSES_PER_LEG: usize = 256;

#[derive(Clone, Debug)]
pub struct LocalQuoteResult {
    pub amount_in_raw: u128,
    pub amount_out_raw_floor: u128,
    pub amount_in: f64,
    pub amount_out: f64,
    pub gross_profit: f64,
    pub edge_bps: f64,
    pub size_bps: u32,
    pub search_samples: usize,
    pub refinement_samples: usize,
    pub crosses_tick: bool,
    pub crossed_tick_legs: u8,
    pub max_headroom_ratio: f64,
}

#[derive(Clone, Debug)]
pub struct LocalLegResult {
    pub amount_out: f64,
    pub crosses_tick: bool,
    pub headroom_ratio: f64,
}

pub fn find_best_local_size(
    triangle: &TrianglePath,
    state: &ScannerState,
    token_map: &HashMap<Address, TokenDef>,
    size_bps: &[u32],
) -> Option<LocalQuoteResult> {
    let token_in = token_map.get(&triangle.start_token)?;
    let candidate_sizes = build_candidate_sizes(token_in.quote_amount_in, size_bps);
    let coarse_sample_count = candidate_sizes.len();
    let mut sampled_bps: Vec<u32> = candidate_sizes
        .iter()
        .map(|(_, scale_bps)| *scale_bps)
        .collect();
    let mut sampled_amounts: BTreeSet<u128> = candidate_sizes
        .iter()
        .map(|(amount_in_raw, _)| *amount_in_raw)
        .collect();
    let mut best: Option<LocalQuoteResult> = None;

    for (amount_in_raw, scale_bps) in &candidate_sizes {
        let Some(candidate) = simulate_triangle_amount(triangle, state, *amount_in_raw, *scale_bps)
        else {
            continue;
        };

        if best
            .as_ref()
            .map(|current| prefer_local_quote(&candidate, current))
            .unwrap_or(true)
        {
            best = Some(candidate);
        }
    }

    let mut refinement_sample_count = 0usize;

    for _ in 0..LOCAL_REFINE_ROUNDS {
        let Some(best_size_bps) = best.as_ref().map(|quote| quote.size_bps) else {
            break;
        };

        let refinement_bps = build_refinement_size_bps(&sampled_bps, best_size_bps);
        let mut added_new_sample = false;

        for refine_bps in refinement_bps {
            insert_sorted_unique(&mut sampled_bps, refine_bps);

            let amount_in_raw = scale_amount_in(token_in.quote_amount_in, refine_bps);
            if amount_in_raw == 0 || !sampled_amounts.insert(amount_in_raw) {
                continue;
            }

            refinement_sample_count += 1;
            added_new_sample = true;

            let Some(candidate) =
                simulate_triangle_amount(triangle, state, amount_in_raw, refine_bps)
            else {
                continue;
            };

            if best
                .as_ref()
                .map(|current| prefer_local_quote(&candidate, current))
                .unwrap_or(true)
            {
                best = Some(candidate);
            }
        }

        if !added_new_sample {
            break;
        }
    }

    let ternary_sample_count = run_ternary_size_search(
        triangle,
        state,
        token_in.quote_amount_in,
        &mut sampled_bps,
        &mut sampled_amounts,
        &mut best,
    );
    refinement_sample_count += ternary_sample_count;

    best.map(|mut quote| {
        quote.search_samples = coarse_sample_count + refinement_sample_count;
        quote.refinement_samples = refinement_sample_count;
        quote
    })
}

fn run_ternary_size_search(
    triangle: &TrianglePath,
    state: &ScannerState,
    base_amount_in: u128,
    sampled_bps: &mut Vec<u32>,
    sampled_amounts: &mut BTreeSet<u128>,
    best: &mut Option<LocalQuoteResult>,
) -> usize {
    let Some(best_size_bps) = best.as_ref().map(|quote| quote.size_bps) else {
        return 0;
    };
    let Some((mut lower, mut upper)) = ternary_search_bounds(sampled_bps, best_size_bps) else {
        return 0;
    };

    let mut search = TernarySizeSearch {
        triangle,
        state,
        base_amount_in,
        sampled_bps,
        sampled_amounts,
        best,
        samples: 0,
    };

    for _ in 0..LOCAL_TERNARY_ROUNDS {
        if upper <= lower + 2 {
            break;
        }

        let span = upper - lower;
        let left = lower + (span / 3);
        let right = upper - (span / 3);
        if left == right {
            break;
        }

        let left_quote = search.sample_size_bps(left);
        let right_quote = search.sample_size_bps(right);

        let left_profit = left_quote
            .as_ref()
            .map(|quote| quote.gross_profit)
            .unwrap_or(f64::NEG_INFINITY);
        let right_profit = right_quote
            .as_ref()
            .map(|quote| quote.gross_profit)
            .unwrap_or(f64::NEG_INFINITY);

        if left_profit < right_profit {
            lower = left;
        } else {
            upper = right;
        }
    }

    search.samples
}

struct TernarySizeSearch<'a> {
    triangle: &'a TrianglePath,
    state: &'a ScannerState,
    base_amount_in: u128,
    sampled_bps: &'a mut Vec<u32>,
    sampled_amounts: &'a mut BTreeSet<u128>,
    best: &'a mut Option<LocalQuoteResult>,
    samples: usize,
}

impl TernarySizeSearch<'_> {
    fn sample_size_bps(&mut self, size_bps: u32) -> Option<LocalQuoteResult> {
        insert_sorted_unique(self.sampled_bps, size_bps);

        let amount_in_raw = scale_amount_in(self.base_amount_in, size_bps);
        if amount_in_raw == 0 || !self.sampled_amounts.insert(amount_in_raw) {
            return None;
        }

        self.samples += 1;
        let candidate =
            simulate_triangle_amount(self.triangle, self.state, amount_in_raw, size_bps)?;
        if self
            .best
            .as_ref()
            .map(|current| prefer_local_quote(&candidate, current))
            .unwrap_or(true)
        {
            *self.best = Some(candidate.clone());
        }
        Some(candidate)
    }
}

pub fn simulate_triangle_amount(
    triangle: &TrianglePath,
    state: &ScannerState,
    amount_in_raw: u128,
    size_bps: u32,
) -> Option<LocalQuoteResult> {
    let amount_in = amount_in_raw as f64;
    if amount_in <= 0.0 {
        return None;
    }

    let mut amount = amount_in;
    let mut crossed_tick_legs = 0u8;
    let mut max_headroom_ratio = 0.0f64;
    for leg in triangle.legs() {
        let pool_state = state.pools.get(&leg.pool)?;
        let leg_result = pool_state.simulate_swap_raw(amount, leg.token_in, leg.token_out)?;
        if leg_result.crosses_tick {
            crossed_tick_legs = crossed_tick_legs.saturating_add(1);
        }
        max_headroom_ratio = max_headroom_ratio.max(leg_result.headroom_ratio);
        amount = leg_result.amount_out;
        if !amount.is_finite() || amount <= 0.0 {
            return None;
        }
    }

    let amount_out_raw_floor = amount.floor();
    if !amount_out_raw_floor.is_finite() || amount_out_raw_floor <= 0.0 {
        return None;
    }

    Some(LocalQuoteResult {
        amount_in_raw,
        amount_out_raw_floor: amount_out_raw_floor as u128,
        amount_in,
        amount_out: amount,
        gross_profit: amount - amount_in,
        edge_bps: ((amount / amount_in) - 1.0) * 10_000.0,
        size_bps,
        search_samples: 0,
        refinement_samples: 0,
        crosses_tick: crossed_tick_legs > 0,
        crossed_tick_legs,
        max_headroom_ratio,
    })
}

impl PoolState {
    pub fn simulate_swap_raw(
        &self,
        amount_in_raw: f64,
        token_in: Address,
        token_out: Address,
    ) -> Option<LocalLegResult> {
        if amount_in_raw <= 0.0 {
            return None;
        }

        let fee_factor = 1.0 - (self.pool.fee as f64 / 1_000_000.0);
        let amount_in_less_fee = amount_in_raw * fee_factor;
        if amount_in_less_fee <= 0.0 {
            return None;
        }

        let sqrt_price = self.sqrt_price_x96.to_string().parse::<f64>().ok()? / Q96_F64;
        let mut liquidity = self.liquidity as f64;

        if !sqrt_price.is_finite()
            || sqrt_price <= 0.0
            || !liquidity.is_finite()
            || liquidity <= 0.0
        {
            return None;
        }

        if token_in == self.pool.token0 && token_out == self.pool.token1 {
            return simulate_zero_for_one(self, amount_in_less_fee, sqrt_price, &mut liquidity);
        }

        if token_in == self.pool.token1 && token_out == self.pool.token0 {
            return simulate_one_for_zero(self, amount_in_less_fee, sqrt_price, &mut liquidity);
        }

        None
    }
}

fn simulate_zero_for_one(
    pool: &PoolState,
    mut amount_remaining: f64,
    mut sqrt_price: f64,
    liquidity: &mut f64,
) -> Option<LocalLegResult> {
    let mut amount_out = 0.0f64;
    let mut current_tick = pool.tick;
    let mut crossed_ticks = 0usize;
    let mut max_headroom_ratio = 0.0f64;

    for _ in 0..=MAX_TICK_CROSSES_PER_LEG {
        if amount_remaining <= 0.0 {
            break;
        }

        let Some(next_tick) = pool.next_initialized_tick_below(current_tick) else {
            let sqrt_lower =
                sqrt_ratio_at_tick(current_tick_lower(current_tick, pool.tick_spacing));
            return finish_zero_for_one_without_loaded_tick(
                amount_remaining,
                amount_out,
                sqrt_price,
                *liquidity,
                sqrt_lower,
                crossed_ticks,
                max_headroom_ratio,
            );
        };
        let sqrt_target = sqrt_ratio_at_tick(next_tick);
        let max_amount_in = *liquidity * ((1.0 / sqrt_target) - (1.0 / sqrt_price));
        max_headroom_ratio =
            max_headroom_ratio.max(headroom_ratio(amount_remaining, max_amount_in));

        if amount_remaining < max_amount_in {
            let inv_next = (1.0 / sqrt_price) + (amount_remaining / *liquidity);
            if !inv_next.is_finite() || inv_next <= 0.0 {
                return None;
            }

            let sqrt_price_next = 1.0 / inv_next;
            amount_out += *liquidity * (sqrt_price - sqrt_price_next);
            amount_remaining = 0.0;
            break;
        }

        amount_remaining -= max_amount_in;
        amount_out += *liquidity * (sqrt_price - sqrt_target);
        sqrt_price = sqrt_target;
        crossed_ticks += 1;
        let liquidity_net = pool.initialized_ticks.get(&next_tick).copied()?;
        *liquidity = apply_crossed_liquidity(*liquidity, -liquidity_net);
        if *liquidity <= 0.0 {
            return None;
        }
        current_tick = next_tick;
    }

    if amount_remaining > 0.0 {
        return None;
    }

    Some(LocalLegResult {
        amount_out: positive_finite(amount_out)?,
        crosses_tick: crossed_ticks > 0,
        headroom_ratio: max_headroom_ratio,
    })
}

fn simulate_one_for_zero(
    pool: &PoolState,
    mut amount_remaining: f64,
    mut sqrt_price: f64,
    liquidity: &mut f64,
) -> Option<LocalLegResult> {
    let mut amount_out = 0.0f64;
    let mut current_tick = pool.tick;
    let mut crossed_ticks = 0usize;
    let mut max_headroom_ratio = 0.0f64;

    for _ in 0..=MAX_TICK_CROSSES_PER_LEG {
        if amount_remaining <= 0.0 {
            break;
        }

        let Some(next_tick) = pool.next_initialized_tick_above(current_tick) else {
            let sqrt_upper = sqrt_ratio_at_tick(
                current_tick_lower(current_tick, pool.tick_spacing) + pool.tick_spacing,
            );
            return finish_one_for_zero_without_loaded_tick(
                amount_remaining,
                amount_out,
                sqrt_price,
                *liquidity,
                sqrt_upper,
                crossed_ticks,
                max_headroom_ratio,
            );
        };
        let sqrt_target = sqrt_ratio_at_tick(next_tick);
        let max_amount_in = *liquidity * (sqrt_target - sqrt_price);
        max_headroom_ratio =
            max_headroom_ratio.max(headroom_ratio(amount_remaining, max_amount_in));

        if amount_remaining < max_amount_in {
            let sqrt_price_next = sqrt_price + (amount_remaining / *liquidity);
            if !sqrt_price_next.is_finite() || sqrt_price_next <= 0.0 {
                return None;
            }

            amount_out += *liquidity * ((1.0 / sqrt_price) - (1.0 / sqrt_price_next));
            amount_remaining = 0.0;
            break;
        }

        amount_remaining -= max_amount_in;
        amount_out += *liquidity * ((1.0 / sqrt_price) - (1.0 / sqrt_target));
        sqrt_price = sqrt_target;
        crossed_ticks += 1;
        let liquidity_net = pool.initialized_ticks.get(&next_tick).copied()?;
        *liquidity = apply_crossed_liquidity(*liquidity, liquidity_net);
        if *liquidity <= 0.0 {
            return None;
        }
        current_tick = next_tick;
    }

    if amount_remaining > 0.0 {
        return None;
    }

    Some(LocalLegResult {
        amount_out: positive_finite(amount_out)?,
        crosses_tick: crossed_ticks > 0,
        headroom_ratio: max_headroom_ratio,
    })
}

fn finish_zero_for_one_without_loaded_tick(
    amount_remaining: f64,
    amount_out_so_far: f64,
    sqrt_price: f64,
    liquidity: f64,
    sqrt_lower: f64,
    crossed_ticks: usize,
    max_headroom_ratio: f64,
) -> Option<LocalLegResult> {
    let max_amount_in = liquidity * ((1.0 / sqrt_lower) - (1.0 / sqrt_price));
    let headroom = max_headroom_ratio.max(headroom_ratio(amount_remaining, max_amount_in));
    if amount_remaining > max_amount_in {
        return None;
    }

    let inv_next = (1.0 / sqrt_price) + (amount_remaining / liquidity);
    if !inv_next.is_finite() || inv_next <= 0.0 {
        return None;
    }

    let sqrt_price_next = 1.0 / inv_next;
    Some(LocalLegResult {
        amount_out: positive_finite(
            amount_out_so_far + liquidity * (sqrt_price - sqrt_price_next),
        )?,
        crosses_tick: crossed_ticks > 0,
        headroom_ratio: headroom,
    })
}

fn finish_one_for_zero_without_loaded_tick(
    amount_remaining: f64,
    amount_out_so_far: f64,
    sqrt_price: f64,
    liquidity: f64,
    sqrt_upper: f64,
    crossed_ticks: usize,
    max_headroom_ratio: f64,
) -> Option<LocalLegResult> {
    let max_amount_in = liquidity * (sqrt_upper - sqrt_price);
    let headroom = max_headroom_ratio.max(headroom_ratio(amount_remaining, max_amount_in));
    if amount_remaining > max_amount_in {
        return None;
    }

    let sqrt_price_next = sqrt_price + (amount_remaining / liquidity);
    if !sqrt_price_next.is_finite() || sqrt_price_next <= 0.0 {
        return None;
    }

    Some(LocalLegResult {
        amount_out: positive_finite(
            amount_out_so_far + liquidity * ((1.0 / sqrt_price) - (1.0 / sqrt_price_next)),
        )?,
        crosses_tick: crossed_ticks > 0,
        headroom_ratio: headroom,
    })
}

impl PoolState {
    fn next_initialized_tick_below(&self, current_tick: i32) -> Option<i32> {
        self.initialized_ticks
            .range(..current_tick)
            .next_back()
            .map(|(tick, _)| *tick)
    }

    fn next_initialized_tick_above(&self, current_tick: i32) -> Option<i32> {
        self.initialized_ticks
            .range((current_tick + 1)..)
            .next()
            .map(|(tick, _)| *tick)
    }
}

fn apply_crossed_liquidity(liquidity: f64, liquidity_delta: i128) -> f64 {
    liquidity + liquidity_delta as f64
}

fn current_tick_lower(current_tick: i32, tick_spacing: i32) -> i32 {
    let spacing = tick_spacing.max(1);
    current_tick.div_euclid(spacing) * spacing
}

fn sqrt_ratio_at_tick(tick: i32) -> f64 {
    1.0001_f64.powf(f64::from(tick) / 2.0)
}

fn headroom_ratio(amount_in_less_fee: f64, max_amount_in_less_fee: f64) -> f64 {
    if max_amount_in_less_fee.is_finite() && max_amount_in_less_fee > 0.0 {
        amount_in_less_fee / max_amount_in_less_fee
    } else {
        f64::INFINITY
    }
}

fn positive_finite(value: f64) -> Option<f64> {
    if value.is_finite() && value > 0.0 {
        Some(value)
    } else {
        None
    }
}

fn build_candidate_sizes(base_amount_in: u128, size_bps: &[u32]) -> Vec<(u128, u32)> {
    let mut candidates = BTreeMap::new();

    if size_bps.is_empty() {
        candidates.insert(base_amount_in, 10_000);
    } else {
        for &scale_bps in size_bps {
            let amount_in_raw = scale_amount_in(base_amount_in, scale_bps);
            if amount_in_raw > 0 {
                candidates.entry(amount_in_raw).or_insert(scale_bps);
            }
        }
    }

    candidates.into_iter().collect()
}

fn build_refinement_size_bps(sampled_bps: &[u32], best_size_bps: u32) -> Vec<u32> {
    let normalized = normalize_size_bps(sampled_bps);
    let Some(best_index) = normalized.iter().position(|value| *value == best_size_bps) else {
        return Vec::new();
    };

    let best = normalized[best_index];
    let lower = best_index
        .checked_sub(1)
        .and_then(|index| normalized.get(index).copied());
    let upper = normalized.get(best_index + 1).copied();
    let mut refinement = Vec::new();

    if let Some(lower) = lower {
        if let Some(midpoint) = midpoint_between(lower, best) {
            refinement.push(midpoint);
        }
    } else if best > 1 {
        refinement.push(best / 2);
    }

    if let Some(upper) = upper {
        if let Some(midpoint) = midpoint_between(best, upper) {
            refinement.push(midpoint);
        }
    } else if let Some(lower) = lower {
        let extension = best.saturating_add(best.saturating_sub(lower) / 2);
        if extension > best {
            refinement.push(extension);
        }
    } else {
        refinement.push(best.saturating_mul(2));
    }

    normalize_size_bps(&refinement)
}

fn ternary_search_bounds(sampled_bps: &[u32], best_size_bps: u32) -> Option<(u32, u32)> {
    let normalized = normalize_size_bps(sampled_bps);
    let best_index = normalized
        .iter()
        .position(|value| *value == best_size_bps)?;
    let lower = best_index
        .checked_sub(1)
        .and_then(|index| normalized.get(index).copied())
        .unwrap_or_else(|| best_size_bps.saturating_div(2).max(1));
    let upper = normalized
        .get(best_index + 1)
        .copied()
        .unwrap_or_else(|| best_size_bps.saturating_mul(2));

    (upper > lower + 2).then_some((lower, upper))
}

fn prefer_local_quote(candidate: &LocalQuoteResult, current: &LocalQuoteResult) -> bool {
    candidate
        .gross_profit
        .partial_cmp(&current.gross_profit)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| {
            candidate
                .edge_bps
                .partial_cmp(&current.edge_bps)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .then_with(|| current.crosses_tick.cmp(&candidate.crosses_tick))
        .then_with(|| {
            current
                .max_headroom_ratio
                .partial_cmp(&candidate.max_headroom_ratio)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .then_with(|| candidate.amount_in_raw.cmp(&current.amount_in_raw))
        .is_gt()
}

fn normalize_size_bps(values: &[u32]) -> Vec<u32> {
    let mut normalized: Vec<u32> = values.iter().copied().filter(|value| *value > 0).collect();
    normalized.sort_unstable();
    normalized.dedup();
    normalized
}

fn insert_sorted_unique(values: &mut Vec<u32>, value: u32) {
    if value == 0 {
        return;
    }

    match values.binary_search(&value) {
        Ok(_) => {}
        Err(index) => values.insert(index, value),
    }
}

fn midpoint_between(lower: u32, upper: u32) -> Option<u32> {
    if upper <= lower + 1 {
        None
    } else {
        Some(lower + ((upper - lower) / 2))
    }
}

fn scale_amount_in(base_amount_in: u128, scale_bps: u32) -> u128 {
    base_amount_in.saturating_mul(u128::from(scale_bps)) / 10_000
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{PoolDef, TokenDef},
        graph::TrianglePath,
        state::{PoolState, ScannerState},
    };
    use ethers::types::{Address, U256};
    use std::{collections::BTreeMap, time::Instant};

    fn addr(raw: &str) -> Address {
        raw.parse().expect("invalid test address")
    }

    fn test_token(
        symbol: &'static str,
        address: Address,
        decimals: u8,
        quote_amount_in: u128,
    ) -> TokenDef {
        TokenDef {
            symbol,
            address,
            decimals,
            quote_amount_in,
            usd_price_hint: 1.0,
        }
    }

    fn test_pool(
        name: &'static str,
        address: Address,
        token0: Address,
        token1: Address,
    ) -> PoolState {
        PoolState {
            pool: PoolDef {
                name,
                address,
                token0,
                token1,
                fee: 500,
                reserve_usd_hint: 1_000_000.0,
            },
            sqrt_price_x96: U256::from_dec_str("79228162514264337593543950336").expect("q96"),
            tick: 0,
            tick_spacing: 10,
            nearest_initialized_lower_tick: Some(-10),
            nearest_initialized_upper_tick: Some(10),
            initialized_ticks: BTreeMap::from([
                (-10, 10_000_000_000_000_000),
                (10, -10_000_000_000_000_000),
            ]),
            liquidity: 10_000_000_000_000_000,
            last_updated_block: 0,
            last_updated_log_index: 0,
        }
    }

    fn test_pool_with_ticks(
        token0: Address,
        token1: Address,
        initialized_ticks: BTreeMap<i32, i128>,
        liquidity: u128,
    ) -> PoolState {
        PoolState {
            pool: PoolDef {
                name: "multi tick",
                address: addr("0x1000000000000000000000000000000000000010"),
                token0,
                token1,
                fee: 500,
                reserve_usd_hint: 1_000_000.0,
            },
            sqrt_price_x96: U256::from_dec_str("79228162514264337593543950336").expect("q96"),
            tick: 0,
            tick_spacing: 10,
            nearest_initialized_lower_tick: initialized_ticks
                .range(..=0)
                .next_back()
                .map(|(tick, _)| *tick),
            nearest_initialized_upper_tick: initialized_ticks
                .range(1..)
                .next()
                .map(|(tick, _)| *tick),
            initialized_ticks,
            liquidity,
            last_updated_block: 0,
            last_updated_log_index: 0,
        }
    }

    fn benchmark_fixture() -> (TrianglePath, HashMap<Address, TokenDef>, ScannerState) {
        let usdc = addr("0xaf88d065e77c8cC2239327C5EDb3A432268e5831");
        let weth = addr("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1");
        let usdt0 = addr("0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9");

        let triangle = TrianglePath {
            id: "test".to_string(),
            start_token: usdc,
            tokens: vec![usdc, weth, usdt0, usdc],
            pools: vec![
                addr("0x1000000000000000000000000000000000000001"),
                addr("0x1000000000000000000000000000000000000002"),
                addr("0x1000000000000000000000000000000000000003"),
            ],
            fees: vec![500, 500, 500],
        };

        let token_map = HashMap::from([
            (usdc, test_token("USDC", usdc, 6, 1_000_000_000)),
            (weth, test_token("WETH", weth, 18, 500_000_000_000_000_000)),
            (usdt0, test_token("USDT0", usdt0, 6, 1_000_000_000)),
        ]);
        let state = ScannerState {
            pools: HashMap::from([
                (
                    triangle.pools[0],
                    test_pool("pool0", triangle.pools[0], usdc, weth),
                ),
                (
                    triangle.pools[1],
                    test_pool("pool1", triangle.pools[1], weth, usdt0),
                ),
                (
                    triangle.pools[2],
                    test_pool("pool2", triangle.pools[2], usdt0, usdc),
                ),
            ]),
        };

        (triangle, token_map, state)
    }

    #[test]
    fn best_size_search_runs_bounded_ternary_refinement() {
        let (triangle, token_map, state) = benchmark_fixture();

        let result = find_best_local_size(&triangle, &state, &token_map, &[2500, 5000, 10_000])
            .expect("best quote");

        assert_eq!(result.size_bps, 316);
        assert_eq!(result.amount_in_raw, 31_600_000);
        assert!(result.gross_profit.is_finite());
        assert!(result.search_samples > 7);
        assert!(result.refinement_samples > 4);
    }

    #[test]
    fn duplicate_size_rounding_is_deduped() {
        let sizes = build_candidate_sizes(3, &[3333, 3334, 3334, 10_000]);
        assert_eq!(sizes, vec![(1, 3334), (3, 10_000)]);
    }

    #[test]
    fn refinement_points_cover_neighbors_and_boundaries() {
        assert_eq!(
            build_refinement_size_bps(&[2500, 5000, 10_000, 20_000, 40_000], 10_000),
            vec![7500, 15_000]
        );
        assert_eq!(
            build_refinement_size_bps(&[2500, 5000, 10_000, 20_000, 40_000], 2500),
            vec![1250, 3750]
        );
        assert_eq!(
            build_refinement_size_bps(&[2500, 5000, 10_000, 20_000, 40_000], 40_000),
            vec![30_000, 50_000]
        );
    }

    #[test]
    fn local_swap_simulation_crosses_multiple_initialized_ticks() {
        let token0 = addr("0x2000000000000000000000000000000000000001");
        let token1 = addr("0x2000000000000000000000000000000000000002");
        let pool = test_pool_with_ticks(
            token0,
            token1,
            BTreeMap::from([(-10, 1_000_000), (10, -500_000), (20, -500_000)]),
            1_000_000,
        );

        let result = pool
            .simulate_swap_raw(600.0, token1, token0)
            .expect("multi-tick quote");

        assert!(result.crosses_tick);
        assert!(result.amount_out > 0.0);
        assert!(result.headroom_ratio > 1.0);
    }

    #[test]
    #[ignore = "microbenchmark"]
    fn bench_local_size_search_latency() {
        let (triangle, token_map, state) = benchmark_fixture();
        let size_bps = [2500, 5000, 10_000, 20_000, 40_000];
        let iterations = 100_000usize;

        let start = Instant::now();
        for _ in 0..iterations {
            let result =
                find_best_local_size(&triangle, &state, &token_map, &size_bps).expect("best quote");
            std::hint::black_box(result);
        }
        let elapsed = start.elapsed();
        let avg_ns = elapsed.as_nanos() / iterations as u128;

        println!(
            "local_size_search iterations={} total_us={} avg_ns={}",
            iterations,
            elapsed.as_micros(),
            avg_ns
        );
    }
}
