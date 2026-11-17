//! Checks of the text encodings, LZW, run-length, and the TIFF predictor.
//!
//! The encoded vectors come from encoders whose output an independent PDF
//! implementation decoded back to the original data.

use super::{
    FilterError, Predictor, ascii_hex_decode, ascii85_decode, lzw_decode, run_length_decode,
    unpredict,
};

/// Encode with LZW as the decoder expects it, to round-trip long inputs.
fn lzw_encode(data: &[u8], early_change: bool) -> Vec<u8> {
    use std::collections::BTreeMap;
    let width_for = |decoder_next: usize| match decoder_next + usize::from(early_change) {
        ..512 => 9,
        512..1024 => 10,
        1024..2048 => 11,
        _ => 12,
    };
    let mut output = Vec::new();
    let (mut bits, mut count) = (0u32, 0);
    let mut emit = |code: usize, width: usize| {
        bits = bits << width | code as u32;
        count += width;
        while count >= 8 {
            count -= 8;
            output.push((bits >> count) as u8);
        }
    };
    let mut table: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
    let mut next = 258;
    let mut width = 9;
    emit(256, width);
    let mut sequence = Vec::new();
    for &byte in data {
        sequence.push(byte);
        if sequence.len() == 1 || table.contains_key(&sequence) {
            continue;
        }
        let known = &sequence[..sequence.len() - 1];
        let code = if known.len() == 1 {
            usize::from(known[0])
        } else {
            table[known]
        };
        emit(code, width);
        table.insert(sequence.clone(), next);
        next += 1;
        // The decoder defines each entry one code later than the encoder.
        width = width_for(next - 1);
        if next >= 4093 {
            emit(256, width);
            table.clear();
            next = 258;
            width = 9;
        }
        sequence = vec![byte];
    }
    if !sequence.is_empty() {
        let code = if sequence.len() == 1 {
            usize::from(sequence[0])
        } else {
            table[&sequence]
        };
        emit(code, width);
        width = width_for(next);
    }
    emit(257, width);
    emit(0, 7);
    output
}

#[test]
fn decodes_ascii_hex() {
    assert_eq!(
        ascii_hex_decode(b"48 65\n6C6c6F>ignored").unwrap(),
        b"Hello"
    );
    assert_eq!(ascii_hex_decode(b"4865 7>").unwrap(), b"Hep");
    assert_eq!(ascii_hex_decode(b"").unwrap(), b"");
    assert_eq!(ascii_hex_decode(b"4G>"), Err(FilterError::Corrupt));
}

#[test]
fn decodes_ascii85() {
    let encoded = b"87cURD_*#-6q/=\"z~>";
    assert_eq!(ascii85_decode(encoded).unwrap(), b"Hello, PDF!\0\0\0\0\0");
    assert_eq!(ascii85_decode(b"87 cU\nRD_*#-6q/=\"z").unwrap().len(), 16);
    // Final groups of two, three, and four characters hold one to three bytes.
    assert_eq!(ascii85_decode(b"5l~>").unwrap(), b"A");
    assert_eq!(ascii85_decode(b"5sb~>").unwrap(), b"AB");
    assert_eq!(ascii85_decode(b"5sdp~>").unwrap(), b"ABC");
    assert_eq!(ascii85_decode(b"~>").unwrap(), b"");
    for damaged in [b"87cUvD".as_slice(), b"s8W-\"", b"87zcU"] {
        assert_eq!(ascii85_decode(damaged), Err(FilterError::Corrupt));
    }
}

#[test]
fn decodes_run_length() {
    let encoded = [0xfc, b'a', 0x01, b'b', b'c', 0xff, b'd', 0x80, b'x'];
    assert_eq!(run_length_decode(&encoded, 100).unwrap(), b"aaaaabcdd");
    assert_eq!(
        run_length_decode(&encoded, 8),
        Err(FilterError::LimitExceeded)
    );
    // Data that stops mid-run yields what there was.
    assert_eq!(run_length_decode(&[0x05, b'a', b'b'], 100).unwrap(), b"ab");
    assert_eq!(run_length_decode(&[0xfe], 100).unwrap(), b"");
}

#[test]
fn decodes_the_lzw_example_from_the_specification() {
    let encoded = [0x80, 0x0b, 0x60, 0x50, 0x22, 0x0c, 0x0c, 0x85, 0x01];
    assert_eq!(lzw_decode(&encoded, true, 100).unwrap(), b"-----A---B");
    assert_eq!(lzw_encode(b"-----A---B", true), encoded);
    assert_eq!(
        lzw_decode(&encoded, true, 9),
        Err(FilterError::LimitExceeded)
    );
    // A code beyond the table is damage; data without an end code is not.
    assert_eq!(
        lzw_decode(&[0x80, 0x7f, 0xff], true, 100),
        Err(FilterError::Corrupt)
    );
    assert_eq!(lzw_decode(&encoded[..4], true, 100).unwrap(), b"---");
}

#[test]
fn lzw_grows_codes_and_clears_its_table() {
    // Enough distinct sequences to pass every code width and fill the table.
    let text: Vec<u8> = (0..1400u32)
        .flat_map(|i| format!("{i} squared is {}; ", i * i).into_bytes())
        .collect();
    let noise: Vec<u8> = (0..9000u32).map(|i| (i * i / 7 + i % 13) as u8).collect();
    for data in [&text, &noise] {
        for early_change in [true, false] {
            let encoded = lzw_encode(data, early_change);
            let decoded = lzw_decode(&encoded, early_change, data.len()).unwrap();
            assert_eq!(&decoded, data, "early change {early_change}");
        }
    }
    // The two conventions differ once codes outgrow nine bits.
    let encoded = lzw_encode(&text, true);
    assert_ne!(lzw_decode(&encoded, false, 1 << 20).ok(), Some(text));
}

#[test]
fn reverses_the_tiff_predictor() {
    let predictor = Predictor {
        kind: 2,
        columns: 3,
        colors: 2,
        bits_per_component: 8,
    };
    // Each row of three two-component samples stores differences.
    let encoded = vec![10, 20, 1, 2, 1, 2, 250, 5, 10, 251, 0, 0];
    assert_eq!(
        unpredict(encoded, predictor).unwrap(),
        [10, 20, 11, 22, 12, 24, 250, 5, 4, 0, 4, 0]
    );

    let wide = Predictor {
        colors: 1,
        bits_per_component: 16,
        ..predictor
    };
    let encoded = vec![0x01, 0xff, 0x00, 0x01, 0xff, 0xff];
    assert_eq!(
        unpredict(encoded, wide).unwrap(),
        [0x01, 0xff, 0x02, 0x00, 0x01, 0xff]
    );

    let packed = Predictor {
        bits_per_component: 4,
        ..predictor
    };
    assert_eq!(unpredict(vec![1, 2], packed), Err(FilterError::Unsupported));
}
