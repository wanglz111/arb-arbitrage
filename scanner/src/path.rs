use ethers::types::{Address, Bytes};

use crate::graph::TrianglePath;

pub fn encode_triangle_path(triangle: &TrianglePath) -> Bytes {
    encode_v3_path(&triangle.tokens, &triangle.fees)
}

pub fn encode_v3_path(tokens: &[Address], fees: &[u32]) -> Bytes {
    assert_eq!(tokens.len(), fees.len() + 1, "invalid v3 path shape");

    let mut encoded = Vec::with_capacity(v3_path_bytes_length(fees.len()));

    for (token, fee) in tokens.iter().zip(fees.iter()) {
        encoded.extend_from_slice(token.as_bytes());
        encoded.extend_from_slice(&fee_to_bytes(*fee));
    }

    encoded.extend_from_slice(tokens[tokens.len() - 1].as_bytes());
    Bytes::from(encoded)
}

pub fn v3_path_bytes_length(hops: usize) -> usize {
    20 + (hops * 23)
}

fn fee_to_bytes(fee: u32) -> [u8; 3] {
    let bytes = fee.to_be_bytes();
    [bytes[1], bytes[2], bytes[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(raw: &str) -> Address {
        raw.parse().expect("invalid test address")
    }

    #[test]
    fn encodes_triangle_path_in_uniswap_v3_packed_format() {
        let usdc = addr("0xaf88d065e77c8cC2239327C5EDb3A432268e5831");
        let weth = addr("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1");
        let wbtc = addr("0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f");
        let encoded = encode_v3_path(&[usdc, weth, wbtc, usdc], &[500, 500, 500]);

        assert_eq!(encoded.len(), v3_path_bytes_length(3));
        assert_eq!(&encoded.as_ref()[20..23], &[0x00, 0x01, 0xf4]);
        assert_eq!(&encoded.as_ref()[43..46], &[0x00, 0x01, 0xf4]);
        assert_eq!(&encoded.as_ref()[66..69], &[0x00, 0x01, 0xf4]);
        assert_eq!(&encoded.as_ref()[69..89], usdc.as_bytes());
    }

    #[test]
    fn encodes_four_hop_path() {
        let wbtc = addr("0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f");
        let usdt0 = addr("0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9");
        let usdc = addr("0xaf88d065e77c8cC2239327C5EDb3A432268e5831");
        let cbbtc = addr("0xcbB7C0000aB88B473b1f5aFd9ef808440eed33Bf");

        let encoded = encode_v3_path(&[wbtc, usdt0, usdc, cbbtc, wbtc], &[500, 100, 100, 500]);

        assert_eq!(encoded.len(), v3_path_bytes_length(4));
        assert_eq!(&encoded.as_ref()[92..112], wbtc.as_bytes());
    }
}
