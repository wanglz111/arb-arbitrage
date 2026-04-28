use std::collections::{HashMap, HashSet};

use ethers::types::Address;

use crate::{
    config::{PoolDef, TokenDef},
    state::ScannerState,
};

const MIN_CYCLE_HOPS: usize = 3;
const MAX_CYCLE_HOPS: usize = 5;

#[derive(Clone, Debug)]
pub struct TrianglePath {
    pub id: String,
    pub start_token: Address,
    pub tokens: Vec<Address>,
    pub pools: Vec<Address>,
    pub fees: Vec<u32>,
}

#[derive(Clone, Debug)]
pub struct CanonicalTriangleView {
    pub dedupe_key: String,
}

#[derive(Clone, Debug)]
pub struct TriangleDisplayView {
    pub label: String,
    pub fee_label: String,
}

#[derive(Debug)]
pub struct TriangleGraph {
    pub triangles: Vec<TrianglePath>,
    pub pool_to_triangles: HashMap<Address, Vec<usize>>,
}

impl TriangleGraph {
    pub fn build(pools: &[PoolDef]) -> Self {
        let mut adjacency: HashMap<Address, Vec<&PoolDef>> = HashMap::new();
        for pool in pools {
            adjacency.entry(pool.token0).or_default().push(pool);
            adjacency.entry(pool.token1).or_default().push(pool);
        }

        let mut seen = HashSet::new();
        let mut triangles = Vec::new();

        for &start_token in adjacency.keys() {
            let mut tokens = vec![start_token];
            let mut route_pools = Vec::new();
            let mut route_fees = Vec::new();
            enumerate_cycles(
                start_token,
                start_token,
                &adjacency,
                &mut tokens,
                &mut route_pools,
                &mut route_fees,
                &mut seen,
                &mut triangles,
            );
        }

        let mut pool_to_triangles: HashMap<Address, Vec<usize>> = HashMap::new();
        for (idx, triangle) in triangles.iter().enumerate() {
            for &pool in &triangle.pools {
                pool_to_triangles.entry(pool).or_default().push(idx);
            }
        }

        Self {
            triangles,
            pool_to_triangles,
        }
    }

    pub fn affected_triangles(&self, pool: Address) -> impl Iterator<Item = &TrianglePath> {
        self.pool_to_triangles
            .get(&pool)
            .into_iter()
            .flat_map(|indexes| indexes.iter())
            .filter_map(|idx| self.triangles.get(*idx))
    }

    pub fn rough_cycle_edge_bps(
        &self,
        triangle: &TrianglePath,
        state: &ScannerState,
        token_map: &HashMap<Address, TokenDef>,
    ) -> Option<f64> {
        let mut gross = 1.0f64;
        for leg in triangle.legs() {
            let pool_state = state.pools.get(&leg.pool)?;
            let spot_rate = pool_state.spot_rate(leg.token_in, leg.token_out, token_map)?;
            let fee_factor = 1.0 - (leg.fee as f64 / 1_000_000.0);
            gross *= spot_rate * fee_factor;
        }

        Some((gross - 1.0) * 10_000.0)
    }
}

impl TrianglePath {
    pub fn legs(&self) -> impl Iterator<Item = RouteLeg> + '_ {
        self.pools
            .iter()
            .copied()
            .zip(self.fees.iter().copied())
            .enumerate()
            .map(|(idx, (pool, fee))| RouteLeg {
                token_in: self.tokens[idx],
                token_out: self.tokens[idx + 1],
                pool,
                fee,
            })
    }

    pub fn canonical_view(&self) -> CanonicalTriangleView {
        CanonicalTriangleView {
            dedupe_key: route_id(&self.tokens, &self.pools, &self.fees),
        }
    }

    pub fn display_view(&self, token_map: &HashMap<Address, TokenDef>) -> TriangleDisplayView {
        TriangleDisplayView {
            label: self
                .tokens
                .iter()
                .map(|token| token_symbol(token_map, *token))
                .collect::<Vec<_>>()
                .join("->"),
            fee_label: self
                .fees
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(" / "),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RouteLeg {
    pub token_in: Address,
    pub token_out: Address,
    pub pool: Address,
    pub fee: u32,
}

#[allow(clippy::too_many_arguments)]
fn enumerate_cycles(
    start_token: Address,
    current_token: Address,
    adjacency: &HashMap<Address, Vec<&PoolDef>>,
    tokens: &mut Vec<Address>,
    route_pools: &mut Vec<Address>,
    route_fees: &mut Vec<u32>,
    seen: &mut HashSet<String>,
    triangles: &mut Vec<TrianglePath>,
) {
    if route_pools.len() >= MAX_CYCLE_HOPS {
        return;
    }

    let Some(next_pools) = adjacency.get(&current_token) else {
        return;
    };

    for pool in next_pools {
        if route_pools.contains(&pool.address) {
            continue;
        }

        let next_token = other_token(pool, current_token);
        let next_hop_count = route_pools.len() + 1;
        let closes_cycle = next_token == start_token;

        if closes_cycle {
            if next_hop_count < MIN_CYCLE_HOPS {
                continue;
            }

            route_pools.push(pool.address);
            route_fees.push(pool.fee);
            tokens.push(start_token);

            let id = route_id(tokens, route_pools, route_fees);
            let candidate = TrianglePath {
                id: id.clone(),
                start_token,
                tokens: tokens.clone(),
                pools: route_pools.clone(),
                fees: route_fees.clone(),
            };
            let canonical = candidate.canonical_view();
            if seen.insert(canonical.dedupe_key) {
                triangles.push(candidate);
            }

            tokens.pop();
            route_fees.pop();
            route_pools.pop();
            continue;
        }

        if next_hop_count >= MAX_CYCLE_HOPS || tokens.contains(&next_token) {
            continue;
        }

        route_pools.push(pool.address);
        route_fees.push(pool.fee);
        tokens.push(next_token);
        enumerate_cycles(
            start_token,
            next_token,
            adjacency,
            tokens,
            route_pools,
            route_fees,
            seen,
            triangles,
        );
        tokens.pop();
        route_fees.pop();
        route_pools.pop();
    }
}

fn route_id(tokens: &[Address], pools: &[Address], fees: &[u32]) -> String {
    format!(
        "{}|{}|{}",
        tokens
            .iter()
            .map(|token| format!("{token:?}"))
            .collect::<Vec<_>>()
            .join(":"),
        pools
            .iter()
            .map(|pool| format!("{pool:?}"))
            .collect::<Vec<_>>()
            .join(":"),
        fees.iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(":")
    )
}

fn other_token(pool: &PoolDef, token: Address) -> Address {
    if pool.token0 == token {
        pool.token1
    } else {
        pool.token0
    }
}

fn token_symbol(token_map: &HashMap<Address, TokenDef>, address: Address) -> &'static str {
    token_map
        .get(&address)
        .map(|token| token.symbol)
        .unwrap_or("UNKNOWN")
}
