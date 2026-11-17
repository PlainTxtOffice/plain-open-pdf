//! Resource bounds, incomplete input, and adversarial grammar cases.

use plain_open_pdf_engine::{ParseErrorKind, ParseLimits, parse_object};

#[test]
fn bounds_input_bytes_including_comments_and_lookahead() {
    for input in [
        b"(abcdef)".as_slice(),
        b"%abcdef\nnull",
        b"/abcdef",
        b"123456",
        b"1 %abcdef\n0 R",
    ] {
        let limits = ParseLimits {
            max_bytes: 4,
            ..Default::default()
        };
        let error = parse_object(input, limits).unwrap_err();
        assert_eq!(error.kind, ParseErrorKind::LimitExceeded, "{input:?}");
        assert_eq!(error.offset, 4);
    }
    let limits = ParseLimits {
        max_bytes: 4,
        ..Default::default()
    };
    assert!(parse_object(b"null", limits).is_ok());
    assert!(parse_object(b"[1] trailing", limits).is_ok());
    assert_eq!(
        parse_object(b"nullx", limits).unwrap_err().kind,
        ParseErrorKind::LimitExceeded
    );
}

#[test]
fn bounds_container_nesting_but_not_literal_parentheses() {
    let limits = ParseLimits {
        max_depth: 2,
        ..Default::default()
    };
    let parentheses = format!("[[{}x{}]]", "(".repeat(100), ")".repeat(100));
    for input in [
        b"[[]]".as_slice(),
        b"<< /A [] >>",
        b"[[(((x)))]]",
        parentheses.as_bytes(),
    ] {
        assert!(parse_object(input, limits).is_ok(), "{input:?}");
    }
    for input in [b"[[[]]]".as_slice(), b"<< /A [[]] >>"] {
        assert_eq!(
            parse_object(input, limits).unwrap_err().kind,
            ParseErrorKind::LimitExceeded
        );
    }
    let limits = ParseLimits {
        max_depth: 0,
        ..Default::default()
    };
    assert!(parse_object(b"true", limits).is_ok());
    assert_eq!(
        parse_object(b"[]", limits).unwrap_err().kind,
        ParseErrorKind::LimitExceeded
    );
}

#[test]
fn bounds_object_count_including_dictionary_keys() {
    let limits = ParseLimits {
        max_objects: 3,
        ..Default::default()
    };
    assert!(parse_object(b"[1 2]", limits).is_ok());
    assert!(parse_object(b"<< /A 1 >>", limits).is_ok());
    for input in [b"[1 2 3]".as_slice(), b"<< /A 1 /B 2 >>"] {
        assert_eq!(
            parse_object(input, limits).unwrap_err().kind,
            ParseErrorKind::LimitExceeded
        );
    }
    let limits = ParseLimits {
        max_objects: 0,
        ..Default::default()
    };
    assert_eq!(
        parse_object(b"null", limits).unwrap_err().kind,
        ParseErrorKind::LimitExceeded
    );
}

#[test]
fn default_limits_admit_large_ordinary_objects() {
    let widths = format!("[{}]", "500 ".repeat(100_000));
    assert!(parse_object(widths.as_bytes(), ParseLimits::default()).is_ok());
    let text = format!("({})", "a".repeat(4_000_000));
    assert!(parse_object(text.as_bytes(), ParseLimits::default()).is_ok());
}

#[test]
fn refuses_unsafe_requested_recursion_depth() {
    let limits = ParseLimits {
        max_depth: usize::MAX,
        ..Default::default()
    };
    assert_eq!(
        parse_object(b"null", limits).unwrap_err().kind,
        ParseErrorKind::InvalidLimits
    );
    let limits = ParseLimits {
        max_depth: 64,
        ..Default::default()
    };
    let nested = format!("{}0{}", "[".repeat(64), "]".repeat(64));
    assert!(parse_object(nested.as_bytes(), limits).is_ok());
}

#[test]
fn incomplete_containers_fail_instead_of_looping() {
    for input in [
        b"[".as_slice(),
        b"[1",
        b"<<",
        b"<< /A",
        b"<< /A 1",
        b"[1 %comment",
    ] {
        assert!(
            parse_object(input, ParseLimits::default()).is_err(),
            "{input:?}"
        );
    }
}

#[test]
fn every_one_and_two_byte_input_terminates_without_panicking() {
    let limits = ParseLimits {
        max_bytes: 2,
        max_objects: 2,
        max_depth: 2,
    };
    for first in 0..=255u8 {
        let _ = parse_object(&[first], limits);
        for second in 0..=255u8 {
            let _ = parse_object(&[first, second], limits);
        }
    }
}

#[test]
fn reports_eof_at_every_truncated_dictionary_prefix() {
    let input = b"<< /Name (value) >>";
    for length in 0..input.len() {
        let error = parse_object(&input[..length], ParseLimits::default()).unwrap_err();
        assert_eq!(
            error.kind,
            ParseErrorKind::UnexpectedEnd,
            "prefix length {length}"
        );
        assert_eq!(error.offset, length);
    }
}
