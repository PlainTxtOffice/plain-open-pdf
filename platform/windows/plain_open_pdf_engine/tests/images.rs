//! Images drawn on a page, at the public library boundary.

mod common;

use plain_open_pdf_engine::{Document, PixelRegion};

/// A 16 by 16 baseline JPEG: the left half (200, 30, 30) and the right half
/// (30, 60, 200), with the color components at half size.
const COLOR_JPEG: &str = "ffd8ffe000104a46494600010101006000600000ffdb004300030202020202030202020303030304060404040404080606050609080a0a090809090a0c0f0c0a0b0e0b09090d110d0e0f101011100a0c12131210130f101010ffdb00430103030304030408040408100b090b1010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010ffc00011080010001003011100021101031101ffc4001f0000010501010101010100000000000000000102030405060708090a0bffc400b5100002010303020403050504040000017d01020300041105122131410613516107227114328191a1082342b1c11552d1f02433627282090a161718191a25262728292a3435363738393a434445464748494a535455565758595a636465666768696a737475767778797a838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7e8e9eaf1f2f3f4f5f6f7f8f9faffc4001f0100030101010101010101010000000000000102030405060708090a0bffc400b51100020102040403040705040400010277000102031104052131061241510761711322328108144291a1b1c109233352f0156272d10a162434e125f11718191a262728292a35363738393a434445464748494a535455565758595a636465666768696a737475767778797a82838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae2e3e4e5e6e7e8e9eaf2f3f4f5f6f7f8f9faffda000c03010002110311003f00f02afcc8fee3399aff00454ff35ce9abfcea3fd28399aff454ff0035cfffd9";

/// A 16 by 8 gray baseline JPEG whose columns rise from 0 by 16 each.
const GRAY_JPEG: &str = "ffd8ffe000104a46494600010101006000600000ffdb004300030202020202030202020303030304060404040404080606050609080a0a090809090a0c0f0c0a0b0e0b09090d110d0e0f101011100a0c12131210130f101010ffc0000b080008001001011100ffc4001f0000010501010101010100000000000000000102030405060708090a0bffc400b5100002010303020403050504040000017d01020300041105122131410613516107227114328191a1082342b1c11552d1f02433627282090a161718191a25262728292a3435363738393a434445464748494a535455565758595a636465666768696a737475767778797a838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7e8e9eaf1f2f3f4f5f6f7f8f9faffda0008010100003f00f8ff00f66dff00975ff80d7eaafecdbff2ebff0001afffd9";

/// A 16 by 8 gray progressive JPEG made by this repository's own encoder:
/// spectral selection, successive approximation of DC and AC coefficients,
/// and a restart after each block.
const PROGRESSIVE_JPEG: &str = "ffd8ffdb004300030202020202030202020303030304060404040404080606050609080a0a090809090a0c0f0c0a0b0e0b09090d110d0e0f101011100a0c12131210130f101010ffc2000b080008001001011100ffc4001f0000010501010101010100000000000000000102030405060708090a0bffc400b5100002010303020403050504040000017d01020300041105122131410613516107227114328191a1082342b1c11552d1f02433627282090a161718191a25262728292a3435363738393a434445464748494a535455565758595a636465666768696a737475767778797a838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7e8e9eaf1f2f3f4f5f6f7f8f9faffdd00040001ffda0008010100000001f37fffd0f49fffda0008010100010502d1a08fed5fffd0d2d96d2cff00ffda0008010100063f027f27cad8643b7caddb319fe1ddf363ebf8d7ffd06b8332c581f2bb2193e7e8bf28e5f9c71dfa0affda0008010100013f212318bace302c775874ed1eaaffd039ab860cbb8021300041223948afffda0008010100000010ff00ffd07fffda0008010100013f10c5af783d362bc31c4e2e59d7ffd0eb0b7dd46013ca4246b7ffd9";

/// The same quantized coefficients as `PROGRESSIVE_JPEG`, as a baseline JPEG.
const SAME_BASELINE_JPEG: &str = "ffd8ffdb004300030202020202030202020303030304060404040404080606050609080a0a090809090a0c0f0c0a0b0e0b09090d110d0e0f101011100a0c12131210130f101010ffc0000b080008001001011100ffc4001f0000010501010101010100000000000000000102030405060708090a0bffc400b5100002010303020403050504040000017d01020300041105122131410613516107227114328191a1082342b1c11552d1f02433627282090a161718191a25262728292a3435363738393a434445464748494a535455565758595a636465666768696a737475767778797a838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7e8e9eaf1f2f3f4f5f6f7f8f9faffda0008010100003f00f9c3e0d687a73f8aafff00b1d34a7b093519c5aff648b97b1789646d9f67fb5ffa43c4171b04f828b8f332f9afd21f859756fe13f06ddf8827d66d74a78a0ff44bcb9b297504fb63911dbe2da36496f5da678d163465691d962841775affd9";

/// `fax_rows(300, 36)` as Group 4 fax data, made by the Windows TIFF
/// encoder with white as zero.
const FAX_GROUP_4: &str = "23869903030440609cc8e1a640c0c11018273238308fe43d641ba41228b10da0a1105fff89710e819ca80cd2261ae4b41964d41a8a406e6b0e7f22c0d48837ad9fe7829a8361260d224c036905053b0c1906e908350b10da0a20499ffc4e0673a086c35c8986695019ce82221ae309c30440182061a626015448d8341350a6a0d84983491506d20a0a71238304186591b0d52560ae5543ffe4d41c8c02e4083489402a1520641238682ac1a08a036104052b0c190691520da0a414c6c9a8a60358890660320406cc06b1120cc05ffff12802e4083489402a152038064d4341560d04501b082029590d3032060410c1c98e434c0c818104307263c830cb2361aa24602b95506dff0c8668990d72d20ca4b23619641830481970d523619641830481970304d43288d06a09ac1584d40511238351160da214048568bfc9a8172301cd19280d220405c8c073464a03389e0d312280aa26c09c8206ec3064159508141413506a22c1b4428090ad035ff8970d725a0ca4940ae4d434c9482a953027296181238369050532b20b50010010";

/// The same rows as Group 3 one-dimensional fax data with end-of-line codes.
const FAX_GROUP_3: &str = "0011c34c0c0c080c1cc70d3030302030731c1800135fc3d60dd20350b06d0501240009afe1eb06e901a858368280920006c4d033a80cd130d72d065140013506a480deb0ff8b0352406f587fd5800f05a836130691506d05053d4007c3d60dd20350b06d050126000f87ac1ba406a160da0a024c0015019f412c35c4c3354067d04b0d71800270c08030061a6520aaa60d00026a16a0d84c1a4541b41414cb800900c030cb1b0d52b057550e00240300c32c6c354ac15d5438009a838c02e041a4500a8a40c8003a868560d0280d8080a452003506c260d22a0da0a0a6360026a2c06b0906604020316035848330240026a2c06b090660402031603584833024002300b810691402a2901c4c004d4342b0681406c04052580011c34c0c0c080c1cc70d3030302030731c1800106196361aa560aeaa0da0020c32c6c354ac15d541b400540668986b96832928658008d8658306081b86a8d8658306081b818001350ca1a0d42a056540280c00520350b06d050122d1000a406a160da0a0245a20013502e301e8ca0348080b8c07a3280ce002061a6520aaa604e08002341a8540aca8050500026a0d42c1b414048b40d60026a0d42c1b414048b40d6002261ae5a0ca4a0565c004d434ca41554c09d2c300022a0da0a0a6b20b540010010010";

fn unhex(text: &str) -> Vec<u8> {
    let digit = |byte: u8| (byte as char).to_digit(16).expect("hex digit") as u8;
    let pairs = text.as_bytes().chunks(2);
    pairs
        .map(|pair| digit(pair[0]) << 4 | digit(pair[1]))
        .collect()
}

/// A one-page document that draws its first image over the whole page.
/// Images are objects 5 and up, named `Im0` and up.
fn document(width: usize, height: usize, before: &str, images: &[(&str, &[u8])]) -> Document {
    let content = format!("{before} q {width} 0 0 {height} 0 0 cm /Im0 Do Q");
    let names: String = (0..images.len())
        .map(|index| format!("/Im{index} {} 0 R ", index + 5))
        .collect();
    let stream = |entries: &str, data: &[u8]| {
        let head = format!("<< {entries} /Length {} >>\nstream\n", data.len());
        [head.as_bytes(), data, b"\nendstream"].concat()
    };
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Contents 4 0 R \
             /Resources << /XObject << {names}>> >> >>"
        )
        .into_bytes(),
        stream("", content.as_bytes()),
    ];
    for (entries, data) in images {
        objects.push(stream(&format!("/Subtype /Image {entries}"), data));
    }
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
    Document::open(file).unwrap()
}

/// The red, green, and blue of each pixel of the page's top row.
fn top_row(document: &mut Document) -> Vec<[u8; 3]> {
    common::assert_regions_match(document, 0, 1.0);
    let image = document.render_page(0, 1.0).unwrap();
    (0..image.width())
        .map(|x| {
            let pixel = image.pixel(x, 0).unwrap();
            [pixel[0], pixel[1], pixel[2]]
        })
        .collect()
}

const WHITE: [u8; 3] = [255; 3];

/// A rectangle inside the pages that damaged images are drawn on.
const PART: PixelRegion = PixelRegion {
    left: 3,
    top: 2,
    width: 9,
    height: 5,
};

#[test]
fn draws_samples_in_each_color_space() {
    // Rows run from the top of the image; the page shows it upright.
    let rgb = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0];
    let entries = "/Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8";
    let mut drawn = document(2, 2, "", &[(entries, &rgb)]);
    common::assert_regions_match(&mut drawn, 0, 1.0);
    common::assert_regions_match(&mut drawn, 0, 7.3);
    let image = drawn.render_page(0, 1.0).unwrap();
    assert_eq!(image.pixel(0, 0), Some([255, 0, 0, 255]));
    assert_eq!(image.pixel(1, 0), Some([0, 255, 0, 255]));
    assert_eq!(image.pixel(0, 1), Some([0, 0, 255, 255]));
    assert_eq!(image.pixel(1, 1), Some([255, 255, 0, 255]));

    // Two bits for each pixel, each an index into a table of colors.
    let entries = "/Width 4 /Height 1 /BitsPerComponent 2 \
                   /ColorSpace [/Indexed /DeviceRGB 3 <FF000000FF000000FF101010>]";
    let mut drawn = document(4, 1, "", &[(entries, &[0b0001_1011])]);
    assert_eq!(
        top_row(&mut drawn),
        [[255, 0, 0], [0, 255, 0], [0, 0, 255], [16, 16, 16]]
    );

    // Sixteen-bit gray, and one bit for each pixel with a reversed range.
    let entries = "/Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 16";
    let mut drawn = document(2, 1, "", &[(entries, &[0x80, 0, 0xFF, 0xFF])]);
    assert_eq!(top_row(&mut drawn), [[128; 3], WHITE]);
    let entries = "/Width 4 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 1 /Decode [1 0]";
    let mut drawn = document(4, 1, "", &[(entries, &[0b1010_0000])]);
    assert_eq!(top_row(&mut drawn), [[0; 3], WHITE, [0; 3], WHITE]);

    // Inks, stored reversed as Adobe applications store them.
    let entries = "/Width 2 /Height 1 /ColorSpace /DeviceCMYK /BitsPerComponent 8 \
                   /Decode [1 0 1 0 1 0 1 0]";
    let inks = [255, 255, 0, 255, 0, 255, 255, 255];
    let mut drawn = document(2, 1, "", &[(entries, &inks)]);
    assert_eq!(top_row(&mut drawn), [[255, 255, 0], [0, 255, 255]]);

    // Data that ends early reads as zero, and a missing color space or a
    // filter that is not supported draws nothing.
    let entries = "/Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8";
    assert_eq!(
        top_row(&mut document(2, 1, "", &[(entries, &[200])])),
        [[200; 3], [0; 3]]
    );
    let entries = "/Width 2 /Height 1 /BitsPerComponent 8";
    assert_eq!(
        top_row(&mut document(2, 1, "", &[(entries, &[0, 0])])),
        [WHITE; 2]
    );
    let entries =
        "/Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /JPXDecode";
    assert_eq!(
        top_row(&mut document(2, 1, "", &[(entries, &[0, 0])])),
        [WHITE; 2]
    );
}

#[test]
fn stencils_paint_the_fill_color_and_masks_hide_pixels() {
    // A zero bit paints unless the range is reversed.
    let entries = "/Width 4 /Height 1 /ImageMask true";
    let mut drawn = document(4, 1, "0 0 1 rg", &[(entries, &[0b1100_0000])]);
    assert_eq!(
        top_row(&mut drawn),
        [WHITE, WHITE, [0, 0, 255], [0, 0, 255]]
    );
    let entries = "/Width 4 /Height 1 /ImageMask true /Decode [1 0]";
    let mut drawn = document(4, 1, "0 0 1 rg", &[(entries, &[0b1100_0000])]);
    assert_eq!(
        top_row(&mut drawn),
        [[0, 0, 255], [0, 0, 255], WHITE, WHITE]
    );

    // A soft mask gives each pixel its opacity.
    let red = [255, 0, 0, 255, 0, 0, 255, 0, 0];
    let entries = "/Width 3 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask 6 0 R";
    let mask = "/Width 3 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8";
    let mut drawn = document(3, 1, "", &[(entries, &red), (mask, &[255, 128, 0])]);
    assert_eq!(top_row(&mut drawn), [[255, 0, 0], [255, 127, 127], WHITE]);

    // A stencil mask hides the pixels where it has a one, and a mask that
    // names itself ends.
    let entries = "/Width 3 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Mask 6 0 R";
    let mask = "/Width 3 /Height 1 /ImageMask true";
    let mut drawn = document(3, 1, "", &[(entries, &red), (mask, &[0b0100_0000])]);
    assert_eq!(top_row(&mut drawn), [[255, 0, 0], WHITE, [255, 0, 0]]);
    let entries = "/Width 3 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask 5 0 R";
    let _ = top_row(&mut document(3, 1, "", &[(entries, &red)]));
}

#[test]
fn images_are_placed_scaled_and_clipped() {
    // A large image drawn small is averaged, not sampled here and there.
    let stripes: Vec<u8> = (0..64).map(|x| if x % 2 == 0 { 0 } else { 255 }).collect();
    let entries = "/Width 64 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8";
    let mut drawn = document(4, 1, "", &[(entries, &stripes)]);
    for pixel in top_row(&mut drawn) {
        assert!((120..=135).contains(&pixel[0]), "{pixel:?}");
    }
    // An image drawn large keeps hard edges between its pixels.
    let entries = "/Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8";
    let mut drawn = document(8, 1, "", &[(entries, &[0, 255])]);
    assert_eq!(top_row(&mut drawn)[3], [0; 3]);
    assert_eq!(top_row(&mut drawn)[4], WHITE);
    // A clip and an opacity apply to an image as to anything else.
    let before = "/Missing gs 0 0 4 1 re W n";
    let mut drawn = document(8, 1, before, &[(entries, &[0, 0])]);
    assert_eq!(top_row(&mut drawn)[3], [0; 3]);
    assert_eq!(top_row(&mut drawn)[4], WHITE);
}

#[test]
fn opaque_image_pixels_average_their_samples_exactly() {
    // Drawn three pixels wide, the middle pixel takes two red samples and
    // two blue ones: halfway, rounded up.
    let entries = "/Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8";
    let red_blue = [255, 0, 0, 0, 0, 255];
    let mut drawn = document(3, 1, "", &[(entries, &red_blue)]);
    assert_eq!(
        top_row(&mut drawn),
        [[255, 0, 0], [128, 0, 128], [0, 0, 255]]
    );
    // A clip edge through the middle pixel lets the white page show there.
    let mut drawn = document(3, 1, "0 0 1.5 1 re W n", &[(entries, &red_blue)]);
    let row = top_row(&mut drawn);
    assert_eq!(row[0], [255, 0, 0]);
    assert!(
        (150..230).contains(&row[1][0]) && (100..160).contains(&row[1][1]),
        "{:?}",
        row[1]
    );
    assert_eq!(row[2], WHITE);
}

#[test]
fn decodes_baseline_jpeg_images() {
    let near = |pixel: [u8; 4], expected: [u8; 3]| {
        let close = pixel.iter().zip(expected).all(|(&a, b)| a.abs_diff(b) <= 6);
        assert!(close, "{pixel:?} is not near {expected:?}");
    };
    let entries =
        "/Width 16 /Height 16 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode";
    let mut drawn = document(16, 16, "", &[(entries, &unhex(COLOR_JPEG))]);
    common::assert_regions_match(&mut drawn, 0, 1.0);
    common::assert_regions_match(&mut drawn, 0, 2.6);
    let image = drawn.render_page(0, 1.0).unwrap();
    for y in [0, 7, 15] {
        near(image.pixel(2, y).unwrap(), [200, 30, 30]);
        near(image.pixel(6, y).unwrap(), [200, 30, 30]);
        near(image.pixel(9, y).unwrap(), [30, 60, 200]);
        near(image.pixel(13, y).unwrap(), [30, 60, 200]);
    }

    let entries =
        "/Width 16 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode";
    let mut drawn = document(16, 8, "", &[(entries, &unhex(GRAY_JPEG))]);
    common::assert_regions_match(&mut drawn, 0, 1.0);
    let image = drawn.render_page(0, 1.0).unwrap();
    for x in 0..16 {
        let level = (x * 16) as u8;
        near(image.pixel(x, 4).unwrap(), [level; 3]);
    }
    // The stream decodes to one byte for each sample.
    let reference = plain_open_pdf_engine::ObjectReference {
        number: 5,
        generation: 0,
    };
    assert_eq!(drawn.stream(reference).unwrap().unwrap().len(), 16 * 8);
}

#[test]
fn decodes_progressive_jpeg_images() {
    let entries =
        "/Width 16 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode";
    let mut progressive = document(16, 8, "", &[(entries, &unhex(PROGRESSIVE_JPEG))]);
    let mut baseline = document(16, 8, "", &[(entries, &unhex(SAME_BASELINE_JPEG))]);
    let reference = plain_open_pdf_engine::ObjectReference {
        number: 5,
        generation: 0,
    };
    let progressive = progressive.stream(reference).unwrap().unwrap();
    let baseline = baseline.stream(reference).unwrap().unwrap();
    // The scans add up to the same coefficients, so the samples are the
    // same, and close to what libjpeg makes of them.
    assert_eq!(progressive, baseline);
    let libjpeg = [
        1, 32, 27, 27, 76, 92, 84, 99, 147, 149, 144, 172, 211, 207, 207, 247,
    ];
    for (ours, theirs) in progressive.iter().zip(libjpeg) {
        assert!(ours.abs_diff(theirs) <= 2, "{ours} is not near {theirs}");
    }

    // Damaged progressive data never panics.
    let whole = unhex(PROGRESSIVE_JPEG);
    for index in (0..whole.len()).step_by(2) {
        let mut cut = document(16, 8, "", &[(entries, &whole[..index])]);
        let _ = cut.render_page(0, 1.0);
        let _ = cut.render_page_region(0, 1.0, PART);
        let mut damaged = whole.clone();
        damaged[index] ^= 0x5A;
        let mut damaged = document(16, 8, "", &[(entries, &damaged)]);
        let _ = damaged.render_page(0, 1.0);
        let _ = damaged.render_page_region(0, 1.0, PART);
    }
}

#[test]
fn decodes_jpeg_images_shown_small_at_a_smaller_size() {
    let near = |pixel: [u8; 4], expected: [u8; 3]| {
        let close = pixel.iter().zip(expected).all(|(&a, b)| a.abs_diff(b) <= 8);
        assert!(close, "{pixel:?} is not near {expected:?}");
    };
    // At a quarter of its size, each block gives two samples across and
    // down, and the halves keep their colors.
    let entries =
        "/Width 16 /Height 16 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode";
    let mut drawn = document(16, 16, "", &[(entries, &unhex(COLOR_JPEG))]);
    common::assert_regions_match(&mut drawn, 0, 0.25);
    let image = drawn.render_page(0, 0.25).unwrap();
    assert_eq!((image.width(), image.height()), (4, 4));
    for y in 0..4 {
        near(image.pixel(0, y).unwrap(), [200, 30, 30]);
        near(image.pixel(3, y).unwrap(), [30, 60, 200]);
    }

    // Columns rising by 16: at half size each pixel stands for two.
    let gray = "/Width 16 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode";
    let mut drawn = document(16, 8, "", &[(gray, &unhex(GRAY_JPEG))]);
    common::assert_regions_match(&mut drawn, 0, 0.5);
    let image = drawn.render_page(0, 0.5).unwrap();
    for x in 0..8 {
        near(image.pixel(x, 2).unwrap(), [(x * 32 + 8) as u8; 3]);
    }

    // The same coefficients give the same smaller image, whether they come
    // in progressive scans or in one baseline scan.
    let mut progressive = document(16, 8, "", &[(gray, &unhex(PROGRESSIVE_JPEG))]);
    let mut baseline = document(16, 8, "", &[(gray, &unhex(SAME_BASELINE_JPEG))]);
    assert_eq!(
        progressive.render_page(0, 0.5).unwrap(),
        baseline.render_page(0, 0.5).unwrap()
    );
}

#[test]
fn drawing_again_reuses_decoded_images_exactly() {
    // Drawn again, at the same size or after another, a page is the same.
    let entries =
        "/Width 16 /Height 16 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode";
    let mut drawn = document(16, 16, "", &[(entries, &unhex(COLOR_JPEG))]);
    let small = drawn.render_page(0, 0.25).unwrap();
    let large = drawn.render_page(0, 1.0).unwrap();
    assert_eq!(drawn.render_page(0, 0.25).unwrap(), small);
    assert_eq!(drawn.render_page(0, 1.0).unwrap(), large);

    // A stencil drawn in two colors paints each.
    let entries = "/Width 1 /Height 1 /ImageMask true";
    let before = "1 0 0 rg q 1 0 0 1 0 0 cm /Im0 Do Q 0 0 1 rg";
    let mut drawn = document(2, 2, before, &[(entries, &[0])]);
    assert_eq!(top_row(&mut drawn), [[0, 0, 255], [0, 0, 255]]);

    // Two files' images with the same object number stay apart.
    let gray = "/Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8";
    let mut dark = document(2, 1, "", &[(gray, &[0, 0])]);
    let mut light = document(2, 1, "", &[(gray, &[200, 200])]);
    assert_eq!(top_row(&mut dark), [[0; 3]; 2]);
    assert_eq!(top_row(&mut light), [[200; 3]; 2]);
    assert_eq!(top_row(&mut dark), [[0; 3]; 2]);

    // An image changed in the document is drawn as it now is.
    let mut changed = document(2, 1, "", &[(gray, &[0, 255])]);
    assert_eq!(top_row(&mut changed), [[0; 3], WHITE]);
    let reference = plain_open_pdf_engine::ObjectReference {
        number: 5,
        generation: 0,
    };
    let mut dictionary = plain_open_pdf_engine::Dictionary::new();
    for (key, value) in [
        (
            "Type",
            plain_open_pdf_engine::PdfObject::Name(b"XObject".to_vec()),
        ),
        (
            "Subtype",
            plain_open_pdf_engine::PdfObject::Name(b"Image".to_vec()),
        ),
        ("Width", plain_open_pdf_engine::PdfObject::Integer(2)),
        ("Height", plain_open_pdf_engine::PdfObject::Integer(1)),
        (
            "ColorSpace",
            plain_open_pdf_engine::PdfObject::Name(b"DeviceGray".to_vec()),
        ),
        (
            "BitsPerComponent",
            plain_open_pdf_engine::PdfObject::Integer(8),
        ),
    ] {
        dictionary.insert(key.as_bytes().to_vec(), value);
    }
    changed.set_stream_data(reference, dictionary, &[255, 0]);
    assert_eq!(top_row(&mut changed), [WHITE, [0; 3]]);
}

/// Rows of runs of many lengths, black where true; some rows repeat the
/// one above, as two-dimensional coding favors.
fn fax_rows(width: usize, height: usize) -> Vec<Vec<bool>> {
    let mut lengths: Vec<usize> = (1..64).collect();
    lengths.extend([
        64, 65, 127, 128, 200, 300, 640, 1000, 1700, 1728, 1792, 1900, 2100, 2500, 2560,
    ]);
    let mut rows: Vec<Vec<bool>> = (0..height)
        .map(|y| {
            let mut row = Vec::new();
            let mut black = y % 3 == 1;
            let mut index = y;
            while row.len() < width {
                let length = if y % 4 != 0 {
                    lengths[index % lengths.len()]
                } else {
                    ((index * 7 + y) % 40).max(1)
                };
                row.extend(std::iter::repeat_n(black, length));
                black = !black;
                index += 5;
            }
            row.truncate(width);
            row
        })
        .collect();
    for y in (2..height).step_by(5) {
        rows[y] = rows[y - 1].clone();
    }
    rows
}

#[test]
fn decodes_group_3_and_group_4_fax_images() {
    let rows = fax_rows(300, 36);
    let packed = |black_is_1: bool| -> Vec<u8> {
        rows.iter()
            .flat_map(|row| {
                let mut bytes = vec![0_u8; 38];
                for (x, &black) in row.iter().enumerate() {
                    if black == black_is_1 {
                        bytes[x / 8] |= 0x80 >> (x % 8);
                    }
                }
                bytes
            })
            .collect()
    };
    let reference = plain_open_pdf_engine::ObjectReference {
        number: 5,
        generation: 0,
    };
    let decode = |data: &str, parameters: &str| {
        let entries = format!(
            "/Width 300 /Height 36 /ColorSpace /DeviceGray /BitsPerComponent 1              /Filter /CCITTFaxDecode /DecodeParms << {parameters} >>"
        );
        let mut drawn = document(300, 36, "", &[(&entries, &unhex(data))]);
        drawn.stream(reference).unwrap().unwrap()
    };
    let group_4 = "/K -1 /Columns 300 /Rows 36";
    assert_eq!(
        decode(FAX_GROUP_4, &format!("{group_4} /BlackIs1 true")),
        packed(true)
    );
    // Black is 0 unless the parameters say otherwise.
    assert_eq!(decode(FAX_GROUP_4, group_4), packed(false));
    // Without a row count, the data ends the image.
    assert_eq!(
        decode(FAX_GROUP_4, "/K -1 /Columns 300 /BlackIs1 true"),
        packed(true)
    );
    let group_3 = "/K 0 /EndOfLine true /Columns 300 /Rows 36 /BlackIs1 true";
    assert_eq!(decode(FAX_GROUP_3, group_3), packed(true));

    // Rows the data does not reach are white, and damaged data never
    // panics.
    let whole = unhex(FAX_GROUP_4);
    let short = hex(&whole[..whole.len() / 2]);
    let partial = decode(&short, &format!("{group_4} /BlackIs1 true"));
    assert_eq!(partial.len(), 38 * 36);
    assert!(partial[38 * 35..].iter().all(|&byte| byte == 0));
    for index in (0..whole.len()).step_by(3) {
        let mut damaged = whole.clone();
        damaged[index] ^= 0x5A;
        let _ = decode(&hex(&damaged), group_4);
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn damaged_jpeg_images_never_panic() {
    let entries =
        "/Width 16 /Height 16 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode";
    let whole = unhex(COLOR_JPEG);
    let render = |data: &[u8]| {
        let mut damaged = document(16, 16, "", &[(entries, data)]);
        let _ = damaged.render_page(0, 1.0);
        let _ = damaged.render_page_region(0, 1.0, PART);
    };
    for length in (0..whole.len()).step_by(3) {
        render(&whole[..length]);
    }
    for index in 0..whole.len() {
        for change in [0x01, 0x10, 0xFF] {
            let mut damaged = whole.clone();
            damaged[index] ^= change;
            render(&damaged);
        }
    }
}
