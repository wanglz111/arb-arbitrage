use std::collections::{HashMap, HashSet};

use ethers::types::Address;

use crate::{
    config::{PoolDef, TokenDef},
    state::ScannerState,
};

#[derive(Clone, Debug)]
pub struct TrianglePath {
    pub id: String,
    pub start_token: Address,
    pub middle_token_1: Address,
    pub middle_token_2: Address,
    pub pools: [Address; 3],
    pub fees: [u32; 3],
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
        let mut seen = HashSet::new();
        let mut triangles = Vec::new();

        for first in pools {
            for &start_token in &[first.token0, first.token1] {
                let middle_token_1 = other_token(first, start_token);

                for second in pools.iter().filter(|pool| {
                    pool.address != first.address
                        && (pool.token0 == middle_token_1 || pool.token1 == middle_token_1)
                }) {
                    let middle_token_2 = other_token(second, middle_token_1);
                    if middle_token_2 == start_token {
                        continue;
                    }

                    for third in pools.iter().filter(|pool| {
                        pool.address != first.address
                            && pool.address != second.address
                            && ((pool.token0 == middle_token_2 && pool.token1 == start_token)
                                || (pool.token1 == middle_token_2 && pool.token0 == start_token))
                    }) {
                        let id = format!(
                            "{start_token:?}:{middle_token_1:?}:{middle_token_2:?}:{}:{}:{}",
                            first.address, second.address, third.address
                        );
                        if !seen.insert(id.clone()) {
                            continue;
                        }

                        triangles.push(TrianglePath {
                            id,
                            start_token,
                            middle_token_1,
                            middle_token_2,
                            pools: [first.address, second.address, third.address],
                            fees: [first.fee, second.fee, third.fee],
                        });
                    }
                }
            }
        }

        let mut pool_to_triangles: HashMap<Address, Vec<usize>> = HashMap::new();
        for (idx, triangle) in triangles.iter().enumerate() {
            for pool in triangle.pools {
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
        let legs = [
            (
                triangle.start_token,
                triangle.middle_token_1,
                triangle.pools[0],
                triangle.fees[0],
            ),
            (
                triangle.middle_token_1,
                triangle.middle_token_2,
                triangle.pools[1],
                triangle.fees[1],
            ),
            (
                triangle.middle_token_2,
                triangle.start_token,
                triangle.pools[2],
                triangle.fees[2],
            ),
        ];

        let mut gross = 1.0f64;
        for (token_in, token_out, pool_address, fee) in legs {
            let pool_state = state.pools.get(&pool_address)?;
            let spot_rate = pool_state.spot_rate(token_in, token_out, token_map)?;
            let fee_factor = 1.0 - (fee as f64 / 1_000_000.0);
            gross *= spot_rate * fee_factor;
        }

        Some((gross - 1.0) * 10_000.0)
    }
}

impl TrianglePath {
    pub fn canonical_view(&self) -> CanonicalTriangleView {
        let tokens = [self.start_token, self.middle_token_1, self.middle_token_2];
        let pools = self.pools.map(|pool| format!("{pool:?}"));

        let mut best_key = String::new();

        for offset in 0..3 {
            let rotated_tokens = [
                tokens[offset % 3],
                tokens[(offset + 1) % 3],
                tokens[(offset + 2) % 3],
            ];
            let rotated_fees = [
                self.fees[offset % 3],
                self.fees[(offset + 1) % 3],
                self.fees[(offset + 2) % 3],
            ];

            let key = format!(
                "{:?}:{:?}:{:?}|{}:{}:{}|{}:{}:{}",
                rotated_tokens[0],
                rotated_tokens[1],
                rotated_tokens[2],
                pools[offset % 3],
                pools[(offset + 1) % 3],
                pools[(offset + 2) % 3],
                rotated_fees[0],
                rotated_fees[1],
                rotated_fees[2],
            );

            if best_key.is_empty() || key < best_key {
                best_key = key;
            }
        }

        CanonicalTriangleView {
            dedupe_key: best_key,
        }
    }

    pub fn display_view(&self, token_map: &HashMap<Address, TokenDef>) -> TriangleDisplayView {
        TriangleDisplayView {
            label: format!(
                "{}->{}->{}->{}",
                token_symbol(token_map, self.start_token),
                token_symbol(token_map, self.middle_token_1),
                token_symbol(token_map, self.middle_token_2),
                token_symbol(token_map, self.start_token),
            ),
            fee_label: format!("{} / {} / {} bps", self.fees[0], self.fees[1], self.fees[2]),
        }
    }
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
