//! TrueType font programs, and text drawn with them, at the public library
//! boundary.

mod common;

use plain_open_pdf_engine::{Document, Segment, TrueTypeFont};

fn words(values: &[i32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|&value| (value as u16).to_be_bytes())
        .collect()
}

/// A font of four glyphs: an empty one, a square 1000 units wide, a
/// composite that shows the square at half size, and a curved shape.
/// "A", "B", and "C" select the last three.
fn font() -> Vec<u8> {
    // One contour; a bounding box; the last point; no instructions; then
    // a flag for each point and the changes in x and in y, as words.
    let mut square = words(&[1, 0, 0, 0, 0, 3, 0]);
    square.extend([1, 1, 1, 1]);
    square.extend(words(&[0, 0, 1000, 0, 0, 1000, 0, -1000]));
    // A composite: word offsets, a scale of one half, then glyph 1.
    let composite = words(&[-1, 0, 0, 0, 0, 0x000B, 1, 100, 200, 0x2000]);
    // The middle point is a control point off the curve.
    let mut curve = words(&[1, 0, 0, 0, 0, 2, 0]);
    curve.extend([1, 0, 1]);
    curve.extend(words(&[0, 500, 500, 0, 1000, -1000]));

    let glyphs = [Vec::new(), square, composite, curve];
    let mut locations = vec![0_u32];
    let mut outlines = Vec::new();
    for glyph in &glyphs {
        outlines.extend(glyph);
        locations.push(outlines.len() as u32);
    }
    let locations: Vec<u8> = locations
        .iter()
        .flat_map(|offset| offset.to_be_bytes())
        .collect();

    let mut head = vec![0; 54];
    head[18..20].copy_from_slice(&1000_u16.to_be_bytes());
    head[50..52].copy_from_slice(&1_u16.to_be_bytes());
    let mut horizontal = vec![0; 36];
    horizontal[34..36].copy_from_slice(&2_u16.to_be_bytes());
    let metrics = words(&[600, 0, 700, 0]);
    let mut maximum = vec![0; 6];
    maximum[4..6].copy_from_slice(&4_u16.to_be_bytes());

    // Three character maps: Windows Unicode in format 4, Windows full
    // Unicode in format 12, and Macintosh in format 6.
    let unicode = words(&[
        4, 32, 0, 4, 0, 0, 0, 67, 0xFFFF, 0, 65, 0xFFFF, -64, 1, 0, 0,
    ]);
    let mut full = words(&[12, 0]);
    for value in [28_u32, 0, 1, 0x1F600, 0x1F600, 3] {
        full.extend(value.to_be_bytes());
    }
    let macintosh = words(&[6, 12, 0, 65, 1, 3]);
    let mut maps = words(&[0, 3]);
    let mut offset = 4 + 3 * 8;
    for (platform, encoding, table) in [(1, 0, &macintosh), (3, 1, &unicode), (3, 10, &full)] {
        maps.extend(words(&[platform, encoding]));
        maps.extend((offset as u32).to_be_bytes());
        offset += table.len();
    }
    for table in [&macintosh, &unicode, &full] {
        maps.extend(table);
    }

    let tables: [(&[u8; 4], Vec<u8>); 7] = [
        (b"cmap", maps),
        (b"glyf", outlines),
        (b"head", head),
        (b"hhea", horizontal),
        (b"hmtx", metrics),
        (b"loca", locations),
        (b"maxp", maximum),
    ];
    let mut file = vec![0, 1, 0, 0];
    file.extend(words(&[tables.len() as i32, 0, 0, 0]));
    let mut offset = 12 + tables.len() * 16;
    for (tag, data) in &tables {
        file.extend(*tag);
        file.extend([0; 4]);
        file.extend((offset as u32).to_be_bytes());
        file.extend((data.len() as u32).to_be_bytes());
        offset += data.len();
    }
    for (_, data) in &tables {
        file.extend(data);
    }
    file
}

#[test]
fn reads_tables_and_character_maps() {
    let font = TrueTypeFont::parse(font()).unwrap();
    assert_eq!(font.glyph_count(), 4);
    assert_eq!(font.units_per_em(), 1000);
    assert_eq!(font.advance(0), Some(600));
    // Glyphs past the last entry share its advance.
    assert_eq!(font.advance(3), Some(700));

    assert!(font.has_map(3, 1) && font.has_map(1, 0) && !font.has_map(0, 3));
    assert_eq!(font.glyph_index(3, 1, 65), Some(1));
    assert_eq!(font.glyph_index(3, 1, 67), Some(3));
    assert_eq!(font.glyph_index(3, 1, 68), None);
    assert_eq!(font.glyph_index(3, 1, 0x1F600), None);
    assert_eq!(font.glyph_index(3, 10, 0x1F600), Some(3));
    assert_eq!(font.glyph_index(3, 10, 65), None);
    assert_eq!(font.glyph_index(1, 0, 65), Some(3));
    assert_eq!(font.glyph_index(1, 0, 66), None);
    assert_eq!(font.glyph_index(0, 3, 65), None);
}

#[test]
fn reads_simple_curved_and_composite_outlines() {
    use Segment::{Close, Line, Move, Quad};
    let font = TrueTypeFont::parse(font()).unwrap();
    assert_eq!(
        *font.outline(1),
        [
            Move(0.0, 0.0),
            Line(0.0, 1000.0),
            Line(1000.0, 1000.0),
            Line(1000.0, 0.0),
            Line(0.0, 0.0),
            Close
        ]
    );
    // The same square at half size, moved by the composite's offset.
    assert_eq!(
        *font.outline(2),
        [
            Move(100.0, 200.0),
            Line(100.0, 700.0),
            Line(600.0, 700.0),
            Line(600.0, 200.0),
            Line(100.0, 200.0),
            Close
        ]
    );
    assert_eq!(
        *font.outline(3),
        [
            Move(0.0, 0.0),
            Quad(500.0, 1000.0, 1000.0, 0.0),
            Line(0.0, 0.0),
            Close
        ]
    );
    // A glyph without a description, and one the font does not have.
    assert!(font.outline(0).is_empty());
    assert!(font.outline(9).is_empty());
}

#[test]
fn damaged_fonts_never_panic() {
    let whole = font();
    assert!(TrueTypeFont::parse(Vec::new()).is_none());
    assert!(TrueTypeFont::parse(b"OTTO then CFF outlines".to_vec()).is_none());
    let probe = |data: Vec<u8>| {
        if let Some(font) = TrueTypeFont::parse(data) {
            for glyph in 0..6 {
                let _ = font.outline(glyph);
                let _ = font.advance(glyph);
            }
            for (platform, encoding) in [(3, 1), (3, 10), (1, 0)] {
                for code in [0, 65, 66, 67, 0x1F600, u32::MAX] {
                    let _ = font.glyph_index(platform, encoding, code);
                }
            }
        }
    };
    for length in 0..whole.len() {
        probe(whole[..length].to_vec());
    }
    for index in 0..whole.len() {
        for change in [0x01, 0x80, 0xFF] {
            let mut damaged = whole.clone();
            damaged[index] ^= change;
            probe(damaged);
        }
    }
}

/// A 200 point square page that shows "A" at 100 points with a font that
/// embeds the test font, and "A" at 50 points with one that embeds none.
fn text_file() -> Vec<u8> {
    let font = font();
    let content = b"BT /F1 100 Tf 10 10 Td (A) Tj ET 1 0 0 rg BT /F2 50 Tf 130 130 Td (A) Tj ET";
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R \
          /Resources << /Font << /F1 5 0 R /F2 8 0 R >> >> >>"
            .to_vec(),
        [
            format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
            content,
            b"\nendstream",
        ]
        .concat(),
        b"<< /Type /Font /Subtype /TrueType /BaseFont /Test /Encoding /WinAnsiEncoding \
          /FirstChar 65 /Widths [1000] /FontDescriptor 6 0 R >>"
            .to_vec(),
        b"<< /Type /FontDescriptor /FontName /Test /FontFile2 7 0 R >>".to_vec(),
        [
            format!("<< /Length {} >>\nstream\n", font.len()).as_bytes(),
            &font,
            b"\nendstream",
        ]
        .concat(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
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
    let trailer = format!(
        "trailer\n<< /Root 1 0 R /Size {} >>\nstartxref\n{table}\n%%EOF\n",
        objects.len() + 1
    );
    file.extend(trailer.as_bytes());
    file
}

#[test]
fn draws_text_with_embedded_and_fallback_fonts() {
    const WHITE: Option<[u8; 4]> = Some([255, 255, 255, 255]);
    const BLACK: Option<[u8; 4]> = Some([0, 0, 0, 255]);
    let mut document = Document::open(text_file()).unwrap();
    // The embedded font's "A" is a square as wide as the font size, from
    // (10, 10) to (110, 110) on the page, which is drawn top down.
    let image = document.render_page(0, 1.0).unwrap();
    assert_eq!(image.pixel(60, 140), BLACK);
    assert_eq!(image.pixel(12, 188), BLACK);
    assert_eq!(image.pixel(108, 92), BLACK);
    assert_eq!(image.pixel(60, 85), WHITE);
    assert_eq!(image.pixel(115, 140), WHITE);
    // Without a fallback font, the font that embeds nothing draws nothing.
    assert_eq!(image.pixel(150, 50), WHITE);
    common::assert_regions_match(&mut document, 0, 1.0);

    assert!(!document.set_fallback_font(b"not a font".to_vec()));
    assert!(document.set_fallback_font(font()));
    let image = document.render_page(0, 1.0).unwrap();
    assert_eq!(image.pixel(60, 140), BLACK);
    // The fallback's "A" in the fill color, 50 points square from
    // (130, 130).
    assert_eq!(image.pixel(150, 50), Some([255, 0, 0, 255]));
    assert_eq!(image.pixel(190, 50), WHITE);
    for scale in [1.0, 0.61, 3.27] {
        common::assert_regions_match(&mut document, 0, scale);
    }

    // Text extraction is unchanged by how the text is drawn.
    assert_eq!(document.page_text(0).unwrap(), "A\nA\n");
}
