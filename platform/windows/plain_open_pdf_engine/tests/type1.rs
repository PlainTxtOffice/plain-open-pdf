//! Type 1 font programs, and text drawn with them, at the public library
//! boundary.

mod common;

use plain_open_pdf_engine::{Document, Segment, Type1Font};

/// A charstring number: one byte, two bytes, or a 32-bit integer.
fn number(value: i32) -> Vec<u8> {
    match value {
        -107..=107 => vec![(value + 139) as u8],
        108..=1131 => vec![
            247 + ((value - 108) / 256) as u8,
            ((value - 108) % 256) as u8,
        ],
        -1131..=-108 => vec![
            251 + ((-value - 108) / 256) as u8,
            ((-value - 108) % 256) as u8,
        ],
        _ => [vec![255], value.to_be_bytes().to_vec()].concat(),
    }
}

/// Numbers followed by an operator; operators above 255 are escaped.
fn operation(numbers: &[i32], operator: u16) -> Vec<u8> {
    let mut bytes: Vec<u8> = numbers.iter().flat_map(|&value| number(value)).collect();
    if operator >= 1200 {
        bytes.extend([12, (operator - 1200) as u8]);
    } else {
        bytes.push(operator as u8);
    }
    bytes
}

/// Encrypt as eexec or charstrings do, after four bytes that stand in for
/// random ones.
fn encrypt(plain: &[u8], key: u16) -> Vec<u8> {
    let mut state = key;
    [b"\x10\x20\x30\x40".as_slice(), plain]
        .concat()
        .into_iter()
        .map(|byte| {
            let cipher = byte ^ (state >> 8) as u8;
            state = (u16::from(cipher).wrapping_add(state))
                .wrapping_mul(52845)
                .wrapping_add(22719);
            cipher
        })
        .collect()
}

fn subroutines() -> Vec<Vec<u8>> {
    vec![
        // The standard flex and hint replacement subroutines.
        [
            operation(&[3, 0], 1216),
            operation(&[], 1217),
            operation(&[], 1217),
        ]
        .concat()
        .into_iter()
        .chain(operation(&[], 1233))
        .chain(operation(&[], 11))
        .collect(),
        [operation(&[0, 1], 1216), operation(&[], 11)].concat(),
        [operation(&[0, 2], 1216), operation(&[], 11)].concat(),
        [
            operation(&[3, 1, 3], 1216),
            operation(&[], 1217),
            operation(&[], 10),
            operation(&[], 11),
        ]
        .concat(),
        // A line used by the square.
        [operation(&[300, 0], 5), operation(&[], 11)].concat(),
    ]
}

/// The glyphs, as (name, plain charstring).
fn glyphs() -> Vec<(&'static str, Vec<u8>)> {
    let square = [
        operation(&[0, 500], 13),
        operation(&[100, 100], 21),
        operation(&[4], 10),
        // 600 / 2 = 300.
        operation(&[600, 2], 1212),
        operation(&[], 7),
        operation(&[-300], 6),
        operation(&[], 9),
        operation(&[], 14),
    ]
    .concat();
    let curve = [
        operation(&[50, 600], 13),
        operation(&[0, 0], 21),
        operation(&[100, 50, 50, 100], 30),
        operation(&[100, 50, -50, -100], 31),
        operation(&[], 9),
        operation(&[], 14),
    ]
    .concat();
    let mut flex = [
        operation(&[0, 500], 13),
        operation(&[0, 0], 21),
        operation(&[1], 10),
    ]
    .concat();
    for (dx, dy) in [
        (300, 50),
        (-200, 0),
        (100, 50),
        (100, 0),
        (100, 0),
        (100, -50),
        (100, -50),
    ] {
        flex.extend(operation(&[dx, dy], 21));
        flex.extend(operation(&[2], 10));
    }
    flex.extend(operation(&[50, 600, 0, 0], 10));
    flex.extend(operation(&[-600, 0], 5));
    flex.extend(operation(&[], 9));
    flex.extend(operation(&[], 14));
    let letter = [
        operation(&[0, 600], 13),
        operation(&[0, 0], 21),
        operation(&[100, 0], 5),
        operation(&[0, 100], 5),
        operation(&[], 9),
        operation(&[], 14),
    ]
    .concat();
    let acute = [
        operation(&[50, 300], 13),
        operation(&[0, 0], 21),
        operation(&[10, 10], 5),
        operation(&[], 9),
        operation(&[], 14),
    ]
    .concat();
    // A with an acute: the accent's origin at (150 - 50, 200).
    let accented = [
        operation(&[0, 600], 13),
        operation(&[50, 150, 200, 65, 194], 1206),
    ]
    .concat();
    vec![
        (
            ".notdef",
            [operation(&[0, 500], 13), operation(&[], 14)].concat(),
        ),
        ("A", letter),
        ("acute", acute),
        ("square", square),
        ("curve", curve),
        ("flex", flex),
        ("Aacute", accented),
    ]
}

#[derive(Clone, Copy)]
enum Layout {
    Binary,
    Hex,
    Pfb,
}

/// A font program: the clear text part, the private part, and the zeros
/// that end it. `standard` gives the font `StandardEncoding`; `plain`
/// leaves its charstrings unencrypted.
fn program(layout: Layout, standard: bool, plain: bool) -> (Vec<u8>, usize, usize) {
    let encoding = if standard {
        "/Encoding StandardEncoding def\n".to_string()
    } else {
        "/Encoding 256 array\n0 1 255 {1 index exch /.notdef put} for\n\
         dup 65 /A put\ndup 66 /square put\ndup 67 /curve put\n\
         dup 68 /flex put\ndup 69 /Aacute put\nreadonly def\n"
            .to_string()
    };
    let clear = format!(
        "%!PS-AdobeFont-1.0: Test 001.000\n11 dict begin\n/FontName /Test def\n\
         /FontMatrix [0.001 0 0 0.001 0 0] readonly def\n{encoding}currentfile eexec\n"
    );
    // The hexadecimal font names its procedures as some fonts do.
    let (read, define, put) = match layout {
        Layout::Hex => ("-|", "|-", "|"),
        _ => ("RD", "ND", "NP"),
    };
    let seal = |code: &[u8]| {
        if plain {
            code.to_vec()
        } else {
            encrypt(code, 4330)
        }
    };
    let mut private = b"dup /Private 9 dict dup begin\n\
        /RD{string currentfile exch readstring pop}executeonly def\n\
        /ND{noaccess def}executeonly def\n/NP{noaccess put}executeonly def\n"
        .to_vec();
    if plain {
        private.extend(b"/lenIV -1 def\n");
    }
    let subroutines = subroutines();
    private.extend(format!("/Subrs {} array\n", subroutines.len()).as_bytes());
    for (index, code) in subroutines.iter().enumerate() {
        let code = seal(code);
        private.extend(format!("dup {index} {} {read} ", code.len()).as_bytes());
        private.extend(&code);
        private.extend(format!(" {put}\n").as_bytes());
    }
    private.extend(
        format!(
            "{define}\n2 index /CharStrings {} dict dup begin\n",
            glyphs().len()
        )
        .as_bytes(),
    );
    for (name, code) in glyphs() {
        let code = seal(&code);
        private.extend(format!("/{name} {} {read} ", code.len()).as_bytes());
        private.extend(&code);
        private.extend(format!(" {define}\n").as_bytes());
    }
    private.extend(
        b"end\nend\nreadonly put\nnoaccess put\ndup/FontName get exch definefont pop\n\
          mark currentfile closefile\n",
    );
    let mut encrypted = encrypt(&private, 55665);
    if let Layout::Hex = layout {
        let digits: String = encrypted.iter().map(|byte| format!("{byte:02x}")).collect();
        let lines: Vec<&str> = digits
            .as_bytes()
            .chunks(64)
            .map(|line| std::str::from_utf8(line).unwrap())
            .collect();
        encrypted = lines.join("\n").into_bytes();
    }
    let trailer = format!("\n{}cleartomark\n", "0".repeat(512)).into_bytes();
    let (clear_length, encrypted_length) = (clear.len(), encrypted.len());
    let data = match layout {
        Layout::Pfb => {
            let segment = |kind: u8, data: &[u8]| {
                let mut bytes = vec![0x80, kind];
                bytes.extend((data.len() as u32).to_le_bytes());
                bytes.extend(data);
                bytes
            };
            [
                segment(1, clear.as_bytes()),
                segment(2, &encrypted),
                segment(1, &trailer),
                vec![0x80, 3],
            ]
            .concat()
        }
        _ => [clear.into_bytes(), encrypted, trailer].concat(),
    };
    (data, clear_length, encrypted_length)
}

fn outline(font: &Type1Font, name: &str) -> Vec<Segment> {
    font.outline(font.glyph_by_name(name).unwrap()).to_vec()
}

#[test]
fn reads_glyphs_through_subroutines_flex_and_accents() {
    let font = Type1Font::parse(program(Layout::Binary, false, false).0).unwrap();
    assert_eq!(font.glyph_count(), 7);
    assert_eq!(font.units_per_em(), 1000.0);
    assert_eq!(font.glyph_name(3), Some("square"));
    assert_eq!(font.glyph_by_code(66), font.glyph_by_name("square"));
    assert_eq!(font.glyph_by_code(70), None);
    assert_eq!(font.encoded_name(69), Some("Aacute"));
    assert_eq!(
        outline(&font, "square"),
        [
            Segment::Move(100.0, 100.0),
            Segment::Line(400.0, 100.0),
            Segment::Line(400.0, 400.0),
            Segment::Line(100.0, 400.0),
            Segment::Close,
        ]
    );
    // Drawing starts at the left sidebearing point that hsbw sets.
    assert_eq!(
        outline(&font, "curve"),
        [
            Segment::Move(50.0, 0.0),
            Segment::Cubic(50.0, 100.0, 100.0, 150.0, 200.0, 150.0),
            Segment::Cubic(300.0, 150.0, 350.0, 100.0, 350.0, 0.0),
            Segment::Close,
        ]
    );
    // A flex's reference point is not drawn; its other six points make
    // two curves, and the end point it leaves is the current point.
    assert_eq!(
        outline(&font, "flex"),
        [
            Segment::Move(0.0, 0.0),
            Segment::Cubic(100.0, 50.0, 200.0, 100.0, 300.0, 100.0),
            Segment::Cubic(400.0, 100.0, 500.0, 50.0, 600.0, 0.0),
            Segment::Line(0.0, 0.0),
            Segment::Close,
        ]
    );
    assert_eq!(
        outline(&font, "Aacute"),
        [
            Segment::Move(0.0, 0.0),
            Segment::Line(100.0, 0.0),
            Segment::Line(100.0, 100.0),
            Segment::Close,
            Segment::Move(150.0, 200.0),
            Segment::Line(160.0, 210.0),
            Segment::Close,
        ]
    );
    assert_eq!(outline(&font, ".notdef"), []);
}

#[test]
fn reads_every_layout_and_encoding() {
    let binary = Type1Font::parse(program(Layout::Binary, false, false).0).unwrap();
    for (layout, plain) in [
        (Layout::Hex, false),
        (Layout::Pfb, false),
        (Layout::Binary, true),
    ] {
        let font = Type1Font::parse(program(layout, false, plain).0).unwrap();
        for name in ["square", "curve", "flex", "Aacute"] {
            assert_eq!(outline(&font, name), outline(&binary, name), "{name}");
        }
    }
    let standard = Type1Font::parse(program(Layout::Binary, true, false).0).unwrap();
    assert_eq!(standard.glyph_by_code(65), standard.glyph_by_name("A"));
    assert_eq!(standard.encoded_name(194), Some("acute"));
    assert!(Type1Font::parse(b"%!PS-AdobeFont-1.0 without a private part".to_vec()).is_none());
}

#[test]
fn damaged_fonts_never_panic() {
    let (data, ..) = program(Layout::Binary, false, false);
    for index in (0..data.len()).step_by(7) {
        let mut damaged = data.clone();
        damaged[index] ^= 0x5A;
        if let Some(font) = Type1Font::parse(damaged.clone()) {
            for glyph in 0..font.glyph_count() as u16 {
                font.outline(glyph);
            }
        }
        damaged.truncate(index);
        let _ = Type1Font::parse(damaged);
    }
}

/// A page that shows the square at 100 points from a font whose program is
/// embedded as `/FontFile`, with its own encoding.
fn text_file() -> Vec<u8> {
    let (font, clear, encrypted) = program(Layout::Binary, false, false);
    let content = b"BT /F1 100 Tf 10 10 Td (B) Tj ET";
    let stream = |entries: &str, data: &[u8]| {
        let head = format!("<< {entries} /Length {} >>\nstream\n", data.len());
        [head.as_bytes(), data, b"\nendstream"].concat()
    };
    let lengths = format!(
        "/Length1 {clear} /Length2 {encrypted} /Length3 {}",
        font.len() - clear - encrypted
    );
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R \
          /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_vec(),
        stream("", content),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Test /FirstChar 66 /Widths [500] \
          /FontDescriptor 6 0 R >>"
            .to_vec(),
        b"<< /Type /FontDescriptor /FontName /Test /FontFile 7 0 R >>".to_vec(),
        stream(&lengths, &font),
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
fn draws_text_with_an_embedded_type1_font() {
    const WHITE: Option<[u8; 4]> = Some([255, 255, 255, 255]);
    const BLACK: Option<[u8; 4]> = Some([0, 0, 0, 255]);
    let mut document = Document::open(text_file()).unwrap();
    let image = document.render_page(0, 1.0).unwrap();
    // The square runs from 100 to 400 units, so at 100 points from (10, 10)
    // it covers (20, 20) to (50, 50) on the page, drawn top down.
    assert_eq!(image.pixel(35, 165), BLACK);
    assert_eq!(image.pixel(22, 178), BLACK);
    assert_eq!(image.pixel(48, 152), BLACK);
    assert_eq!(image.pixel(15, 165), WHITE);
    assert_eq!(image.pixel(55, 165), WHITE);
    assert_eq!(image.pixel(35, 145), WHITE);
    for scale in [1.0, 1.9] {
        common::assert_regions_match(&mut document, 0, scale);
    }
}
