//! CFF font programs, and text drawn with them, at the public library
//! boundary.

mod common;

use plain_open_pdf_engine::{CffFont, Document, Segment};

/// A number as a Type 2 charstring or DICT operand of one or two bytes.
fn number(value: i32) -> Vec<u8> {
    match value {
        -107..=107 => vec![(value + 139) as u8],
        108..=1131 => vec![
            247 + ((value - 108) / 256) as u8,
            ((value - 108) % 256) as u8,
        ],
        _ => vec![
            251 + ((-value - 108) / 256) as u8,
            ((-value - 108) % 256) as u8,
        ],
    }
}

/// Numbers followed by an operator.
fn operation(numbers: &[i32], operator: u8) -> Vec<u8> {
    let mut bytes: Vec<u8> = numbers.iter().flat_map(|&value| number(value)).collect();
    bytes.push(operator);
    bytes
}

/// An INDEX of items, with one-byte offsets.
fn index(items: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = (items.len() as u16).to_be_bytes().to_vec();
    if items.is_empty() {
        return bytes;
    }
    bytes.push(1);
    let mut offset = 1;
    bytes.push(offset);
    for item in items {
        offset += item.len() as u8;
        bytes.push(offset);
    }
    bytes.extend(items.concat());
    bytes
}

/// A DICT entry whose number always takes five bytes, so that the size of
/// the dictionary does not depend on the positions it holds.
fn entry(values: &[usize], operator: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    for &value in values {
        bytes.push(29);
        bytes.extend((value as u32).to_be_bytes());
    }
    bytes.push(operator);
    bytes
}

/// A font of four glyphs: `.notdef`; "A", a square from (100, 100) to
/// (400, 400) after a width; "sq", a curve drawn by a local subroutine; and
/// "B", which passes hints and a hint mask and calls a global subroutine.
/// The font's own encoding gives them codes 65, 97, and 66.
fn font() -> Vec<u8> {
    const RMOVETO: u8 = 21;
    const HLINETO: u8 = 6;
    const VLINETO: u8 = 7;
    const ENDCHAR: u8 = 14;
    let square = [
        operation(&[600, 100, 100], RMOVETO),
        operation(&[300], HLINETO),
        operation(&[300], VLINETO),
        operation(&[-300], HLINETO),
        vec![ENDCHAR],
    ]
    .concat();
    let curved = [
        operation(&[0, 0], RMOVETO),
        operation(&[-107], 10),
        vec![ENDCHAR],
    ]
    .concat();
    let hinted = [
        operation(&[10, 20], 18),
        // Two more numbers before the mask are a vertical stem, so the
        // mask has one byte for the two stems.
        operation(&[30, 40], 19),
        vec![0xC0],
        operation(&[50, 60], RMOVETO),
        operation(&[10], HLINETO),
        operation(&[-107], 29),
        vec![ENDCHAR],
    ]
    .concat();
    let char_strings = index(&[vec![ENDCHAR], square, curved, hinted]);
    let local = index(&[[operation(&[100, 0, 0, 100, -100, 0], 8), vec![11]].concat()]);
    let global = index(&[[operation(&[10], VLINETO), vec![11]].concat()]);
    // String 34 is "A" and 35 is "B"; 391 is the font's first own string.
    let charset: Vec<u8> = [
        vec![0],
        [34_u16, 391, 35]
            .iter()
            .flat_map(|sid| sid.to_be_bytes())
            .collect(),
    ]
    .concat();
    let encoding = vec![0, 3, 65, 97, 66];
    let private = entry(&[6], 19);

    let names = index(&[b"Test".to_vec()]);
    let strings = index(&[b"sq".to_vec()]);
    // The top dictionary holds four entries of fixed size.
    let top_size = 6 + 6 + 6 + 11;
    let start = 4 + names.len() + (5 + top_size) + strings.len() + global.len();
    let charset_at = start;
    let encoding_at = charset_at + charset.len();
    let char_strings_at = encoding_at + encoding.len();
    let private_at = char_strings_at + char_strings.len();
    let top = [
        entry(&[charset_at], 15),
        entry(&[encoding_at], 16),
        entry(&[char_strings_at], 17),
        entry(&[private.len(), private_at], 18),
    ]
    .concat();
    assert_eq!(top.len(), top_size);
    [
        vec![1, 0, 4, 1],
        names,
        index(&[top]),
        strings,
        global,
        charset,
        encoding,
        char_strings,
        private,
        local,
    ]
    .concat()
}

#[test]
fn reads_names_encoding_and_outlines() {
    use Segment::{Close, Cubic, Line, Move};
    let font = CffFont::parse(font()).unwrap();
    assert_eq!(font.glyph_count(), 4);
    assert_eq!(font.units_per_em(), 1000.0);
    assert!(!font.is_cid_keyed());

    let names: Vec<_> = (0..5).map(|glyph| font.glyph_name(glyph)).collect();
    assert_eq!(
        names,
        [Some(".notdef"), Some("A"), Some("sq"), Some("B"), None]
    );
    assert_eq!(font.glyph_by_name("sq"), Some(2));
    assert_eq!(font.glyph_by_name("B"), Some(3));
    assert_eq!(font.glyph_by_name("C"), None);
    assert_eq!(font.glyph_by_code(65), Some(1));
    assert_eq!(font.glyph_by_code(97), Some(2));
    assert_eq!(font.glyph_by_code(66), Some(3));
    assert_eq!(font.glyph_by_code(67), None);
    // A font that is not CID-keyed takes a CID as the glyph itself.
    assert_eq!(font.glyph_by_cid(2), Some(2));
    assert_eq!(font.glyph_by_cid(4), None);

    // The first number of the square's charstring is its width.
    assert_eq!(
        *font.outline(1),
        [
            Move(100.0, 100.0),
            Line(400.0, 100.0),
            Line(400.0, 400.0),
            Line(100.0, 400.0),
            Close
        ]
    );
    assert_eq!(
        *font.outline(2),
        [
            Move(0.0, 0.0),
            Cubic(100.0, 0.0, 100.0, 100.0, 0.0, 100.0),
            Close
        ]
    );
    assert_eq!(
        *font.outline(3),
        [Move(50.0, 60.0), Line(60.0, 60.0), Line(60.0, 70.0), Close]
    );
    assert!(font.outline(0).is_empty());
    assert!(font.outline(9).is_empty());
}

#[test]
fn reads_a_font_inside_an_opentype_file() {
    let cff = font();
    let mut file = b"OTTO".to_vec();
    file.extend([0, 1, 0, 0, 0, 0, 0, 0]);
    file.extend(b"CFF ");
    file.extend([0; 4]);
    file.extend(28_u32.to_be_bytes());
    file.extend((cff.len() as u32).to_be_bytes());
    file.extend(&cff);
    let font = CffFont::parse(file).unwrap();
    assert_eq!(font.glyph_by_name("A"), Some(1));
    assert_eq!(font.outline(1).len(), 5);
}

#[test]
fn damaged_fonts_never_panic() {
    let whole = font();
    assert!(CffFont::parse(Vec::new()).is_none());
    assert!(CffFont::parse(b"\x00\x01\x00\x00 a TrueType font".to_vec()).is_none());
    let probe = |data: Vec<u8>| {
        if let Some(font) = CffFont::parse(data) {
            for glyph in 0..6 {
                let _ = font.outline(glyph);
                let _ = font.glyph_name(glyph);
                let _ = font.glyph_by_cid(glyph);
            }
            let _ = font.glyph_by_name("A");
            let _ = font.glyph_by_code(65);
        }
    };
    for length in 0..whole.len() {
        probe(whole[..length].to_vec());
    }
    for index in 0..whole.len() {
        for change in [0x01, 0x10, 0x80, 0xFF] {
            let mut damaged = whole.clone();
            damaged[index] ^= change;
            probe(damaged);
        }
    }
}

/// A 200 point square page that shows "A" at 100 points with a font that
/// embeds the test font as `/FontFile3`, then "a" with a font that has no
/// encoding of its own, so that the program's encoding picks the glyph.
fn text_file() -> Vec<u8> {
    text_file_with(b"BT /F1 100 Tf 10 10 Td (A) Tj ET 0 0 1 rg BT /F2 100 Tf 100 100 Td (a) Tj ET")
}

/// The page of `text_file` with other content.
fn text_file_with(content: &[u8]) -> Vec<u8> {
    let font = font();
    let stream = |entries: &str, data: &[u8]| {
        let head = format!("<< {entries} /Length {} >>\nstream\n", data.len());
        [head.as_bytes(), data, b"\nendstream"].concat()
    };
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R \
          /Resources << /Font << /F1 5 0 R /F2 8 0 R >> >> >>"
            .to_vec(),
        stream("", content),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+Test /Encoding /WinAnsiEncoding \
          /FirstChar 65 /Widths [600] /FontDescriptor 6 0 R >>"
            .to_vec(),
        b"<< /Type /FontDescriptor /FontName /ABCDEF+Test /FontFile3 7 0 R >>".to_vec(),
        stream("/Subtype /Type1C", &font),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+Test \
          /FirstChar 97 /Widths [600] /FontDescriptor 6 0 R >>"
            .to_vec(),
    ];
    let mut file = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(file.len());
        file.extend(format!("{} 0 obj\n", index + 1).as_bytes());
        file.extend(object);
        file.extend(b"\nendobj\n");
    }
    let table = file.len();
    file.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        file.extend(format!("{offset:010} 00000 n \n").as_bytes());
    }
    let size = objects.len() + 1;
    file.extend(
        format!("trailer\n<< /Root 1 0 R /Size {size} >>\nstartxref\n{table}\n%%EOF\n").as_bytes(),
    );
    file
}

#[test]
fn draws_text_with_an_embedded_cff_font() {
    const WHITE: Option<[u8; 4]> = Some([255, 255, 255, 255]);
    const BLACK: Option<[u8; 4]> = Some([0, 0, 0, 255]);
    let mut document = Document::open(text_file()).unwrap();
    let image = document.render_page(0, 1.0).unwrap();
    // "A" is a square from 100 to 400 units, so at 100 points it covers
    // (20, 20) to (50, 50) on the page, which is drawn top down.
    assert_eq!(image.pixel(35, 165), BLACK);
    assert_eq!(image.pixel(22, 178), BLACK);
    assert_eq!(image.pixel(48, 152), BLACK);
    assert_eq!(image.pixel(15, 165), WHITE);
    assert_eq!(image.pixel(55, 165), WHITE);
    assert_eq!(image.pixel(35, 145), WHITE);
    // "a" is found through the program's own encoding: a curve bulging
    // right from (100, 100), in the fill color.
    assert_eq!(image.pixel(104, 95), Some([0, 0, 255, 255]));
    assert_eq!(image.pixel(98, 95), WHITE);
    assert_eq!(image.pixel(112, 95), WHITE);
    for scale in [1.0, 0.37, 2.54] {
        common::assert_regions_match(&mut document, 0, scale);
    }
}

#[test]
fn glyphs_placed_far_off_the_page_leave_the_rest_drawn() {
    const BLACK: Option<[u8; 4]> = Some([0, 0, 0, 255]);
    // A glyph at a position so far away that quarter pixels cannot be told
    // apart, or that overflows whole pixels, comes first; the same glyph at
    // its usual place must still be drawn, and nothing may panic.
    let huge = format!("1{}", "0".repeat(308));
    for far in [
        format!("{huge} 10"),
        format!("10 {huge}"),
        "100000000000000000000 10".to_string(),
        "-100000000000000000000 10".to_string(),
        "10 100000000000000000000".to_string(),
    ] {
        let content = format!("BT /F1 100 Tf {far} Td (A) Tj ET BT /F1 100 Tf 10 10 Td (A) Tj ET");
        let mut document = Document::open(text_file_with(content.as_bytes())).unwrap();
        let image = document.render_page(0, 1.0).unwrap();
        assert_eq!(image.pixel(35, 165), BLACK, "{far}");
        common::assert_regions_match(&mut document, 0, 1.0);
    }
}
