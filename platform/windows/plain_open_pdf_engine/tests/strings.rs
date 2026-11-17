//! PDF byte string normalization and escaping at the public boundary.

use plain_open_pdf_engine::{ParseErrorKind, ParseLimits, PdfObject, parse_object};

#[test]
fn literal_strings_preserve_binary_and_nested_parentheses() {
    for (input, expected) in [
        (b"()".as_slice(), b"".as_slice()),
        (b"(a(b)c)", b"a(b)c"),
        (b"(\xff\x00%<>)", b"\xff\x00%<>"),
        (b"(a\r\nb\rc\nd)", b"a\nb\nc\nd"),
        (br"(\n\r\t\b\f\(\)\\\q)", b"\n\r\t\x08\x0c()\\q"),
        (br"(\101\12\1x\777\08)", b"A\n\x01x\xff\x008"),
        (b"(a\\\r\nb\\\rc\\\nd)", b"abcd"),
    ] {
        assert_eq!(
            parse_object(input, ParseLimits::default()).unwrap().value,
            PdfObject::String(expected.to_vec()),
            "{input:?}"
        );
    }
}

#[test]
fn hex_strings_ignore_pdf_whitespace_and_pad_odd_digits() {
    for (input, expected) in [
        (b"<>".as_slice(), b"".as_slice()),
        (b"<4 1\t4\r\n2\x0c\0f>", b"AB\xf0"),
        (b"<aBcD>", b"\xab\xcd"),
    ] {
        assert_eq!(
            parse_object(input, ParseLimits::default()).unwrap().value,
            PdfObject::String(expected.to_vec())
        );
    }
    for input in [b"<4%comment\n1>".as_slice(), b"<gg>", b"<\x0b>"] {
        assert_eq!(
            parse_object(input, ParseLimits::default())
                .unwrap_err()
                .kind,
            ParseErrorKind::InvalidHexString
        );
    }
}

#[test]
fn rejects_all_truncated_prefixes_of_delimited_strings() {
    for input in [b"(a(b)c)".as_slice(), br"(a\)b)", b"<414243>"] {
        for length in 0..input.len() {
            let error = parse_object(&input[..length], ParseLimits::default()).unwrap_err();
            assert_eq!(
                error.kind,
                ParseErrorKind::UnexpectedEnd,
                "{input:?} at {length}"
            );
            assert_eq!(error.offset, length);
        }
    }
}
