//! Writing object syntax at the public library boundary.

use plain_open_pdf_engine::{
    Dictionary, ObjectReference, ParseLimits, PdfObject, decode_text_string, encode_text_string,
    parse_object, write_object,
};

fn written(object: &PdfObject) -> String {
    let mut output = Vec::new();
    write_object(object, &mut output);
    String::from_utf8(output).unwrap()
}

fn name(text: &str) -> PdfObject {
    PdfObject::Name(text.as_bytes().to_vec())
}

#[test]
fn writes_each_kind_of_object() {
    assert_eq!(written(&PdfObject::Null), "null");
    assert_eq!(written(&PdfObject::Boolean(true)), "true");
    assert_eq!(written(&PdfObject::Integer(-12)), "-12");
    assert_eq!(written(&PdfObject::Real(0.5)), "0.5");
    assert_eq!(written(&PdfObject::Real(-100.0)), "-100");
    assert_eq!(written(&PdfObject::Real(0.000_000_1)), "0.0000001");
    assert_eq!(written(&name("Type")), "/Type");
    assert_eq!(written(&name("A B#/(")), "/A#20B#23#2F#28");
    assert_eq!(
        written(&PdfObject::String(b"a (b) c".to_vec())),
        "(a \\(b\\) c)"
    );
    assert_eq!(written(&PdfObject::String(vec![0x5C])), "(\\\\)");
    assert_eq!(written(&PdfObject::String(vec![0, 255, 10])), "<00FF0A>");
    assert_eq!(written(&PdfObject::String(Vec::new())), "()");
    let reference = PdfObject::Reference(ObjectReference {
        number: 7,
        generation: 2,
    });
    assert_eq!(written(&reference), "7 2 R");

    let mut entries = Dictionary::new();
    entries.insert(
        b"Kids".to_vec(),
        PdfObject::Array(vec![reference, PdfObject::Null]),
    );
    entries.insert(b"Count".to_vec(), PdfObject::Integer(1));
    assert_eq!(
        written(&PdfObject::Dictionary(entries)),
        "<< /Count 1 /Kids [7 2 R null] >>"
    );
    assert_eq!(written(&PdfObject::Dictionary(Dictionary::new())), "<< >>");
    assert_eq!(written(&PdfObject::Array(Vec::new())), "[]");
}

#[test]
fn written_objects_parse_back_to_the_same_value() {
    let mut inner = Dictionary::new();
    inner.insert(vec![b' ', 0, 200, b'#'], PdfObject::Real(-3.25));
    inner.insert(b"S".to_vec(), PdfObject::String((0..=255).collect()));
    let object = PdfObject::Array(vec![
        PdfObject::Dictionary(inner),
        PdfObject::Array(vec![
            PdfObject::Integer(i64::MIN),
            PdfObject::Boolean(false),
        ]),
        PdfObject::String(b"((\\))".to_vec()),
        name(""),
        PdfObject::Reference(ObjectReference {
            number: 4_000_000,
            generation: 65_535,
        }),
    ]);
    let text = written(&object);
    let parsed = parse_object(text.as_bytes(), ParseLimits::default()).unwrap();
    assert_eq!(parsed.value, object);
    assert_eq!(parsed.consumed, text.len());
}

#[test]
fn encodes_text_strings() {
    assert_eq!(encode_text_string("plain (1)"), b"plain (1)");
    assert_eq!(encode_text_string("\u{e9}"), [0xFE, 0xFF, 0x00, 0xE9]);
    for text in ["", "A b", "caf\u{e9}", "\u{1f600} \u{4e2d}", "line\nbreak"] {
        assert_eq!(decode_text_string(&encode_text_string(text)), text);
    }
}
