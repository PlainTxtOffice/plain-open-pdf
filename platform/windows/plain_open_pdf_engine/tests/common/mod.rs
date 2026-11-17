//! Checks shared by the test files that render pages.

use plain_open_pdf_engine::{Document, DocumentError, PixelRegion, Pixmap};

/// The error for a region that holds no pixel of the page image.
pub const OUTSIDE: DocumentError = DocumentError::Unsupported("a page region outside the page");

/// The bytes of a rectangle of an image, row by row. The rectangle must lie
/// inside the image.
pub fn crop(image: &Pixmap, left: usize, top: usize, width: usize, height: usize) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(width * height * 4);
    for y in top..top + height {
        let start = (y * image.width() + left) * 4;
        pixels.extend(&image.data()[start..start + width * 4]);
    }
    pixels
}

/// The rectangles a page image of the given size is tried with: the whole
/// image, rectangles inside it and at each edge, single pixels, rectangles
/// that reach past it, and others from a fixed sequence of numbers.
fn regions(width: usize, height: usize) -> Vec<PixelRegion> {
    let region = |left, top, width, height| PixelRegion {
        left,
        top,
        width,
        height,
    };
    let (across, down) = (width.div_ceil(3), height.div_ceil(3));
    let mut regions = vec![
        region(0, 0, width, height),
        // Inside, away from any round position.
        region(width / 7, height / 5, width * 3 / 7 + 1, height * 2 / 5 + 1),
        // Along the left, top, right, and bottom edges.
        region(0, height / 4, across, down),
        region(width / 4, 0, across, down),
        region(width - across, height / 3, across, down),
        region(width / 3, height - down, across, down),
        // Single pixels: the corners and the middle.
        region(0, 0, 1, 1),
        region(width - 1, height - 1, 1, 1),
        region(width / 2, height / 2, 1, 1),
        // Past the right and bottom edges, by a little and by a lot.
        region(width / 2, height / 2, width, height),
        region(width - 1, 0, 5, height + 3),
        region(0, height - 1, usize::MAX, usize::MAX),
        region(width / 3, height / 3, usize::MAX - 1, 2),
    ];
    // A linear congruential sequence, so every run tries the same places.
    let mut state: u64 = 0x2545_F491_4F6C_DD1D ^ ((width as u64) << 32) ^ height as u64;
    let mut next = |limit: usize| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((state >> 33) as usize) % limit
    };
    for _ in 0..16 {
        let (left, top) = (next(width), next(height));
        // Some of these reach past the image.
        regions.push(region(left, top, 1 + next(width), 1 + next(height)));
    }
    regions
}

/// Check that every region of a page's image is drawn exactly as that part
/// of the whole image, and that a region with no pixel of the image is
/// refused.
pub fn assert_regions_match(document: &mut Document, page_index: usize, scale: f64) {
    let whole = document.render_page(page_index, scale).unwrap();
    let (width, height) = (whole.width(), whole.height());
    assert!(width > 0 && height > 0);
    for region in regions(width, height) {
        let part = document
            .render_page_region(page_index, scale, region)
            .unwrap();
        let kept_width = region.width.min(width - region.left);
        let kept_height = region.height.min(height - region.top);
        assert_eq!(
            (part.width(), part.height()),
            (kept_width, kept_height),
            "{region:?} of {width} by {height} at scale {scale}"
        );
        let expected = crop(&whole, region.left, region.top, kept_width, kept_height);
        assert!(
            part.data() == expected.as_slice(),
            "{region:?} of {width} by {height} at scale {scale} differs from the whole image"
        );
    }
    // Drawing regions leaves the whole image as it was.
    assert!(document.render_page(page_index, scale).unwrap() == whole);

    let region = |left, top, width, height| PixelRegion {
        left,
        top,
        width,
        height,
    };
    for outside in [
        region(width, 0, 1, 1),
        region(0, height, 1, 1),
        region(width + 9, height + 9, 4, 4),
        region(usize::MAX, usize::MAX, usize::MAX, usize::MAX),
        region(0, 0, 0, height),
        region(0, 0, width, 0),
        region(1, 1, 0, 0),
    ] {
        assert_eq!(
            document.render_page_region(page_index, scale, outside),
            Err(OUTSIDE),
            "{outside:?}"
        );
    }
}
