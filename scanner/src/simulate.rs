use std::collections::{BTreeMap, BTreeSet, HashMap};

use ethers::types::{Address, U256};
use num_bigint::BigUint;
use num_traits::{One, Zero};

use crate::{
    config::TokenDef,
    graph::TrianglePath,
    state::{PoolState, ScannerState},
};

const LOCAL_REFINE_ROUNDS: usize = 2;
const LOCAL_TERNARY_ROUNDS: usize = 12;
const MAX_TICK_CROSSES_PER_LEG: usize = 256;
const Q96: u128 = 1u128 << 96;
const FEE_DENOMINATOR: u32 = 1_000_000;
const MIN_TICK: i32 = -887272;
const MAX_TICK: i32 = 887272;

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
    pub amount_out_raw: u128,
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

    let mut amount_raw = amount_in_raw;
    let mut crossed_tick_legs = 0u8;
    let mut max_headroom_ratio = 0.0f64;
    for leg in triangle.legs() {
        let pool_state = state.pools.get(&leg.pool)?;
        let leg_result = pool_state.simulate_swap_raw(amount_raw, leg.token_in, leg.token_out)?;
        if leg_result.crosses_tick {
            crossed_tick_legs = crossed_tick_legs.saturating_add(1);
        }
        max_headroom_ratio = max_headroom_ratio.max(leg_result.headroom_ratio);
        amount_raw = leg_result.amount_out_raw;
        if amount_raw == 0 {
            return None;
        }
    }

    if amount_raw == 0 {
        return None;
    }
    let amount_out = amount_raw as f64;

    Some(LocalQuoteResult {
        amount_in_raw,
        amount_out_raw_floor: amount_raw,
        amount_in,
        amount_out,
        gross_profit: amount_out - amount_in,
        edge_bps: ((amount_out / amount_in) - 1.0) * 10_000.0,
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
        amount_in_raw: u128,
        token_in: Address,
        token_out: Address,
    ) -> Option<LocalLegResult> {
        if amount_in_raw == 0 || self.liquidity == 0 || self.sqrt_price_x96.is_zero() {
            return None;
        }

        if token_in == self.pool.token0 && token_out == self.pool.token1 {
            return simulate_exact_input(self, amount_in_raw, true);
        }

        if token_in == self.pool.token1 && token_out == self.pool.token0 {
            return simulate_exact_input(self, amount_in_raw, false);
        }

        None
    }
}

fn simulate_exact_input(
    pool: &PoolState,
    amount_in_raw: u128,
    zero_for_one: bool,
) -> Option<LocalLegResult> {
    let mut amount_remaining = U256::from(amount_in_raw);
    let mut amount_out = U256::zero();
    let mut sqrt_price = pool.sqrt_price_x96;
    let mut liquidity = pool.liquidity;
    let mut current_tick = pool.tick;
    let mut crossed_ticks = 0usize;
    let mut max_headroom_ratio = 0.0f64;

    for _ in 0..=MAX_TICK_CROSSES_PER_LEG {
        if amount_remaining.is_zero() {
            break;
        }

        let (sqrt_target, initialized_tick) =
            next_sqrt_price_target(pool, current_tick, zero_for_one, sqrt_price)?;
        let step = compute_swap_step(
            sqrt_price,
            sqrt_target,
            liquidity,
            amount_remaining,
            pool.pool.fee,
        )?;
        let max_amount_in = step.amount_in.saturating_add(step.fee_amount);
        max_headroom_ratio =
            max_headroom_ratio.max(headroom_ratio_u256(amount_remaining, max_amount_in));

        sqrt_price = step.sqrt_price_next;
        amount_remaining = amount_remaining.checked_sub(max_amount_in)?;
        amount_out = amount_out.checked_add(step.amount_out)?;

        if sqrt_price == sqrt_target {
            if let Some(next_tick) = initialized_tick {
                crossed_ticks += 1;
                let liquidity_net = pool.initialized_ticks.get(&next_tick).copied()?;
                liquidity = cross_liquidity(liquidity, liquidity_net, zero_for_one)?;
                if liquidity == 0 {
                    return None;
                }
                current_tick = if zero_for_one {
                    next_tick.saturating_sub(1)
                } else {
                    next_tick
                };
            } else if amount_remaining > U256::zero() {
                return None;
            }
        } else {
            current_tick = get_tick_at_sqrt_ratio(sqrt_price)?;
        }
    }

    if !amount_remaining.is_zero() {
        return None;
    }

    if amount_out > U256::from(u128::MAX) {
        return None;
    }
    let amount_out_raw = amount_out.as_u128();
    Some(LocalLegResult {
        amount_out_raw,
        crosses_tick: crossed_ticks > 0,
        headroom_ratio: max_headroom_ratio,
    })
}

fn next_sqrt_price_target(
    pool: &PoolState,
    current_tick: i32,
    zero_for_one: bool,
    sqrt_price: U256,
) -> Option<(U256, Option<i32>)> {
    if zero_for_one {
        if let Some(next_tick) = pool.next_initialized_tick_below(current_tick) {
            return Some((get_sqrt_ratio_at_tick(next_tick)?, Some(next_tick)));
        }
        let fallback_tick = current_tick_lower(current_tick, pool.tick_spacing);
        let fallback = get_sqrt_ratio_at_tick(fallback_tick.max(MIN_TICK))?;
        return (fallback < sqrt_price).then_some((fallback, None));
    }

    if let Some(next_tick) = pool.next_initialized_tick_above(current_tick) {
        return Some((get_sqrt_ratio_at_tick(next_tick)?, Some(next_tick)));
    }
    let fallback_tick = current_tick_lower(current_tick, pool.tick_spacing) + pool.tick_spacing;
    let fallback = get_sqrt_ratio_at_tick(fallback_tick.min(MAX_TICK))?;
    (fallback > sqrt_price).then_some((fallback, None))
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

fn cross_liquidity(liquidity: u128, liquidity_net: i128, zero_for_one: bool) -> Option<u128> {
    let delta = if zero_for_one {
        liquidity_net.checked_neg()?
    } else {
        liquidity_net
    };

    if delta >= 0 {
        liquidity.checked_add(delta as u128)
    } else {
        liquidity.checked_sub((-delta) as u128)
    }
}

fn current_tick_lower(current_tick: i32, tick_spacing: i32) -> i32 {
    let spacing = tick_spacing.max(1);
    current_tick.div_euclid(spacing) * spacing
}

fn headroom_ratio_u256(amount_remaining: U256, max_amount_in: U256) -> f64 {
    if !max_amount_in.is_zero() {
        let remaining = amount_remaining
            .to_string()
            .parse::<f64>()
            .unwrap_or(f64::INFINITY);
        let max_in = max_amount_in
            .to_string()
            .parse::<f64>()
            .unwrap_or(f64::INFINITY);
        remaining / max_in
    } else {
        f64::INFINITY
    }
}

#[derive(Clone, Debug)]
struct SwapStep {
    sqrt_price_next: U256,
    amount_in: U256,
    amount_out: U256,
    fee_amount: U256,
}

fn compute_swap_step(
    sqrt_price_current: U256,
    sqrt_price_target: U256,
    liquidity: u128,
    amount_remaining: U256,
    fee_pips: u32,
) -> Option<SwapStep> {
    let zero_for_one = sqrt_price_current >= sqrt_price_target;
    let fee_complement = FEE_DENOMINATOR.checked_sub(fee_pips)?;
    let amount_remaining_less_fee = mul_div(
        amount_remaining,
        U256::from(fee_complement),
        U256::from(FEE_DENOMINATOR),
    )?;
    let amount_in_to_target = if zero_for_one {
        get_amount0_delta(sqrt_price_target, sqrt_price_current, liquidity, true)?
    } else {
        get_amount1_delta(sqrt_price_current, sqrt_price_target, liquidity, true)?
    };

    let sqrt_price_next = if amount_remaining_less_fee >= amount_in_to_target {
        sqrt_price_target
    } else {
        get_next_sqrt_price_from_input(
            sqrt_price_current,
            liquidity,
            amount_remaining_less_fee,
            zero_for_one,
        )?
    };
    let reached_target = sqrt_price_next == sqrt_price_target;

    let amount_in = if reached_target {
        amount_in_to_target
    } else if zero_for_one {
        get_amount0_delta(sqrt_price_next, sqrt_price_current, liquidity, true)?
    } else {
        get_amount1_delta(sqrt_price_current, sqrt_price_next, liquidity, true)?
    };
    let amount_out = if zero_for_one {
        get_amount1_delta(sqrt_price_next, sqrt_price_current, liquidity, false)?
    } else {
        get_amount0_delta(sqrt_price_current, sqrt_price_next, liquidity, false)?
    };
    let fee_amount = if reached_target {
        mul_div_rounding_up(amount_in, U256::from(fee_pips), U256::from(fee_complement))?
    } else {
        amount_remaining.checked_sub(amount_in)?
    };

    Some(SwapStep {
        sqrt_price_next,
        amount_in,
        amount_out,
        fee_amount,
    })
}

fn get_next_sqrt_price_from_input(
    sqrt_price_x96: U256,
    liquidity: u128,
    amount_in: U256,
    zero_for_one: bool,
) -> Option<U256> {
    if zero_for_one {
        get_next_sqrt_price_from_amount0_rounding_up(sqrt_price_x96, liquidity, amount_in)
    } else {
        let quotient = mul_div(amount_in, U256::from(Q96), U256::from(liquidity))?;
        sqrt_price_x96.checked_add(quotient)
    }
}

fn get_next_sqrt_price_from_amount0_rounding_up(
    sqrt_price_x96: U256,
    liquidity: u128,
    amount: U256,
) -> Option<U256> {
    if amount.is_zero() {
        return Some(sqrt_price_x96);
    }

    let numerator1 = U256::from(liquidity) << 96;
    let product = checked_mul_big(amount, sqrt_price_x96);
    let denominator = checked_add_big(u256_to_big(numerator1), product);
    big_to_u256_roundtrip(mul_div_rounding_up_big(
        u256_to_big(numerator1),
        u256_to_big(sqrt_price_x96),
        denominator,
    )?)
}

fn get_amount0_delta(
    sqrt_ratio_a_x96: U256,
    sqrt_ratio_b_x96: U256,
    liquidity: u128,
    round_up: bool,
) -> Option<U256> {
    let (lower, upper) = ordered_ratios(sqrt_ratio_a_x96, sqrt_ratio_b_x96);
    if lower.is_zero() {
        return None;
    }
    let numerator1 = u256_to_big(U256::from(liquidity) << 96);
    let numerator2 = u256_to_big(upper.checked_sub(lower)?);
    let upper_big = u256_to_big(upper);
    let lower_big = u256_to_big(lower);

    let value = if round_up {
        div_rounding_up_big(
            mul_div_rounding_up_big(numerator1, numerator2, upper_big)?,
            lower_big,
        )
    } else {
        (numerator1 * numerator2 / upper_big) / lower_big
    };
    big_to_u256_roundtrip(value)
}

fn get_amount1_delta(
    sqrt_ratio_a_x96: U256,
    sqrt_ratio_b_x96: U256,
    liquidity: u128,
    round_up: bool,
) -> Option<U256> {
    let (lower, upper) = ordered_ratios(sqrt_ratio_a_x96, sqrt_ratio_b_x96);
    let delta = upper.checked_sub(lower)?;
    if round_up {
        mul_div_rounding_up(U256::from(liquidity), delta, U256::from(Q96))
    } else {
        mul_div(U256::from(liquidity), delta, U256::from(Q96))
    }
}

fn ordered_ratios(a: U256, b: U256) -> (U256, U256) {
    if a > b { (b, a) } else { (a, b) }
}

fn mul_div(a: U256, b: U256, denominator: U256) -> Option<U256> {
    if denominator.is_zero() {
        return None;
    }
    big_to_u256_roundtrip(u256_to_big(a) * u256_to_big(b) / u256_to_big(denominator))
}

fn mul_div_rounding_up(a: U256, b: U256, denominator: U256) -> Option<U256> {
    if denominator.is_zero() {
        return None;
    }
    big_to_u256_roundtrip(mul_div_rounding_up_big(
        u256_to_big(a),
        u256_to_big(b),
        u256_to_big(denominator),
    )?)
}

fn mul_div_rounding_up_big(a: BigUint, b: BigUint, denominator: BigUint) -> Option<BigUint> {
    if denominator.is_zero() {
        return None;
    }
    let product = a * b;
    let quotient = &product / &denominator;
    let remainder = product % denominator;
    Some(if remainder.is_zero() {
        quotient
    } else {
        quotient + BigUint::one()
    })
}

fn div_rounding_up_big(numerator: BigUint, denominator: BigUint) -> BigUint {
    let quotient = &numerator / &denominator;
    let remainder = numerator % denominator;
    if remainder.is_zero() {
        quotient
    } else {
        quotient + BigUint::one()
    }
}

fn checked_mul_big(a: U256, b: U256) -> BigUint {
    u256_to_big(a) * u256_to_big(b)
}

fn checked_add_big(a: BigUint, b: BigUint) -> BigUint {
    a + b
}

fn u256_to_big(value: U256) -> BigUint {
    let mut bytes = [0u8; 32];
    value.to_big_endian(&mut bytes);
    BigUint::from_bytes_be(&bytes)
}

fn big_to_u256_roundtrip(value: BigUint) -> Option<U256> {
    if value.bits() > 256 {
        return None;
    }
    let bytes = value.to_bytes_be();
    Some(U256::from_big_endian(&bytes))
}

fn get_sqrt_ratio_at_tick(tick: i32) -> Option<U256> {
    if !(MIN_TICK..=MAX_TICK).contains(&tick) {
        return None;
    }

    let abs_tick = tick.unsigned_abs();
    let mut ratio = if (abs_tick & 0x1) != 0 {
        hex_u256("fffcb933bd6fad37aa2d162d1a594001")?
    } else {
        hex_u256("100000000000000000000000000000000")?
    };

    const FACTORS: &[(u32, &str)] = &[
        (0x2, "fff97272373d413259a46990580e213a"),
        (0x4, "fff2e50f5f656932ef12357cf3c7fdcc"),
        (0x8, "ffe5caca7e10e4e61c3624eaa0941cd0"),
        (0x10, "ffcb9843d60f6159c9db58835c926644"),
        (0x20, "ff973b41fa98c081472e6896dfb254c0"),
        (0x40, "ff2ea16466c96a3843ec78b326b52861"),
        (0x80, "fe5dee046a99a2a811c461f1969c3053"),
        (0x100, "fcbe86c7900a88aedcffc83b479aa3a4"),
        (0x200, "f987a7253ac413176f2b074cf7815e54"),
        (0x400, "f3392b0822b70005940c7a398e4b70f3"),
        (0x800, "e7159475a2c29b7443b29c7fa6e889d9"),
        (0x1000, "d097f3bdfd2022b8845ad8f792aa5825"),
        (0x2000, "a9f746462d870fdf8a65dc1f90e061e5"),
        (0x4000, "70d869a156d2a1b890bb3df62baf32f7"),
        (0x8000, "31be135f97d08fd981231505542fcfa6"),
        (0x10000, "9aa508b5b7a84e1c677de54f3e99bc9"),
        (0x20000, "5d6af8dedb81196699c329225ee604"),
        (0x40000, "2216e584f5fa1ea926041bedfe98"),
        (0x80000, "48a170391f7dc42444e8fa2"),
    ];

    for (mask, factor) in FACTORS {
        if (abs_tick & mask) != 0 {
            ratio = (ratio.checked_mul(hex_u256(factor)?)?) >> 128;
        }
    }

    if tick > 0 {
        ratio = U256::MAX / ratio;
    }

    let shifted = ratio >> 32;
    let rounded = if ratio & U256::from((1u128 << 32) - 1) == U256::zero() {
        shifted
    } else {
        shifted.checked_add(U256::one())?
    };
    Some(rounded)
}

fn get_tick_at_sqrt_ratio(sqrt_price_x96: U256) -> Option<i32> {
    let mut low = MIN_TICK;
    let mut high = MAX_TICK;
    while low < high {
        let mid = high - ((high - low) / 2);
        if get_sqrt_ratio_at_tick(mid)? <= sqrt_price_x96 {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    Some(low)
}

fn hex_u256(raw: &str) -> Option<U256> {
    U256::from_str_radix(raw, 16).ok()
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
            .simulate_swap_raw(600, token1, token0)
            .expect("multi-tick quote");

        assert!(result.crosses_tick);
        assert!(result.amount_out_raw > 0);
        assert!(result.headroom_ratio > 1.0);
    }

    #[test]
    fn tick_math_matches_uniswap_v3_boundaries() {
        assert_eq!(
            get_sqrt_ratio_at_tick(0).expect("tick 0"),
            U256::from_dec_str("79228162514264337593543950336").expect("q96")
        );
        assert_eq!(
            get_sqrt_ratio_at_tick(MIN_TICK).expect("min tick"),
            U256::from(4_295_128_739u64)
        );
        assert_eq!(
            get_sqrt_ratio_at_tick(MAX_TICK).expect("max tick"),
            U256::from_dec_str("1461446703485210103287273052203988822378723970342")
                .expect("max sqrt")
        );
    }

    #[test]
    fn full_math_rounds_like_solidity_helpers() {
        assert_eq!(
            mul_div(U256::from(10u64), U256::from(10u64), U256::from(6u64)).expect("mul div"),
            U256::from(16u64)
        );
        assert_eq!(
            mul_div_rounding_up(U256::from(10u64), U256::from(10u64), U256::from(6u64))
                .expect("mul div up"),
            U256::from(17u64)
        );
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
