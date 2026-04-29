use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use ethers::{
    types::{Address, Bytes, U256},
    utils::{hex, id},
};

use crate::{graph::TrianglePath, path::encode_triangle_path};

const EXECUTE_SIGNATURE: &str = "execute(uint256,uint256,bytes)";
const EXECUTE_ROUTE_SIGNATURE: &str = "executeRoute(uint256,uint256,uint256)";
const ABI_WORD_BYTES: usize = 32;
const EXECUTE_SELECTOR_BYTES: usize = 4;
const EXECUTE_HEAD_BYTES: usize = ABI_WORD_BYTES * 3;
const AMOUNT_IN_OFFSET: usize = EXECUTE_SELECTOR_BYTES;
const MIN_AMOUNT_OUT_OFFSET: usize = AMOUNT_IN_OFFSET + ABI_WORD_BYTES;
const PATH_OFFSET_OFFSET: usize = MIN_AMOUNT_OUT_OFFSET + ABI_WORD_BYTES;
const PATH_LENGTH_OFFSET: usize = PATH_OFFSET_OFFSET + ABI_WORD_BYTES;
const PATH_BYTES_OFFSET: usize = PATH_LENGTH_OFFSET + ABI_WORD_BYTES;

#[derive(Clone, Debug)]
pub struct ExecutionPlan {
    pub route_id: U256,
    pub loan_token: Address,
    pub amount_in: U256,
    pub expected_amount_out: U256,
    pub amount_out_minimum: U256,
    pub path: Bytes,
    pub slippage_bps: u32,
    pub execute_calldata: Bytes,
    pub execute_route_calldata: Bytes,
}

impl ExecutionPlan {
    pub fn calldata_hex(&self) -> String {
        format!("0x{}", hex::encode(self.execute_calldata.as_ref()))
    }

    pub fn route_calldata_hex(&self) -> String {
        format!("0x{}", hex::encode(self.execute_route_calldata.as_ref()))
    }
}

#[derive(Clone, Debug)]
pub struct RouteCatalogEntry {
    pub route_id: U256,
    pub label: String,
    pub loan_token: Address,
    pub path: Bytes,
    pub set_route_calldata: Bytes,
}

impl RouteCatalogEntry {
    pub fn path_hex(&self) -> String {
        format!("0x{}", hex::encode(self.path.as_ref()))
    }

    pub fn set_route_calldata_hex(&self) -> String {
        format!("0x{}", hex::encode(self.set_route_calldata.as_ref()))
    }
}

#[derive(Clone, Debug)]
struct PreparedRoute {
    route_id: U256,
    loan_token: Address,
    path: Bytes,
    execute_calldata_template: Vec<u8>,
    execute_route_calldata_template: [u8; EXECUTE_SELECTOR_BYTES + (ABI_WORD_BYTES * 3)],
}

impl PreparedRoute {
    fn new(route_id: U256, triangle: &TrianglePath) -> Self {
        let path = encode_triangle_path(triangle);
        let padded_path_length = padded_path_length(path.len());
        let mut execute_calldata_template =
            vec![
                0u8;
                EXECUTE_SELECTOR_BYTES + EXECUTE_HEAD_BYTES + ABI_WORD_BYTES + padded_path_length
            ];
        execute_calldata_template[..EXECUTE_SELECTOR_BYTES]
            .copy_from_slice(&id(EXECUTE_SIGNATURE)[..EXECUTE_SELECTOR_BYTES]);
        write_u256_word(
            &mut execute_calldata_template[PATH_OFFSET_OFFSET..PATH_LENGTH_OFFSET],
            U256::from(EXECUTE_HEAD_BYTES),
        );
        write_u256_word(
            &mut execute_calldata_template[PATH_LENGTH_OFFSET..PATH_BYTES_OFFSET],
            U256::from(path.len()),
        );
        execute_calldata_template[PATH_BYTES_OFFSET..PATH_BYTES_OFFSET + path.len()]
            .copy_from_slice(path.as_ref());

        let mut execute_route_calldata_template =
            [0u8; EXECUTE_SELECTOR_BYTES + (ABI_WORD_BYTES * 3)];
        execute_route_calldata_template[..EXECUTE_SELECTOR_BYTES]
            .copy_from_slice(&id(EXECUTE_ROUTE_SIGNATURE)[..EXECUTE_SELECTOR_BYTES]);
        write_u256_word(
            &mut execute_route_calldata_template
                [EXECUTE_SELECTOR_BYTES..EXECUTE_SELECTOR_BYTES + ABI_WORD_BYTES],
            route_id,
        );

        Self {
            route_id,
            loan_token: triangle.start_token,
            path,
            execute_calldata_template,
            execute_route_calldata_template,
        }
    }

    fn build_execute_calldata(&self, amount_in: U256, amount_out_minimum: U256) -> Bytes {
        let mut calldata = self.execute_calldata_template.clone();
        write_u256_word(
            &mut calldata[AMOUNT_IN_OFFSET..MIN_AMOUNT_OUT_OFFSET],
            amount_in,
        );
        write_u256_word(
            &mut calldata[MIN_AMOUNT_OUT_OFFSET..PATH_OFFSET_OFFSET],
            amount_out_minimum,
        );
        Bytes::from(calldata)
    }

    fn build_execute_route_calldata(&self, amount_in: U256, amount_out_minimum: U256) -> Bytes {
        let mut calldata = self.execute_route_calldata_template;
        write_u256_word(
            &mut calldata[EXECUTE_SELECTOR_BYTES + ABI_WORD_BYTES
                ..EXECUTE_SELECTOR_BYTES + (ABI_WORD_BYTES * 2)],
            amount_in,
        );
        write_u256_word(
            &mut calldata[EXECUTE_SELECTOR_BYTES + (ABI_WORD_BYTES * 2)..],
            amount_out_minimum,
        );
        Bytes::from(calldata.to_vec())
    }
}

pub struct ExecutionBuilder {
    routes: HashMap<String, PreparedRoute>,
    slippage_bps: u32,
}

impl ExecutionBuilder {
    pub fn new(triangles: &[TrianglePath], slippage_bps: u32) -> Result<Self> {
        if slippage_bps > 10_000 {
            bail!("execution slippage bps must be <= 10000");
        }

        let routes = triangles
            .iter()
            .enumerate()
            .map(|(idx, triangle)| {
                (
                    triangle.id.clone(),
                    PreparedRoute::new(U256::from(idx.saturating_add(1)), triangle),
                )
            })
            .collect();

        Ok(Self {
            routes,
            slippage_bps,
        })
    }

    pub fn build_plan(
        &self,
        triangle: &TrianglePath,
        amount_in: U256,
        expected_amount_out: U256,
    ) -> Result<ExecutionPlan> {
        let route = self
            .routes
            .get(&triangle.id)
            .with_context(|| format!("missing prepared route for {}", triangle.id))?;
        let amount_out_minimum =
            apply_slippage_bps(expected_amount_out, self.slippage_bps).max(amount_in);

        Ok(ExecutionPlan {
            route_id: route.route_id,
            loan_token: route.loan_token,
            amount_in,
            expected_amount_out,
            amount_out_minimum,
            path: route.path.clone(),
            slippage_bps: self.slippage_bps,
            execute_calldata: route.build_execute_calldata(amount_in, amount_out_minimum),
            execute_route_calldata: route
                .build_execute_route_calldata(amount_in, amount_out_minimum),
        })
    }

    pub fn route_catalog(&self, triangles: &[TrianglePath]) -> Result<Vec<RouteCatalogEntry>> {
        triangles
            .iter()
            .map(|triangle| {
                let route = self
                    .routes
                    .get(&triangle.id)
                    .with_context(|| format!("missing prepared route for {}", triangle.id))?;
                Ok(RouteCatalogEntry {
                    route_id: route.route_id,
                    label: triangle.id.clone(),
                    loan_token: route.loan_token,
                    path: route.path.clone(),
                    set_route_calldata: build_set_route_calldata(route.route_id, &route.path),
                })
            })
            .collect()
    }
}

fn build_set_route_calldata(route_id: U256, path: &Bytes) -> Bytes {
    const SET_ROUTE_SIGNATURE: &str = "setRoute(uint256,bytes)";
    let padded_path_length = padded_path_length(path.len());
    let mut calldata =
        vec![
            0u8;
            EXECUTE_SELECTOR_BYTES + (ABI_WORD_BYTES * 2) + ABI_WORD_BYTES + padded_path_length
        ];
    calldata[..EXECUTE_SELECTOR_BYTES]
        .copy_from_slice(&id(SET_ROUTE_SIGNATURE)[..EXECUTE_SELECTOR_BYTES]);
    write_u256_word(
        &mut calldata[EXECUTE_SELECTOR_BYTES..EXECUTE_SELECTOR_BYTES + ABI_WORD_BYTES],
        route_id,
    );
    write_u256_word(
        &mut calldata[EXECUTE_SELECTOR_BYTES + ABI_WORD_BYTES
            ..EXECUTE_SELECTOR_BYTES + (ABI_WORD_BYTES * 2)],
        U256::from(ABI_WORD_BYTES * 2),
    );
    write_u256_word(
        &mut calldata[EXECUTE_SELECTOR_BYTES + (ABI_WORD_BYTES * 2)
            ..EXECUTE_SELECTOR_BYTES + (ABI_WORD_BYTES * 3)],
        U256::from(path.len()),
    );
    calldata[EXECUTE_SELECTOR_BYTES + (ABI_WORD_BYTES * 3)
        ..EXECUTE_SELECTOR_BYTES + (ABI_WORD_BYTES * 3) + path.len()]
        .copy_from_slice(path.as_ref());
    Bytes::from(calldata)
}

fn apply_slippage_bps(amount_out: U256, slippage_bps: u32) -> U256 {
    if slippage_bps == 0 {
        return amount_out;
    }

    let kept_bps = 10_000u32.saturating_sub(slippage_bps);
    amount_out * U256::from(kept_bps) / U256::from(10_000u32)
}

fn padded_path_length(path_length: usize) -> usize {
    path_length.div_ceil(ABI_WORD_BYTES) * ABI_WORD_BYTES
}

fn write_u256_word(slot: &mut [u8], value: U256) {
    debug_assert_eq!(slot.len(), ABI_WORD_BYTES);
    value.to_big_endian(slot);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ethers::abi::{AbiParser, Token};

    fn addr(raw: &str) -> Address {
        raw.parse().expect("invalid test address")
    }

    fn route(tokens: Vec<Address>, fees: Vec<u32>) -> TrianglePath {
        TrianglePath {
            id: "test".to_string(),
            start_token: tokens[0],
            pools: vec![Address::zero(); fees.len()],
            tokens,
            fees,
        }
    }

    fn triangle() -> TrianglePath {
        route(
            vec![
                addr("0xaf88d065e77c8cC2239327C5EDb3A432268e5831"),
                addr("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1"),
                addr("0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9"),
                addr("0xaf88d065e77c8cC2239327C5EDb3A432268e5831"),
            ],
            vec![500, 500, 100],
        )
    }

    #[test]
    fn manual_execute_encoding_matches_abi_encoding() {
        let builder = ExecutionBuilder::new(&[triangle()], 25).expect("builder");
        let triangle = triangle();
        let amount_in = U256::from(1_000_000_000u64);
        let expected_amount_out = U256::from(1_020_000_000u64);
        let plan = builder
            .build_plan(&triangle, amount_in, expected_amount_out)
            .expect("plan");

        let function = AbiParser::default()
            .parse_function(
                "function execute(uint256 loanAmount, uint256 amountOutMinimum, bytes path)",
            )
            .expect("function");
        let expected = function
            .encode_input(&[
                Token::Uint(amount_in),
                Token::Uint(plan.amount_out_minimum),
                Token::Bytes(plan.path.to_vec()),
            ])
            .expect("calldata");

        assert_eq!(plan.execute_calldata.as_ref(), expected.as_slice());
        assert_eq!(plan.loan_token, triangle.start_token);
        assert_eq!(plan.amount_out_minimum, U256::from(1_017_450_000u64));
    }

    #[test]
    fn route_calldata_matches_abi_encoding() {
        let builder = ExecutionBuilder::new(&[triangle()], 25).expect("builder");
        let triangle = triangle();
        let amount_in = U256::from(1_000_000_000u64);
        let expected_amount_out = U256::from(1_020_000_000u64);
        let plan = builder
            .build_plan(&triangle, amount_in, expected_amount_out)
            .expect("plan");

        let function = AbiParser::default()
            .parse_function("function executeRoute(uint256 routeId, uint256 loanAmount, uint256 amountOutMinimum)")
            .expect("function");
        let expected = function
            .encode_input(&[
                Token::Uint(plan.route_id),
                Token::Uint(amount_in),
                Token::Uint(plan.amount_out_minimum),
            ])
            .expect("calldata");

        assert_eq!(plan.execute_route_calldata.as_ref(), expected.as_slice());
    }

    #[test]
    fn supports_four_hop_direct_execute_calldata() {
        let four_hop = route(
            vec![
                addr("0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f"),
                addr("0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9"),
                addr("0xaf88d065e77c8cC2239327C5EDb3A432268e5831"),
                addr("0xcbB7C0000aB88B473b1f5aFd9ef808440eed33Bf"),
                addr("0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f"),
            ],
            vec![500, 100, 100, 500],
        );
        let builder = ExecutionBuilder::new(std::slice::from_ref(&four_hop), 0).expect("builder");
        let plan = builder
            .build_plan(&four_hop, U256::from(1u64), U256::from(2u64))
            .expect("plan");

        assert_eq!(plan.path.len(), 112);
        assert!(plan.execute_calldata.len() > plan.execute_route_calldata.len());
    }

    #[test]
    fn amount_out_minimum_never_drops_below_flash_repayment() {
        let builder = ExecutionBuilder::new(&[triangle()], 25).expect("builder");
        let amount_in = U256::from(1_000_000_000u64);
        let plan = builder
            .build_plan(&triangle(), amount_in, U256::from(1_001_250_000u64))
            .expect("plan");

        assert_eq!(plan.amount_out_minimum, amount_in);
    }
}
