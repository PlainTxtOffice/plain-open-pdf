//! Tests of the standard 14 fonts and their metrics.

use plain_open_pdf_engine::StandardFont;

#[test]
fn names_every_font_and_finds_it_again() {
    for font in StandardFont::ALL {
        assert_eq!(StandardFont::from_name(font.name()), Some(font));
    }
    assert_eq!(StandardFont::ALL.len(), 14);
}

#[test]
fn recognizes_common_names_for_the_same_designs() {
    let found = |name| StandardFont::from_name(name);
    assert_eq!(found("Arial"), Some(StandardFont::Helvetica));
    assert_eq!(found("ArialMT"), Some(StandardFont::Helvetica));
    assert_eq!(found("Arial,Bold"), Some(StandardFont::HelveticaBold));
    assert_eq!(
        found("Arial-BoldItalicMT"),
        Some(StandardFont::HelveticaBoldOblique)
    );
    assert_eq!(
        found("Times New Roman,Italic"),
        Some(StandardFont::TimesItalic)
    );
    assert_eq!(found("TimesNewRomanPSMT"), Some(StandardFont::TimesRoman));
    assert_eq!(
        found("TimesNewRomanPS-BoldMT"),
        Some(StandardFont::TimesBold)
    );
    assert_eq!(found("Times"), Some(StandardFont::TimesRoman));
    assert_eq!(
        found("CourierNew,BoldItalic"),
        Some(StandardFont::CourierBoldOblique)
    );
    assert_eq!(found("CourierNewPSMT"), Some(StandardFont::Courier));
    assert_eq!(found("Helvetica,Regular"), Some(StandardFont::Helvetica));
    // Other designs have other metrics.
    for other in [
        "Helvetica-Narrow",
        "Arial-Black",
        "ArialNarrow",
        "Verdana",
        "Symbol,Bold",
        "",
    ] {
        assert_eq!(found(other), None, "{other}");
    }
}

#[test]
fn gives_adobe_widths_and_font_metrics() {
    let helvetica = StandardFont::Helvetica;
    assert_eq!(helvetica.width("H"), Some(722));
    assert_eq!(helvetica.width("space"), Some(278));
    assert_eq!(helvetica.width("Euro"), Some(556));
    assert_eq!(helvetica.width("alpha"), None);
    assert_eq!(StandardFont::TimesRoman.width("e"), Some(444));
    assert_eq!(StandardFont::CourierBoldOblique.width("W"), Some(600));
    assert_eq!(StandardFont::Symbol.width("alpha"), Some(631));
    assert_eq!(StandardFont::ZapfDingbats.width("a1"), Some(974));
    assert_eq!(helvetica.bounding_box(), [-166, -225, 1000, 931]);
    assert_eq!((helvetica.ascender(), helvetica.descender()), (718, -207));
    assert_eq!((helvetica.cap_height(), helvetica.x_height()), (718, 523));
    assert_eq!(StandardFont::TimesItalic.italic_angle(), -15.5);
    assert!(StandardFont::Symbol.is_symbolic() && !helvetica.is_symbolic());
}

#[test]
fn encodes_and_measures_text() {
    let helvetica = StandardFont::Helvetica;
    // WinAnsiEncoding: the e acute is 0xE9, the em dash 0x97, the bullet
    // 0x95, and a character it lacks becomes a question mark.
    assert_eq!(
        helvetica.encode("Caf\u{e9} \u{2014}\u{2022}\u{4e2d}"),
        b"Caf\xe9 \x97\x95?"
    );
    // H 722, e 556, l 222, l 222, o 556 thousandths.
    assert_eq!(helvetica.text_width("Hello", 10.0), 22.78);
    assert_eq!(helvetica.text_width("", 10.0), 0.0);
    assert_eq!(StandardFont::Courier.text_width("abc", 10.0), 18.0);
    // Symbol and ZapfDingbats use their own encodings.
    assert_eq!(StandardFont::Symbol.encode("\u{3b1}?"), b"a?");
    assert_eq!(StandardFont::Symbol.text_width("\u{3b1}", 1000.0), 631.0);
    // A character they lack below U+0100 is taken as a code: "a" is alpha.
    assert_eq!(StandardFont::Symbol.encode("a\u{4e2d}"), b"a?");
    assert_eq!(StandardFont::Symbol.text_width("a", 1000.0), 631.0);
    assert_eq!(
        StandardFont::ZapfDingbats.encode("\u{2701}!\u{4e2d}"),
        b"!! "
    );
}
