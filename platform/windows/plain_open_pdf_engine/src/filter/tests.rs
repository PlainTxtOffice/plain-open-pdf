//! Decoder checks against streams produced by an independent zlib encoder.

use super::{FilterError, Predictor, inflate_zlib, unpredict};

fn hex(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text
        .bytes()
        .map(|byte| (byte as char).to_digit(16).expect("hex digit") as u8)
        .collect();
    digits
        .chunks(2)
        .map(|pair| pair[0] << 4 | pair[1])
        .collect()
}

/// A dynamic-Huffman stream of 400 pseudo-random lowercase letters.
const DYNAMIC: &str = "78da1590471204210cc4de4a34d9c60c26bc7e59f55da56aa58d835890e761513e\
    93e880c2077079dcd0d5cd026c49a3c6ee0648bee866eee150b89ceda2336a307b60064b35056f8d56f7\
    2252e731d7d16e5d1b4a7ff2c2f5530197abe25a114faaacf079fee083195751e4d225d8e8370515f5c4\
    f8c42d3a2ad19bbbe51becddc34388b95a4895beada1f4dc97497dbb3a8d4a433f77d7759fbcd25b3e55\
    fbf9cc7e35276d567f38bbf3c4a325af8f0c6ae5ec25f3216b37e26f5de353e37f36d471a14dbea1dfc8\
    cf0dc00655bd6fcaf2ff11f2526d52231bc1a026fea3ed15c612e7e8d86ac90fa58df5e1350fc1b1fed9\
    cb442ae306da4027705765fd00fc8aab5d";

fn dynamic_plain() -> Vec<u8> {
    (0..400u32)
        .map(|i| ((i * i / 7 + i % 11) % 26 + 97) as u8)
        .collect()
}

#[test]
fn inflates_fixed_dynamic_and_stored_blocks() {
    let fixed = hex("78da4b4c4a4e8421001de00499");
    assert_eq!(inflate_zlib(&fixed, 100).unwrap(), b"abcabcabcabc");

    let dynamic: String = DYNAMIC.split_whitespace().collect();
    assert_eq!(inflate_zlib(&hex(&dynamic), 400).unwrap(), dynamic_plain());

    let stored = hex("7801010300fcff414243024d00c7");
    assert_eq!(inflate_zlib(&stored, 3).unwrap(), b"ABC");
}

#[test]
fn bounds_output_and_rejects_damaged_streams() {
    let fixed = hex("78da4b4c4a4e8421001de00499");
    assert_eq!(inflate_zlib(&fixed, 11), Err(FilterError::LimitExceeded));
    let dynamic: String = DYNAMIC.split_whitespace().collect();
    let dynamic = hex(&dynamic);
    assert_eq!(inflate_zlib(&dynamic, 399), Err(FilterError::LimitExceeded));

    for input in [&[][..], &[0x78], &[0x79, 0xda, 0x03, 0x00], &fixed[..6]] {
        assert_eq!(inflate_zlib(input, 100), Err(FilterError::Corrupt));
    }
    // Truncation and bit damage end in an error or other data, never a panic.
    for length in 0..dynamic.len() {
        let _ = inflate_zlib(&dynamic[..length], 4096);
    }
    for index in 0..dynamic.len() {
        let mut damaged = dynamic.clone();
        damaged[index] ^= 0x55;
        let _ = inflate_zlib(&damaged, 4096);
    }
}

#[test]
fn reverses_each_png_row_filter() {
    let predictor = Predictor {
        kind: 12,
        columns: 3,
        colors: 1,
        bits_per_component: 8,
    };
    let encoded = vec![
        0, 10, 20, 30, // none
        1, 1, 1, 1, // sub: 1 2 3
        2, 1, 1, 1, // up: 2 3 4
        3, 1, 1, 1, // average: 2 3 4
        4, 1, 1, 1, // Paeth: 3 4 5
    ];
    assert_eq!(
        unpredict(encoded, predictor).unwrap(),
        [10, 20, 30, 1, 2, 3, 2, 3, 4, 2, 3, 4, 3, 4, 5]
    );

    let none = Predictor {
        kind: 1,
        ..predictor
    };
    assert_eq!(unpredict(vec![9, 9], none).unwrap(), [9, 9]);
    let unknown_filter = unpredict(vec![7, 1, 1, 1], predictor);
    assert_eq!(unknown_filter, Err(FilterError::Corrupt));
    let huge = Predictor {
        columns: usize::MAX / 8,
        ..predictor
    };
    assert_eq!(unpredict(vec![0, 1], huge), Err(FilterError::Corrupt));
}
