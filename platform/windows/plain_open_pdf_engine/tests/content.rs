//! Content stream operations at the public library boundary.

use plain_open_pdf_engine::{Operation, ParseErrorKind, PdfObject, parse_content};

fn operation(operator: &str, operands: Vec<PdfObject>) -> Operation {
    Operation {
        operator: operator.as_bytes().to_vec(),
        operands,
    }
}

fn name(text: &str) -> PdfObject {
    PdfObject::Name(text.as_bytes().to_vec())
}

fn string(text: &str) -> PdfObject {
    PdfObject::String(text.as_bytes().to_vec())
}

#[test]
fn reads_operands_then_operators() {
    let content =
        b"q 1 0 0 1 72.5 -700 cm\nBT /F1 12 Tf (Hello) Tj\n[(A) -120 (B)] TJ T* (x) ' ET Q";
    let integer = PdfObject::Integer;
    assert_eq!(
        parse_content(content).unwrap(),
        [
            operation("q", vec![]),
            operation(
                "cm",
                vec![
                    integer(1),
                    integer(0),
                    integer(0),
                    integer(1),
                    PdfObject::Real(72.5),
                    integer(-700),
                ]
            ),
            operation("BT", vec![]),
            operation("Tf", vec![name("F1"), integer(12)]),
            operation("Tj", vec![string("Hello")]),
            operation(
                "TJ",
                vec![PdfObject::Array(vec![
                    string("A"),
                    integer(-120),
                    string("B")
                ])]
            ),
            operation("T*", vec![]),
            operation("'", vec![string("x")]),
            operation("ET", vec![]),
            operation("Q", vec![]),
        ]
    );
}

#[test]
fn reads_keyword_operands_comments_and_dictionaries() {
    let content = b"% comment\n/Span << /MCID 3 /Lang (en) >> BDC true false null gs EMC\n1 0 0 rg";
    let operations = parse_content(content).unwrap();
    let operators: Vec<_> = operations
        .iter()
        .map(|operation| String::from_utf8_lossy(&operation.operator).into_owned())
        .collect();
    assert_eq!(operators, ["BDC", "gs", "EMC", "rg"]);
    assert_eq!(operations[0].operands.len(), 2);
    assert!(operations[0].operands[1].as_dictionary().is_some());
    assert_eq!(
        operations[1].operands,
        [
            PdfObject::Boolean(true),
            PdfObject::Boolean(false),
            PdfObject::Null
        ]
    );
    // Numbers in a row are operands, never an indirect reference.
    assert_eq!(operations[3].operands.len(), 3);
}

#[test]
fn keeps_inline_image_data() {
    let content = b"q BI /W 2 /H 1 /BPC 8 /CS /G ID \x00EI\xff EI Q (after) Tj";
    let operations = parse_content(content).unwrap();
    assert_eq!(operations.len(), 4);
    assert_eq!(operations[1].operator, b"BI");
    // The dictionary's keys and values, then the data.
    assert_eq!(operations[1].operands.len(), 9);
    assert_eq!(operations[1].operands[0], name("W"));
    let data = PdfObject::String(b"\x00EI\xff".to_vec());
    assert_eq!(operations[1].operands[8], data);
    assert_eq!(operations[3], operation("Tj", vec![string("after")]));

    // Image data that never ends runs to the end of the stream.
    let unfinished = parse_content(b"BI /W 1 ID \x01\x02\x03").unwrap();
    assert_eq!(unfinished.len(), 1);
    let data = PdfObject::String(b"\x01\x02\x03".to_vec());
    assert_eq!(unfinished[0].operands[2], data);
    let empty = parse_content(b"BI /W 1 ID EI").unwrap();
    assert_eq!(empty[0].operands[2], PdfObject::String(Vec::new()));
    let missing = parse_content(b"BI /W 1 Q").unwrap_err();
    assert_eq!(missing.kind, ParseErrorKind::UnexpectedToken);
}

#[test]
fn tolerates_empty_trailing_and_stray_input() {
    assert_eq!(parse_content(b"").unwrap(), []);
    assert_eq!(parse_content(b"  % only a comment").unwrap(), []);
    // Operands with no operator are dropped; a stray delimiter is skipped.
    assert_eq!(
        parse_content(b"Q ] ) 1 2").unwrap(),
        [operation("Q", vec![])]
    );
    assert_eq!(
        parse_content(b"(unterminated").unwrap_err().kind,
        ParseErrorKind::UnexpectedEnd
    );
    for first in 0..=255u8 {
        for second in *b" (<B1" {
            let _ = parse_content(&[first, second, b'I', b' ', b'I', b'D']);
        }
    }
}
