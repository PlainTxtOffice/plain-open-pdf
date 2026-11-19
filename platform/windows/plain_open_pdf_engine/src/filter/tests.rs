//! Decoder checks against streams produced by an independent zlib encoder
//! and against short streams built bit by bit.

use super::{FilterError, Predictor, deflate_zlib, inflate_zlib, unpredict};

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
    // A stream cut anywhere before its checksum is corrupt; the checksum
    // itself is not needed.
    let checksum = dynamic.len() - 4;
    for length in 0..checksum {
        assert_eq!(inflate_zlib(&dynamic[..length], 4096), CORRUPT);
    }
    for length in checksum..=dynamic.len() {
        assert_eq!(inflate_zlib(&dynamic[..length], 4096), Ok(dynamic_plain()));
    }
    // Bit damage ends in an error or other data, never a panic.
    for index in 0..dynamic.len() {
        let mut damaged = dynamic.clone();
        damaged[index] ^= 0x55;
        let _ = inflate_zlib(&damaged, 4096);
    }
}

const CORRUPT: Result<Vec<u8>, FilterError> = Err(FilterError::Corrupt);
const TOO_LONG: Result<Vec<u8>, FilterError> = Err(FilterError::LimitExceeded);

/// Decode a stream given as hex digits into at most 1000 bytes.
fn inflate_hex(stream: &str) -> Result<Vec<u8>, FilterError> {
    inflate_zlib(&hex(stream), 1000)
}

/// Check that every stream that stops before the last byte is corrupt.
fn assert_every_cut_is_corrupt(stream: &str) {
    let stream = hex(stream);
    for length in 0..stream.len() {
        assert_eq!(inflate_zlib(&stream[..length], 1000), CORRUPT, "{length}");
    }
}

#[test]
fn reads_stored_blocks_between_compressed_blocks() {
    // A fixed block, then a stored block: the decoder has read ahead and
    // must find the stored bytes again.
    let fixed_then_stored = "789c7204040200fdff7879";
    assert_eq!(inflate_hex(fixed_then_stored).unwrap(), b"Axy");
    assert_every_cut_is_corrupt(fixed_then_stored);
    let with_trailing_bytes = "789c7204040200fdff7879010203040506070809";
    assert_eq!(inflate_hex(with_trailing_bytes).unwrap(), b"Axy");

    let two_stored_then_fixed = "789c000100feff51000000ffff8b0200";
    assert_eq!(inflate_hex(two_stored_then_fixed).unwrap(), b"QZ");
    assert_every_cut_is_corrupt(two_stored_then_fixed);
}

#[test]
fn reads_streams_that_end_without_a_checksum() {
    assert_eq!(inflate_hex("789c0300").unwrap(), b"");
    assert_eq!(inflate_hex("789c4b4c0200").unwrap(), b"ab");
    // The same stream with its checksum has exactly eight bytes of data.
    assert_eq!(inflate_hex("789c4b4c0200012600c4").unwrap(), b"ab");
    // The end-of-block code is seven zero bits. Five are here; the missing
    // two must not be made up.
    assert_eq!(inflate_hex("789c03"), CORRUPT);
    // Three literals and no end-of-block code.
    assert_eq!(inflate_hex("789c4b4c4a06"), CORRUPT);
}

#[test]
fn rejects_symbols_and_block_types_that_do_not_exist() {
    let cases = [
        ("789c4b1c0300", "length symbol 286"),
        ("789c4b1c0700", "length symbol 287"),
        ("789c4b043e00", "distance symbol 30"),
        ("789c4b047e00", "distance symbol 31"),
        ("789c0700", "block type 3"),
    ];
    for (stream, name) in cases {
        assert_eq!(inflate_hex(stream), CORRUPT, "{name}");
    }
}

#[test]
fn copies_matches_from_the_output_so_far() {
    // Distance 2 after two bytes reaches the first byte.
    assert_eq!(inflate_hex("789c4b4c024200").unwrap(), b"ababa");
    // Distance 3 after two bytes, and a match before any output.
    assert_eq!(inflate_hex("789c4b4c022200"), CORRUPT);
    assert_eq!(inflate_hex("789c030200"), CORRUPT);

    // Matches that run into the bytes they write.
    assert_eq!(inflate_hex("789c4b1c0500").unwrap(), [b'a'; 259]);
    assert_eq!(inflate_hex("789c4b4c024300").unwrap(), b"abababa");
    assert_eq!(inflate_hex("789c4b4c4a462000").unwrap(), b"abcabcabcabca");
}

#[test]
fn reports_a_bad_distance_before_the_limit() {
    // Distance 3 after two bytes with length 258: corrupt, though the match
    // would also pass the limit.
    assert_eq!(inflate_zlib(&hex("789c4b4c1a4500"), 4), CORRUPT);
    assert_eq!(inflate_zlib(&hex("789c4b4c024200"), 4), TOO_LONG);
    assert_eq!(inflate_zlib(&hex("789c4b4c024200"), 5).unwrap(), b"ababa");
    let run = hex("789c4b1c0500");
    assert_eq!(inflate_zlib(&run, 258), TOO_LONG);
    assert_eq!(inflate_zlib(&run, 259).unwrap(), [b'a'; 259]);
}

#[test]
fn checks_stored_blocks() {
    let stored = hex("789c010300fcff414243");
    assert_eq!(inflate_zlib(&stored, 3).unwrap(), b"ABC");
    assert_eq!(inflate_zlib(&stored, 2), TOO_LONG);
    assert_every_cut_is_corrupt("789c010300fcff414243");
    // The length and its complement do not match.
    assert_eq!(inflate_hex("789c010300fcfe414243"), CORRUPT);
}

#[test]
fn reads_dynamic_blocks_with_unusual_codes() {
    // No distance code at all, and a single distance code of one bit.
    assert_eq!(
        inflate_hex("789c05e0db922449922ccb7e1bfeff4f2110").unwrap(),
        b"AA"
    );
    assert_eq!(
        inflate_hex("789c05e0db922449922ccb7e1bfeff4f210a").unwrap(),
        b"A"
    );
    // A literal code of two 2-bit codes leaves half the codes unused.
    assert_eq!(
        inflate_hex("789c05e0db922449922ccb7e1bfdff4f1120").unwrap(),
        b"A"
    );
    assert_eq!(
        inflate_hex("789c05e0db922449922ccb7e1bfdff4f11f0ff3f"),
        CORRUPT,
        "an unused code in the data"
    );
    // A repeat that starts in the literal lengths and ends in the distance
    // lengths.
    assert_eq!(
        inflate_hex("789c05e3db922449922ccbfeff4fd31d03").unwrap(),
        [0xfe, 0xff]
    );
}

#[test]
fn rejects_damaged_dynamic_block_headers() {
    let cases = [
        (
            "789c05e0db922449922ccb5ebe0dffffa71008",
            "a repeat of the previous length comes first",
        ),
        (
            "789cf5e0db922449922ccb7e1bfeff4f2110",
            "287 literal/length codes",
        ),
        ("789c05fedb922449922ccb7e1bfeff4f2110", "31 distance codes"),
        ("789c05ffdb922449922ccb7e1bfeff4f2110", "32 distance codes"),
        (
            "789c05e0db922449922ccb7e1be2ffff140001",
            "no end-of-block code",
        ),
        (
            "789c05e0db922449922ccb7e1be2ff7f140201",
            "more literal codes than fit",
        ),
        (
            "789c05e09324499224499200",
            "more code-length codes than fit",
        ),
        ("789c05e00100000000000000", "no code-length code"),
        (
            "789c05e2db922449922ccbfeff4fd31d03",
            "a repeat runs past the last length",
        ),
        (
            "789c05e0db922449922ccbfeffffff3f",
            "a run of zeros runs past the last length",
        ),
    ];
    for (stream, name) in cases {
        assert_eq!(inflate_hex(stream), CORRUPT, "{name}");
    }
}

#[test]
fn reads_codes_of_fifteen_bits() {
    // Both codes have 15-bit codes, and the stream ends with its last byte.
    let stream = "789cedfddb922449922ccbfec41ffcc803d26f3c3c3c984756cfdae77cd74fbeb07cc333\
        28128b9a4756cfda7f708efeffffbfeff6ff1f";
    assert_eq!(inflate_hex(stream).unwrap(), b"ABnmdnmdn");
    assert_eq!(inflate_zlib(&hex(stream), 8), TOO_LONG);
    assert_every_cut_is_corrupt(stream);
}

/// A Huffman-only stream of 2047 letters: `A` once, `B` twice, and so on
/// up to `K` 1024 times, so that `A` and the end of the block have codes of
/// 11 bits.
const LONG_CODES: &str = "780105c1816124c9110431dbf4c79dae0cf8ef8f80ff254992244992244992244992\
    244992244992244992244992244992244992244992244992244992999999999999999999999999999999\
    9999999999999999999939e79c73ce39e79c73ce39e79c73ce39cff33ccff33ccff33ccfe7f3f97c3e9f\
    9f9f9f3ffffc972449922449922449922449922449922449922449922449922449922449922449922449\
    92244966666666666666666666666666666666666666666666666666e69c73ce39e79c73ce39e79c73ce\
    39e79ce7799ee7799ee7799ecfe7f3f97c3e3f3f3f7ffefc479224499224499224499224499224499224\
    499224499224499224499224499224499224499224333333333333333333333333333333333333333333\
    3333333373ce39e79c73ce39e79c73ce39e79c73cef33ccff33ccff33ccff3f97c3e9fcfcfcfcf9f3fff\
    489224499224499224499224499224499224499224499224499224499224499224499224499264666666\
    66666666666666666666666666666666666666666666ce39e79c73ce39e79c73ce39e79c73ce799ee779\
    9ee7799ee7793e9fcfe7f3f9f9f9f9f3e71f499224499224499224499224499224499224499224499224\
    4992244992244992244992244992cccccccccccccccccccccccccccccccccccccccccccccccccccc9c73\
    ce39e79c73ce39e79c73ce39e79ce7799ee7799ee7799ee7f3f97c3e9f9f9f9f9f3ffffc1f6c234fe0";

#[test]
fn reads_codes_longer_than_the_lookup_table() {
    // Each letter twice as often as the one before, then spread evenly.
    let mut sorted = Vec::new();
    for (power, letter) in (b'A'..=b'K').enumerate() {
        sorted.extend(std::iter::repeat_n(letter, 1 << power));
    }
    let plain: Vec<u8> = (0..sorted.len())
        .map(|index| sorted[index * 1021 % sorted.len()])
        .collect();
    assert_eq!(plain.len(), 2047);

    let stream = hex(LONG_CODES);
    assert_eq!(inflate_zlib(&stream, 2047).unwrap(), plain);
    assert_eq!(inflate_zlib(&stream, 2046), TOO_LONG);
    for length in 0..stream.len() - 4 {
        assert_eq!(inflate_zlib(&stream[..length], 2047), CORRUPT, "{length}");
    }
}

#[test]
fn reverses_the_crate_encoder_on_long_matches() {
    let mut state = 2_463_534_242_u32;
    let mut noise = |length: usize| -> Vec<u8> {
        (0..length)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state >> 8) as u8
            })
            .collect()
    };
    let samples = [
        // One byte repeated: every match is one byte back.
        vec![0; 100_000],
        // A block of the largest distance a match can reach, three times.
        noise(32_768).repeat(3),
        noise(70_000),
    ];
    for sample in samples {
        let packed = deflate_zlib(&sample);
        assert_eq!(inflate_zlib(&packed, usize::MAX).as_ref(), Ok(&sample));
        assert_eq!(inflate_zlib(&packed, sample.len()).as_ref(), Ok(&sample));
        assert_eq!(inflate_zlib(&packed, sample.len() - 1), TOO_LONG);
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
