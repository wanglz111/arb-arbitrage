use ethers::types::Bytes;

use crate::graph::TrianglePath;

pub const TRIANGLE_PATH_BYTES_LENGTH: usize = (20 * 4) + (3 * 3);

pub fn encode_triangle_path(triangle: &TrianglePath) -> Bytes {
    let tokens = [
        triangle.start_token,
        triangle.middle_token_1,
        triangle.middle_token_2,
        triangle.start_token,
    ];

    let mut encoded = Vec::with_capacity(TRIANGLE_PATH_BYTES_LENGTH);

    for (token, fee) in tokens.iter().zip(triangle.fees.iter()) {
        encoded.extend_from_slice(token.as_bytes());
        encoded.extend_from_slice(&fee_to_bytes(*fee));
    }

    encoded.extend_from_slice(tokens[tokens.len() - 1].as_bytes());
    Bytes::from(encoded)
}

fn fee_to_bytes(fee: u32) -> [u8; 3] {
    let bytes = fee.to_be_bytes();
    [bytes[1], bytes[2], bytes[3]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::TrianglePath;
    use ethers::types::Address;

    fn addr(raw: &str) -> Address {
        raw.parse().expect("invalid test address")
    }

    #[test]
    fn encodes_triangle_path_in_uniswap_v3_packed_format() {
        let triangle = TrianglePath {
            id: "test".to_string(),
            start_token: addr("0xaf88d065e77c8cC2239327C5EDb3A432268e5831"),
            middle_token_1: addr("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1"),
            middle_token_2: addr("0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f"),
            pools: [Address::zero(); 3],
            fees: [500, 500, 500],
        };

        let encoded = encode_triangle_path(&triangle);

        assert_eq!(encoded.len(), TRIANGLE_PATH_BYTES_LENGTH);
        assert_eq!(&encoded.as_ref()[20..23], &[0x00, 0x01, 0xf4]);
        assert_eq!(&encoded.as_ref()[43..46], &[0x00, 0x01, 0xf4]);
        assert_eq!(&encoded.as_ref()[66..69], &[0x00, 0x01, 0xf4]);
        assert_eq!(&encoded.as_ref()[69..89], triangle.start_token.as_bytes(),);
    }
}
