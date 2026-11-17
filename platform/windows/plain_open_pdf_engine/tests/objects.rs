//! Public object grammar tests built from original byte fixtures.

use plain_open_pdf_engine::{
    ObjectReference, ParseErrorKind, ParseLimits, PdfObject, parse_object,
};

fn parse(bytes: &[u8]) -> PdfObject {
    parse_object(bytes, ParseLimits::default()).unwrap().value
}

#[test]
fn reads_scalar_values_and_pdf_number_syntax() {
    for (input, expected) in [
        ("null", PdfObject::Null),
        ("true", PdfObject::Boolean(true)),
        ("false", PdfObject::Boolean(false)),
        ("+17", PdfObject::Integer(17)),
        ("-98", PdfObject::Integer(-98)),
        ("00987", PdfObject::Integer(987)),
        ("-9223372036854775808", PdfObject::Integer(i64::MIN)),
        ("9223372036854775807", PdfObject::Integer(i64::MAX)),
        ("4.", PdfObject::Real(4.0)),
        ("-.002", PdfObject::Real(-0.002)),
        ("+.5", PdfObject::Real(0.5)),
    ] {
        assert_eq!(parse(input.as_bytes()), expected, "{input}");
    }
}

#[test]
fn reads_nested_page_dictionary_without_resolving_references() {
    let parsed = parse_object(
        b"% original fixture\r\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Info << /Flag true >> >> trailing",
        ParseLimits::default(),
    ).unwrap();
    let PdfObject::Dictionary(entries) = parsed.value else {
        panic!("dictionary expected")
    };
    assert_eq!(
        entries[b"Type".as_slice()],
        PdfObject::Name(b"Page".to_vec())
    );
    assert_eq!(
        entries[b"Parent".as_slice()],
        PdfObject::Reference(ObjectReference {
            number: 2,
            generation: 0
        })
    );
    assert_eq!(
        entries[b"MediaBox".as_slice()],
        PdfObject::Array(vec![
            PdfObject::Integer(0),
            PdfObject::Integer(0),
            PdfObject::Integer(612),
            PdfObject::Integer(792),
        ])
    );
    let PdfObject::Dictionary(info) = &entries[b"Info".as_slice()] else {
        panic!("dictionary expected")
    };
    assert_eq!(info[b"Flag".as_slice()], PdfObject::Boolean(true));
    assert_eq!(parsed.consumed, 98);
}

#[test]
fn reads_references_with_comments_and_generation_numbers() {
    assert_eq!(
        parse(b"23% number\r\n65535 % generation\nR"),
        PdfObject::Reference(ObjectReference {
            number: 23,
            generation: 65535
        })
    );
}

#[test]
fn reference_lookahead_preserves_independent_array_numbers() {
    assert_eq!(
        parse(b"[1 2 3 /R 4 0 R]"),
        PdfObject::Array(vec![
            PdfObject::Integer(1),
            PdfObject::Integer(2),
            PdfObject::Integer(3),
            PdfObject::Name(b"R".to_vec()),
            PdfObject::Reference(ObjectReference {
                number: 4,
                generation: 0
            }),
        ])
    );
    let parsed = parse_object(b"12 0 Rabbit", ParseLimits::default()).unwrap();
    assert_eq!(parsed.value, PdfObject::Integer(12));
    assert_eq!(parsed.consumed, 2);
}

#[test]
fn preserves_byte_names_and_decodes_name_escapes() {
    assert_eq!(
        parse(b"/A#20B#2f#ff"),
        PdfObject::Name(b"A B/\xff".to_vec())
    );
    assert_eq!(parse(b"/"), PdfObject::Name(Vec::new()));
    assert_eq!(parse(b"/a{b}"), PdfObject::Name(b"a".to_vec()));
    assert_eq!(parse(b"true}"), PdfObject::Boolean(true));
    for (input, expected) in [
        (b"/A#GG".as_slice(), b"A#GG".as_slice()),
        (b"/A#0", b"A#0"),
        (b"/A#", b"A#"),
        (b"/#00", b"\0"),
    ] {
        assert_eq!(
            parse(input),
            PdfObject::Name(expected.to_vec()),
            "{input:?}"
        );
    }
    assert_eq!(parse(b"/\xfe"), PdfObject::Name(vec![254]));
}

#[test]
fn reads_empty_containers_and_adjacent_objects() {
    assert_eq!(parse(b"[]"), PdfObject::Array(Vec::new()));
    assert_eq!(parse(b"<<>>"), PdfObject::Dictionary(Default::default()));
    assert_eq!(
        parse(b"[true(false)/Name<41>]"),
        PdfObject::Array(vec![
            PdfObject::Boolean(true),
            PdfObject::String(b"false".to_vec()),
            PdfObject::Name(b"Name".to_vec()),
            PdfObject::String(b"A".to_vec()),
        ])
    );
}

#[test]
fn returns_precise_prefix_length_and_does_not_parse_streams() {
    let parsed = parse_object(b" \0\t%comment\r\n[1] next", ParseLimits::default()).unwrap();
    assert_eq!(parsed.consumed, 16);
    assert_eq!(
        parse_object(b"<<>>stream\ndata", ParseLimits::default())
            .unwrap()
            .consumed,
        4
    );
}

#[test]
fn rejects_bad_syntax_with_offsets() {
    for (input, offset, kind) in [
        (b" trueX".as_slice(), 1, ParseErrorKind::UnexpectedToken),
        (b"[1e2]", 1, ParseErrorKind::InvalidNumber),
        (b"-5x", 0, ParseErrorKind::InvalidNumber),
        (b"<< 2 3 >>", 3, ParseErrorKind::UnexpectedToken),
        (b"[1}", 2, ParseErrorKind::UnexpectedToken),
    ] {
        let error = parse_object(input, ParseLimits::default()).unwrap_err();
        assert_eq!((error.offset, error.kind), (offset, kind), "{input:?}");
    }
}

#[test]
fn repairs_malformed_numbers() {
    for (input, expected) in [
        ("--5", PdfObject::Integer(-5)),
        ("+-5", PdfObject::Integer(5)),
        ("4.-5", PdfObject::Real(4.5)),
        ("1.2.3", PdfObject::Real(1.2)),
        ("+", PdfObject::Integer(0)),
        ("-.", PdfObject::Integer(0)),
        (
            "9223372036854775808",
            PdfObject::Real(9_223_372_036_854_775_808.0),
        ),
        (
            "-9223372036854775809",
            PdfObject::Real(-9_223_372_036_854_775_808.0),
        ),
    ] {
        let parsed = parse_object(input.as_bytes(), ParseLimits::default()).unwrap();
        assert_eq!(parsed.value, expected, "{input}");
        assert_eq!(parsed.consumed, input.len(), "{input}");
    }
}

#[test]
fn last_duplicate_dictionary_key_wins() {
    let PdfObject::Dictionary(entries) = parse(b"<< /A 1 /B 2 /#41 3 >>") else {
        panic!("dictionary expected")
    };
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[b"A".as_slice()], PdfObject::Integer(3));
}

#[test]
fn references_that_cannot_name_an_object_read_as_null() {
    for input in ["0 0 R", "4294967296 0 R", "1 65536 R"] {
        let parsed = parse_object(input.as_bytes(), ParseLimits::default()).unwrap();
        assert_eq!(parsed.value, PdfObject::Null, "{input}");
        assert_eq!(parsed.consumed, input.len(), "{input}");
    }
}

#[test]
fn rejects_numbers_beyond_finite_f64() {
    for input in [format!("{}.0", "9".repeat(400)), "9".repeat(400)] {
        assert_eq!(
            parse_object(input.as_bytes(), ParseLimits::default())
                .unwrap_err()
                .kind,
            ParseErrorKind::NumberOutOfRange
        );
    }
}
