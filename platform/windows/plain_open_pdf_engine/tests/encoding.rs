//! Simple font encodings and glyph names at the public library boundary.

use plain_open_pdf_engine::{PdfObject, SimpleEncoding, glyph_text};

fn name(text: &str) -> PdfObject {
    PdfObject::Name(text.as_bytes().to_vec())
}

#[test]
fn predefined_encodings_follow_annex_d() {
    let standard = SimpleEncoding::standard();
    assert_eq!(
        standard.decode(b"It's `ok'"),
        "It\u{2019}s \u{2018}ok\u{2019}"
    );
    assert_eq!(standard.decode(b"\xAE\xE1\xFB"), "\u{fb01}\u{c6}\u{df}");
    // Codes the encoding lacks are replaced.
    assert_eq!(standard.text(0x80), None);
    assert_eq!(standard.decode(b"a\x80\x0Ab"), "a\u{fffd}\u{fffd}b");

    let win = SimpleEncoding::win_ansi();
    assert_eq!(
        win.decode(b"caf\xE9 \x80 \x93x\x94"),
        "caf\u{e9} \u{20ac} \u{201c}x\u{201d}"
    );
    assert_eq!(win.decode(b"'\x92`"), "'\u{2019}`");
    // The second space and hyphen, and an unused code.
    assert_eq!(win.decode(b"\xA0\xAD\x81"), " -\u{2022}");

    let mac = SimpleEncoding::mac_roman();
    assert_eq!(
        mac.decode(b"caf\x8E\xDB\xCA\xD2"),
        "caf\u{e9}\u{a4} \u{201c}"
    );
    assert_eq!(mac.text(0xAD), None);

    assert_eq!(
        SimpleEncoding::symbol().decode(b"a\xB3\xA5"),
        "\u{3b1}\u{2265}\u{221e}"
    );
    assert_eq!(
        SimpleEncoding::zapf_dingbats().decode(b"\x21\x48"),
        "\u{2701}\u{2605}"
    );
}

#[test]
fn selects_the_encoding_a_font_starts_from() {
    let win = SimpleEncoding::win_ansi();
    assert_eq!(SimpleEncoding::named(b"WinAnsiEncoding"), Some(win.clone()));
    assert_eq!(SimpleEncoding::named(b"MacExpertEncoding"), None);
    assert_eq!(SimpleEncoding::named(b"Identity-H"), None);

    let for_font = SimpleEncoding::for_font;
    assert_eq!(
        for_font(b"TrueType", b"Arial", Some(b"WinAnsiEncoding")),
        win
    );
    assert_eq!(for_font(b"Type1", b"Symbol", Some(b"WinAnsiEncoding")), win);
    // Without a supported name, the built-in encoding applies.
    let standard = SimpleEncoding::standard();
    assert_eq!(for_font(b"Type1", b"Helvetica", None), standard);
    assert_eq!(
        for_font(b"Type1", b"Times", Some(b"MacExpertEncoding")),
        standard
    );
    assert_eq!(
        for_font(b"Type1", b"Symbol", None),
        SimpleEncoding::symbol()
    );
    assert_eq!(
        for_font(b"TrueType", b"ABCDEF+SymbolMT", None),
        SimpleEncoding::symbol()
    );
    assert_eq!(
        for_font(b"Type1", b"ZapfDingbats", None),
        SimpleEncoding::zapf_dingbats()
    );
    assert_eq!(for_font(b"Type3", b"", None).decode(b"A"), "\u{fffd}");
}

#[test]
fn differences_replace_codes_by_glyph_name() {
    let mut encoding = SimpleEncoding::win_ansi();
    encoding.apply_differences(&[
        name("ignored"),
        PdfObject::Integer(65),
        name("Euro"),
        name("f_f_i"),
        name("unknown17"),
        PdfObject::Integer(200),
        name("uni2603"),
        PdfObject::Null,
        name("a.sc"),
        PdfObject::Integer(255),
        name("zero"),
        name("one"),
        PdfObject::Integer(-1),
        name("two"),
    ]);
    assert_eq!(encoding.decode(b"ABCD"), "\u{20ac}ffi\u{fffd}D");
    assert_eq!(encoding.decode(b"\xC8\xC9\xCA\xFF"), "\u{2603}a\u{ca}0");
}

#[test]
fn reads_glyph_names() {
    let text = |name: &str| glyph_text(name.as_bytes());
    assert_eq!(text("eacute").as_deref(), Some("\u{e9}"));
    assert_eq!(text("union").as_deref(), Some("\u{222a}"));
    assert_eq!(text("a100").as_deref(), Some("\u{275e}"));
    assert_eq!(text("ffl").as_deref(), Some("\u{fb04}"));
    assert_eq!(text("f_l.alt").as_deref(), Some("fl"));
    assert_eq!(text("uni00410042").as_deref(), Some("AB"));
    assert_eq!(text("u1F600").as_deref(), Some("\u{1f600}"));
    for unknown in [
        "", ".notdef", "g23", "uni004", "uniD800", "u110000", "u+041", "A_g23",
    ] {
        assert_eq!(text(unknown), None, "{unknown}");
    }
    assert_eq!(glyph_text(b"\xFF"), None);
}
