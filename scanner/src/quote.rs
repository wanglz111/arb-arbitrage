use std::sync::Arc;

use anyhow::{Context, Result};
use ethers::{
    abi::{Abi, AbiParser},
    contract::Contract,
    types::{Address, U256},
};

use crate::{graph::TrianglePath, path::encode_triangle_path, state::RpcProvider};

pub const ARBITRUM_QUOTER_V2: &str = "0x61fFE014bA17989E743c5F6cB21bF9697530B21e";

#[derive(Clone, Debug)]
pub struct ExactQuoteResult {
    pub amount_in: U256,
    pub amount_out: U256,
    pub gas_estimate: U256,
}

pub struct QuoteEngine {
    contract: Contract<RpcProvider>,
}

impl QuoteEngine {
    pub fn new(provider: Arc<RpcProvider>) -> Result<Self> {
        let address: Address = ARBITRUM_QUOTER_V2
            .parse()
            .context("invalid Arbitrum QuoterV2 address")?;
        let contract = Contract::new(address, quoter_abi()?, provider);
        Ok(Self { contract })
    }

    pub async fn quote_triangle_amount(
        &self,
        triangle: &TrianglePath,
        amount_in: U256,
    ) -> Result<ExactQuoteResult> {
        let path = encode_triangle_path(triangle);

        let (amount_out, _, _, gas_estimate): (U256, Vec<U256>, Vec<u32>, U256) = self
            .contract
            .method::<_, (U256, Vec<U256>, Vec<u32>, U256)>("quoteExactInput", (path, amount_in))?
            .call()
            .await
            .context("quoteExactInput failed")?;

        Ok(ExactQuoteResult {
            amount_in,
            amount_out,
            gas_estimate,
        })
    }
}

fn quoter_abi() -> Result<Abi> {
    AbiParser::default()
        .parse(&["function quoteExactInput(bytes path, uint256 amountIn) returns (uint256 amountOut, uint160[] sqrtPriceX96AfterList, uint32[] initializedTicksCrossedList, uint256 gasEstimate)"])
        .context("failed to build QuoterV2 ABI")
}
