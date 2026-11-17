//! Rendering a rectangle of a page: it must hold exactly the pixels of
//! that rectangle of the whole page image.

mod common;

use plain_open_pdf_engine::{Document, PixelRegion};

/// A PDF file with the given objects, numbered from 1; the first is the
/// catalog.
fn file(objects: &[Vec<u8>]) -> Vec<u8> {
    let mut file = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(file.len());
        file.extend(format!("{} 0 obj\n", index + 1).as_bytes());
        file.extend(object);
        file.extend(b"\nendobj\n");
    }
    let table = file.len();
    let size = objects.len() + 1;
    file.extend(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in offsets {
        file.extend(format!("{offset:010} 00000 n \n").as_bytes());
    }
    file.extend(
        format!("trailer\n<< /Root 1 0 R /Size {size} >>\nstartxref\n{table}\n%%EOF\n").as_bytes(),
    );
    file
}

/// A stream object with the given dictionary entries.
fn stream(entries: &str, data: &[u8]) -> Vec<u8> {
    let head = format!("<< {entries} /Length {} >>\nstream\n", data.len());
    [head.as_bytes(), data, b"\nendstream"].concat()
}

/// A plain object.
fn object(body: &str) -> Vec<u8> {
    body.as_bytes().to_vec()
}

/// How many pixels of an image are not white.
fn drawn_pixels(document: &mut Document, scale: f64) -> usize {
    let image = document.render_page(0, scale).unwrap();
    let pixels = image.data().chunks(4);
    pixels.filter(|pixel| pixel[..3] != [255; 3]).count()
}

/// A 180 by 140 point page with a little of everything the renderer
/// draws: curved, even-odd, and overlapping fills; dashed, round-capped,
/// and hairline strokes; nested clips; images upright, turned, masked,
/// half-transparent, shrunk, and hanging off the page; shadings as an
/// operator and as a pattern; tiling patterns, colored and uncolored, on a
/// turned grid; nested forms; a blend mode; Type 3 text; and an annotation.
fn busy_file(rotation: u16, crop: &str) -> Vec<u8> {
    let content = b"
        0.9 0.2 0.1 rg 10.3 12.7 m 60.2 8.1 80.9 50.5 40.4 60.6 c
        20 70 5.5 40.25 10.3 12.7 c h 30 30 15.5 12.25 re f*
        0 0.3 0.8 RG 3.3 w 1 J [7 4.5] 2 d 15 130 m 90 95 120 135 170 100 c S
        [] 0 d 0.2 w 0 G 0 0 m 180 140 l S
        q 95.5 60.25 60 50 re W n 100 50 m 160 85 l 110 120 l h 140 70 30 30 re W* n
        0.1 0.6 0.2 rg 0 0 180 140 re f
        q 30.7 0 0 22.9 110.2 75.4 cm /Im0 Do Q
        /Sh1 sh
        Q
        q 37.3 0 0 21.7 12.4 80.6 cm /Im0 Do Q
        q 20 12 -9 15 100 20 cm /Im0 Do Q
        q /Half gs 25 0 0 25 -8.5 -6.5 cm /Im1 Do Q
        q 11 0 0 9 150.5 5.5 cm /Im2 Do Q
        q 40 0 0 30 160 120 cm /Im1 Do Q
        q 14 9 -6 11 168 60 cm /Im1 Do Q
        /Pattern cs /P1 scn 60 5 35.5 30.5 re f
        /P2 scn 5 95 m 50 99 l 45 135 l 8 128 l h f
        /Pattern CS /P2 SCN 6 w 100 5 m 140 15 l S
        /Cs1 cs 0.7 0 0.5 /P3 scn 55 100 m 95 104 90 138 60 132 c h f
        q 0.8 0.3 -0.3 0.8 60 40 cm /Fm Do Q
        q /Mul gs 1 1 0 rg 0 60 180 12 re f Q
        0 0.5 0.5 rg BT /F3 14 Tf 20 70 Td (AAB) Tj ET";
    // Four by four pixels of red, green, and blue that all differ.
    let colors: Vec<u8> = (0..48).map(|index| (index * 53 % 256) as u8).collect();
    let grays: Vec<u8> = (0..9).map(|index| (index * 31) as u8).collect();
    let alphas = [255, 128, 0, 200, 255, 60, 0, 255, 255];
    let many: Vec<u8> = (0..64 * 48)
        .map(|index| (((index % 64) * 4) ^ ((index / 64) * 5)) as u8)
        .collect();
    let cell = b"0 0 4.5 4.5 re f 0.2 0.4 0.9 rg 5 5 3 3 re f";
    let form = b"0.3 0.3 0.9 rg 0 0 50 50 re f q 1 0 0 1 5 5 cm /Inner Do Q";
    let inner = b"1 0.8 0 rg 0 0 12 12 re f 0 G 1.5 w 2 2 m 30 9 l S";
    let square = b"1000 0 0 0 750 750 d1 0 0 750 750 re f";
    let blue = b"1000 0 d0 0 0 1 rg 0 0 500 500 re f";
    let green = b"0 1 0 rg 0 0 10 10 re f";
    let tiling = |paint: u8| {
        stream(
            &format!(
                "/PatternType 1 /PaintType {paint} /TilingType 1 /BBox [0 0 8 8] \
                 /XStep 8 /YStep 8 /Matrix [0.9 0.3 -0.3 0.9 3 2] /Resources << >>"
            ),
            cell,
        )
    };
    file(&[
        object("<< /Type /Catalog /Pages 2 0 R >>"),
        object("<< /Type /Pages /Count 1 /Kids [3 0 R] >>"),
        object(&format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 180 140] {crop} /Rotate {rotation} \
             /Contents 4 0 R /Annots [18 0 R] /Resources << \
             /XObject << /Im0 5 0 R /Im1 6 0 R /Im2 8 0 R /Fm 13 0 R >> \
             /ExtGState << /Half << /ca 0.5 >> /Mul << /BM /Multiply >> >> \
             /Shading << /Sh1 9 0 R >> \
             /Pattern << /P1 10 0 R /P2 11 0 R /P3 12 0 R >> \
             /ColorSpace << /Cs1 [/Pattern /DeviceRGB] >> \
             /Font << /F3 15 0 R >> >> >>"
        )),
        stream("", content),
        stream(
            "/Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceRGB /BitsPerComponent 8",
            &colors,
        ),
        stream(
            "/Subtype /Image /Width 3 /Height 3 /ColorSpace /DeviceGray /BitsPerComponent 8 \
             /SMask 7 0 R",
            &grays,
        ),
        stream(
            "/Subtype /Image /Width 3 /Height 3 /ColorSpace /DeviceGray /BitsPerComponent 8",
            &alphas,
        ),
        stream(
            "/Subtype /Image /Width 64 /Height 48 /ColorSpace /DeviceGray /BitsPerComponent 8",
            &many,
        ),
        object(
            "<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [90 60 160 110] \
             /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0.2] /C1 [0 0.4 1] /N 1 >> \
             /Extend [true true] >>",
        ),
        object("<< /PatternType 2 /Shading 9 0 R /Matrix [0.4 0.1 0 0.5 30 -20] >>"),
        tiling(1),
        tiling(2),
        stream(
            "/Subtype /Form /BBox [0 0 30 30] /Matrix [1 0 0 1 2 3] \
             /Resources << /XObject << /Inner 14 0 R >> >>",
            form,
        ),
        stream("/Subtype /Form /BBox [0 0 40 20]", inner),
        object(
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] \
             /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /square 16 0 R /blue 17 0 R >> \
             /Encoding << /Type /Encoding /Differences [65 /square /blue] >> \
             /FirstChar 65 /LastChar 66 /Widths [1000 1000] /Resources << >> >>",
        ),
        stream("", square),
        stream("", blue),
        object(
            "<< /Type /Annot /Subtype /Square /Rect [150.5 30.25 172 52] /AP << /N 19 0 R >> >>",
        ),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", green),
    ])
}

#[test]
fn regions_of_a_busy_page_match_the_whole_image() {
    for rotation in [0, 90, 180, 270] {
        let mut document = Document::open(busy_file(rotation, "")).unwrap();
        // The page really is drawn on, all over.
        let image = document.render_page(0, 1.0).unwrap();
        let (width, height) = if rotation % 180 == 0 {
            (180, 140)
        } else {
            (140, 180)
        };
        assert_eq!((image.width(), image.height()), (width, height));
        assert!(drawn_pixels(&mut document, 1.0) > width * height / 4);
        for scale in [1.0, 2.0, 4.54, 0.7, 3.33] {
            common::assert_regions_match(&mut document, 0, scale);
        }
    }
}

#[test]
fn regions_of_a_cropped_page_match_the_whole_image() {
    // A crop box that starts and ends between pixels moves everything by a
    // part of a pixel.
    let crop = "/CropBox [10.5 7.25 170.25 131.5]";
    for rotation in [0, 270] {
        let mut document = Document::open(busy_file(rotation, crop)).unwrap();
        for scale in [1.0, 2.6] {
            common::assert_regions_match(&mut document, 0, scale);
        }
    }
}

/// A 200 point square page filled with a tiling pattern whose cells step
/// by `step` points and paint, half transparent, well past their own
/// bounding box, so every pixel takes paint from several cells.
fn tiled_file(step: u32) -> Vec<u8> {
    let cell = b"/Faint gs 0.9 0.1 0.3 rg -11.5 -9.5 33 31 re f 0 0.2 0.8 rg 1 1 4 4 re f";
    file(&[
        object("<< /Type /Catalog /Pages 2 0 R >>"),
        object("<< /Type /Pages /Count 1 /Kids [3 0 R] >>"),
        object(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R \
             /Resources << /Pattern << /P1 5 0 R >> >> >>",
        ),
        stream("", b"/Pattern cs /P1 scn 0 0 200 200 re f"),
        stream(
            &format!(
                "/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] \
                 /XStep {step} /YStep {step} \
                 /Resources << /ExtGState << /Faint << /ca 0.3 >> >> >>"
            ),
            cell,
        ),
    ])
}

/// A small rectangle in the middle of the tiled page.
const MIDDLE: PixelRegion = PixelRegion {
    left: 93,
    top: 88,
    width: 9,
    height: 7,
};

#[test]
fn a_small_region_of_a_large_tiled_shape_takes_paint_from_every_cell() {
    let mut document = Document::open(tiled_file(10)).unwrap();
    // Every pixel is painted, by more than the one cell it lies in.
    assert_eq!(drawn_pixels(&mut document, 1.0), 200 * 200);
    let whole = document.render_page(0, 1.0).unwrap();
    let part = document.render_page_region(0, 1.0, MIDDLE).unwrap();
    assert_eq!(part.data(), common::crop(&whole, 93, 88, 9, 7).as_slice());
    for scale in [1.0, 1.37] {
        common::assert_regions_match(&mut document, 0, scale);
    }
}

#[test]
fn a_pattern_with_too_many_tiles_for_the_page_is_left_out_of_a_region_too() {
    // A step of two points makes more tiles over the page than are drawn
    // for one fill, so the fill is left out. The few tiles that reach a
    // small region are within that number, and must be left out all the
    // same.
    let mut document = Document::open(tiled_file(2)).unwrap();
    assert_eq!(drawn_pixels(&mut document, 1.0), 0);
    let part = document.render_page_region(0, 1.0, MIDDLE).unwrap();
    assert_eq!((part.width(), part.height()), (9, 7));
    assert!(part.data().iter().all(|&byte| byte == 255));
    common::assert_regions_match(&mut document, 0, 1.0);
    // Drawn larger the page takes as many tiles, and stays empty.
    common::assert_regions_match(&mut document, 0, 2.5);
}

#[test]
fn content_that_stops_early_stops_the_same_way_for_a_region() {
    // Content is drawn up to an image that cannot be read. The image lies
    // far from the region, which must still stop where the page does.
    let ends = [
        "/Filter /FlateDecode /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8",
        "/Width 2 /Height 2 /ColorSpace 9 0 R /BitsPerComponent 8",
        "/Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask 5 0 R",
        "/Width 900000 /Height 900000 /ColorSpace /DeviceRGB /BitsPerComponent 8",
    ];
    for entries in ends {
        let content = b"1 0 0 rg 0 0 60 60 re f
            q 10 0 0 10 180 180 cm /Im0 Do Q
            0 0 1 rg 30 30 60 60 re f";
        let bytes = file(&[
            object("<< /Type /Catalog /Pages 2 0 R >>"),
            object("<< /Type /Pages /Count 1 /Kids [3 0 R] >>"),
            object(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R \
                 /Resources << /XObject << /Im0 5 0 R >> >> >>",
            ),
            stream("", content),
            stream(&format!("/Subtype /Image {entries}"), b"not image data"),
        ]);
        let mut document = Document::open(bytes).unwrap();
        let region = PixelRegion {
            left: 20,
            top: 120,
            width: 50,
            height: 40,
        };
        let whole = document.render_page(0, 1.0);
        let part = document.render_page_region(0, 1.0, region);
        match (whole, part) {
            (Ok(whole), Ok(part)) => {
                assert_eq!(
                    part.data(),
                    common::crop(&whole, 20, 120, 50, 40).as_slice(),
                    "{entries}"
                );
                common::assert_regions_match(&mut document, 0, 1.0);
            }
            (whole, part) => assert_eq!(whole.err(), part.err(), "{entries}"),
        }
    }
}
