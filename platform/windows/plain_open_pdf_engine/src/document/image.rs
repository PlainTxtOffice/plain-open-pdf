//! Image XObjects: decoding their samples to RGBA pixels (ISO 32000-2
//! sections 8.6 and 8.9).

use super::DocumentError;
use super::reader::{Reader, entry, size_entry};
use crate::raster::{to_byte, to_byte_f64};
use crate::{Dictionary, ObjectReference, PdfObject};
use std::sync::{Arc, Mutex, PoisonError, Weak};

/// The most pixels an image may have.
const MAX_PIXELS: usize = 100_000_000;
/// How deeply color spaces and masks may refer to one another.
const MAX_DEPTH: usize = 8;

/// A decoded image: rows from the top, four bytes for each pixel, with
/// colors that are not multiplied by the alpha.
pub(super) struct Image {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// How an image's samples stand for colors.
enum Space {
    Gray,
    Rgb,
    Cmyk,
    /// The lightness of a Lab color is used as a gray; its range is 0 to
    /// 100.
    Lab,
    /// Amounts of ink, as in Separation and DeviceN spaces, shown as gray:
    /// more is darker.
    Tint(usize),
    /// Indexes into a table of colors in a base space.
    Indexed(Box<Space>, Vec<u8>),
}

impl Space {
    fn components(&self) -> usize {
        match self {
            Self::Gray | Self::Indexed(..) => 1,
            Self::Rgb | Self::Lab => 3,
            Self::Cmyk => 4,
            Self::Tint(count) => *count,
        }
    }

    /// The red, green, and blue of a color given as components from 0 to 1.
    fn color(&self, components: &[f32]) -> [f32; 3] {
        let at = |index: usize| components.get(index).copied().unwrap_or(0.0);
        match self {
            Self::Gray => [at(0); 3],
            Self::Rgb => [at(0), at(1), at(2)],
            Self::Cmyk => [at(0), at(1), at(2)].map(|ink| (1.0 - ink) * (1.0 - at(3))),
            Self::Lab => [at(0); 3],
            Self::Tint(_) => {
                let ink: f32 = components.iter().map(|tint| tint.clamp(0.0, 1.0)).sum();
                [1.0 - ink.min(1.0); 3]
            }
            // An index is not a fraction; `load` looks it up instead.
            Self::Indexed(base, _) => base.color(components),
        }
    }
}

/// Read a color space from its name or array form.
fn color_space(
    reader: &mut Reader,
    object: &PdfObject,
    resources: &Dictionary,
    depth: usize,
) -> Result<Option<Space>, DocumentError> {
    if depth > MAX_DEPTH {
        return Ok(None);
    }
    let object = reader.resolve(object)?;
    let (family, parts): (&[u8], &[PdfObject]) = match &object {
        PdfObject::Name(name) => (name, &[]),
        PdfObject::Array(parts) => match parts.first().and_then(PdfObject::as_name) {
            Some(name) => (name, &parts[1..]),
            None => return Ok(None),
        },
        _ => return Ok(None),
    };
    Ok(match family {
        b"DeviceGray" | b"G" | b"CalGray" => Some(Space::Gray),
        b"DeviceRGB" | b"RGB" | b"CalRGB" => Some(Space::Rgb),
        b"DeviceCMYK" | b"CMYK" => Some(Space::Cmyk),
        b"Lab" => Some(Space::Lab),
        // An ICC profile is not applied; its component count picks a
        // device space.
        b"ICCBased" => {
            let profile = match parts.first() {
                Some(profile) => reader.resolve(profile)?,
                None => PdfObject::Null,
            };
            let count = profile
                .as_dictionary()
                .and_then(|profile| entry(profile, "N"))
                .and_then(PdfObject::as_integer);
            match count {
                Some(1) => Some(Space::Gray),
                Some(4) => Some(Space::Cmyk),
                _ => Some(Space::Rgb),
            }
        }
        b"Indexed" | b"I" => {
            let Some(base) = parts.first() else {
                return Ok(None);
            };
            let Some(base) = color_space(reader, base, resources, depth + 1)? else {
                return Ok(None);
            };
            // The table is a string, or a stream of the same bytes.
            let table = match parts.get(2) {
                Some(PdfObject::Reference(table)) => match reader.stream(table.number)? {
                    Some(data) => data,
                    None => match reader.resolve(&PdfObject::Reference(*table))? {
                        PdfObject::String(data) => data,
                        _ => Vec::new(),
                    },
                },
                Some(PdfObject::String(data)) => data.clone(),
                _ => Vec::new(),
            };
            Some(Space::Indexed(Box::new(base), table))
        }
        b"Separation" => Some(Space::Tint(1)),
        b"DeviceN" => {
            let names = match parts.first() {
                Some(names) => reader.resolve(names)?,
                None => PdfObject::Null,
            };
            Some(Space::Tint(
                names.as_array().map_or(1, <[PdfObject]>::len).max(1),
            ))
        }
        b"Pattern" => None,
        // Any other name is looked up among the resources.
        name if parts.is_empty() => match reader.resolve_entry(resources, "ColorSpace")? {
            PdfObject::Dictionary(spaces) => match spaces.get(name) {
                Some(space) => color_space(reader, space, resources, depth + 1)?,
                None => None,
            },
            _ => None,
        },
        _ => None,
    })
}

/// The samples of an image as fractions from 0 to 1, row by row, with
/// `count` of them for each pixel. Missing data reads as zero.
struct Samples<'a> {
    data: &'a [u8],
    bits: usize,
    /// Bytes in each row, which starts on a byte boundary.
    row_bytes: usize,
    count: usize,
}

impl Samples<'_> {
    /// One sample as the whole number the data holds.
    fn raw(&self, x: usize, y: usize, component: usize) -> u32 {
        let index = x * self.count + component;
        let row = y * self.row_bytes;
        let byte = |offset: usize| u32::from(self.data.get(row + offset).copied().unwrap_or(0));
        match self.bits {
            8 => byte(index),
            16 => byte(index * 2) << 8 | byte(index * 2 + 1),
            bits => {
                let position = index * bits;
                let shift = 8 - bits - position % 8;
                byte(position / 8) >> shift & ((1 << bits) - 1)
            }
        }
    }
}

/// Decode the image XObject with the given object number.
///
/// `fill` is the color an image mask paints with. An image that cannot be
/// drawn, such as one in a filter that is not supported, yields `None`.
///
/// `shown` is how many device pixels the image covers across and down,
/// when it is known. A JPEG image with at least twice as many pixels as
/// that is decoded at a half, a quarter, or an eighth of its size, which
/// is still at least the size it is shown at.
pub(super) fn load(
    reader: &mut Reader,
    number: u32,
    resources: &Dictionary,
    fill: [f64; 3],
    depth: usize,
    shown: Option<(f64, f64)>,
) -> Result<Option<Image>, DocumentError> {
    if depth > MAX_DEPTH {
        return Ok(None);
    }
    let PdfObject::Dictionary(image) =
        reader.resolve(&PdfObject::Reference(crate::ObjectReference {
            number,
            generation: 0,
        }))?
    else {
        return Ok(None);
    };
    let (Some(width), Some(height)) = (size_entry(&image, "Width"), size_entry(&image, "Height"))
    else {
        return Ok(None);
    };
    if width == 0 || height == 0 || width.saturating_mul(height) > MAX_PIXELS {
        return Ok(None);
    }
    let reduce = reduction(reader, &image, width, height, shown)?;
    let data = match reader.stream_reduced(number, reduce) {
        Ok(Some(data)) => data,
        Ok(None) | Err(DocumentError::Unsupported(_) | DocumentError::Stream { .. }) => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    decode(reader, &image, &data, resources, fill, depth, reduce)
}

/// What a JPEG image is decoded at: its size divided by 1, 2, 4, or 8,
/// whichever keeps it at least as large as the `shown` pixels it covers.
fn reduction(
    reader: &mut Reader,
    image: &Dictionary,
    width: usize,
    height: usize,
    shown: Option<(f64, f64)>,
) -> Result<usize, DocumentError> {
    let last_filter = match reader.resolve_entry(image, "Filter")? {
        PdfObject::Name(name) => Some(name),
        PdfObject::Array(names) => names
            .last()
            .and_then(PdfObject::as_name)
            .map(<[u8]>::to_vec),
        _ => None,
    };
    let jpeg = matches!(last_filter.as_deref(), Some(b"DCTDecode" | b"DCT"));
    Ok(match shown {
        Some((across, down)) if jpeg => {
            let surplus = (width as f64 / across.max(1e-6)).min(height as f64 / down.max(1e-6));
            [8, 4, 2]
                .into_iter()
                .find(|&reduce| surplus >= reduce as f64)
                .unwrap_or(1)
        }
        _ => 1,
    })
}

/// The most bytes of decoded images kept for drawing them again, across
/// every document.
const CACHE_BYTES: usize = 256 << 20;

/// The most decoded images kept, which bounds the work of finding one.
const CACHE_ENTRIES: usize = 1024;

/// What a decoded image is kept by: the file it comes from, by the address
/// of its bytes; its object number; what its size was divided by; and, for
/// a stencil, the color it paints in.
#[derive(Clone, Copy, PartialEq, Eq)]
struct CacheKey {
    file: usize,
    number: u32,
    reduce: usize,
    paint: Option<[u8; 3]>,
}

struct Cached {
    key: CacheKey,
    /// The file's bytes, which keep their address from being reused while
    /// this is held, and tell when no document holds them any more.
    file: Weak<Vec<u8>>,
    image: Arc<Image>,
    used: u64,
}

struct ImageCache {
    entries: Vec<Cached>,
    clock: u64,
}

/// Decoded images, so that drawing a page again, at any size and on any
/// thread, need not decode them again.
static IMAGE_CACHE: Mutex<ImageCache> = Mutex::new(ImageCache {
    entries: Vec::new(),
    clock: 0,
});

/// Decode the image XObject with the given object number for drawing, as
/// `load` does, keeping it to draw again.
///
/// A document with objects set since it was opened draws without the
/// cache, and so does an image whose color space names a resource, which
/// can differ from page to page.
pub(super) fn load_for_drawing(
    reader: &mut Reader,
    number: u32,
    resources: &Dictionary,
    fill: [f64; 3],
    shown: Option<(f64, f64)>,
) -> Result<Option<Arc<Image>>, DocumentError> {
    // The checks only decide whether to use the cache; what they cannot
    // read, `load` reports or passes over as it always has.
    let key = cache_key(reader, number, fill, shown).unwrap_or(None);
    if let Some(key) = key
        && let Some(image) = cached(key)
    {
        return Ok(Some(image));
    }
    let image = load(reader, number, resources, fill, 0, shown)?.map(Arc::new);
    if let (Some(key), Some(image)) = (key, &image) {
        keep(key, Arc::downgrade(reader.file()), image);
    }
    Ok(image)
}

fn cache_key(
    reader: &mut Reader,
    number: u32,
    fill: [f64; 3],
    shown: Option<(f64, f64)>,
) -> Result<Option<CacheKey>, DocumentError> {
    if reader.is_modified() {
        return Ok(None);
    }
    let reference = PdfObject::Reference(ObjectReference {
        number,
        generation: 0,
    });
    let PdfObject::Dictionary(image) = reader.resolve(&reference)? else {
        return Ok(None);
    };
    let (Some(width), Some(height)) = (size_entry(&image, "Width"), size_entry(&image, "Height"))
    else {
        return Ok(None);
    };
    let stencil = matches!(
        reader.resolve_entry(&image, "ImageMask")?,
        PdfObject::Boolean(true)
    );
    // A stencil reads nothing but its bits. Otherwise the image's color
    // space and its soft mask's must not depend on the page's resources,
    // and a mask that is an image of its own, not a stencil, is not
    // cached at all.
    if !stencil {
        if let Some(space) = entry(&image, "ColorSpace")
            && names_a_resource(reader, space, 0)?
        {
            return Ok(None);
        }
        if let Some(mask) = entry(&image, "SMask") {
            let mask = reader.resolve(mask)?;
            if let Some(space) = mask
                .as_dictionary()
                .and_then(|mask| entry(mask, "ColorSpace"))
                && names_a_resource(reader, space, 0)?
            {
                return Ok(None);
            }
        }
        if let Some(mask @ PdfObject::Reference(_)) = entry(&image, "Mask") {
            let mask = reader.resolve(mask)?;
            let Some(mask) = mask.as_dictionary() else {
                return Ok(None);
            };
            if !matches!(
                reader.resolve_entry(mask, "ImageMask")?,
                PdfObject::Boolean(true)
            ) {
                return Ok(None);
            }
        }
    }
    let paint =
        stencil.then(|| fill.map(|component| to_byte_f64(component.clamp(0.0, 1.0) * 255.0)));
    Ok(Some(CacheKey {
        file: Arc::as_ptr(reader.file()) as usize,
        number,
        reduce: reduction(reader, &image, width, height, shown)?,
        paint,
    }))
}

/// Whether reading a color space may look a name up among the resources.
fn names_a_resource(
    reader: &mut Reader,
    space: &PdfObject,
    depth: usize,
) -> Result<bool, DocumentError> {
    if depth > MAX_DEPTH {
        return Ok(true);
    }
    let space = reader.resolve(space)?;
    let (family, parts): (&[u8], &[PdfObject]) = match &space {
        PdfObject::Name(name) => (name, &[]),
        PdfObject::Array(parts) => match parts.first().and_then(PdfObject::as_name) {
            Some(name) => (name, &parts[1..]),
            None => return Ok(false),
        },
        _ => return Ok(false),
    };
    Ok(match family {
        b"DeviceGray" | b"G" | b"CalGray" | b"DeviceRGB" | b"RGB" | b"CalRGB" | b"DeviceCMYK"
        | b"CMYK" | b"Lab" | b"ICCBased" | b"Separation" | b"DeviceN" | b"Pattern" => false,
        b"Indexed" | b"I" => match parts.first() {
            Some(base) => names_a_resource(reader, base, depth + 1)?,
            None => false,
        },
        _ => parts.is_empty(),
    })
}

/// The kept image for a key, if a document still holds its file.
fn cached(key: CacheKey) -> Option<Arc<Image>> {
    let mut cache = IMAGE_CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    cache.clock += 1;
    let clock = cache.clock;
    let found = cache
        .entries
        .iter_mut()
        .find(|cached| cached.key == key && cached.file.strong_count() > 0)?;
    found.used = clock;
    Some(Arc::clone(&found.image))
}

/// Keep a decoded image, dropping images of files no document holds, then
/// the least recently used ones, to stay within `CACHE_BYTES`.
fn keep(key: CacheKey, file: Weak<Vec<u8>>, image: &Arc<Image>) {
    let size = image.rgba.len();
    if size > CACHE_BYTES / 2 {
        return;
    }
    let mut cache = IMAGE_CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    cache.clock += 1;
    let clock = cache.clock;
    cache
        .entries
        .retain(|cached| cached.key != key && cached.file.strong_count() > 0);
    cache.entries.push(Cached {
        key,
        file,
        image: Arc::clone(image),
        used: clock,
    });
    let mut total: usize = cache
        .entries
        .iter()
        .map(|cached| cached.image.rgba.len())
        .sum();
    while total > CACHE_BYTES || cache.entries.len() > CACHE_ENTRIES {
        let Some(oldest) = cache
            .entries
            .iter()
            .enumerate()
            .min_by_key(|(_, cached)| cached.used)
            .map(|(index, _)| index)
        else {
            break;
        };
        total -= cache.entries.swap_remove(oldest).image.rgba.len();
    }
}

/// Drop the kept images of a file, when its last document goes away, and of
/// any other file no document holds.
pub(super) fn forget_file(file: usize) {
    let mut cache = IMAGE_CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    cache
        .entries
        .retain(|cached| cached.key.file != file && cached.file.strong_count() > 0);
}

/// The full key an inline image's dictionary abbreviates (ISO 32000-2
/// section 8.9.7, table 91).
fn inline_key(key: &[u8]) -> &[u8] {
    match key {
        b"BPC" => b"BitsPerComponent",
        b"CS" => b"ColorSpace",
        b"D" => b"Decode",
        b"DP" => b"DecodeParms",
        b"F" => b"Filter",
        b"H" => b"Height",
        b"IM" => b"ImageMask",
        b"I" => b"Interpolate",
        b"L" => b"Length",
        b"W" => b"Width",
        other => other,
    }
}

/// The full name of a filter an inline image abbreviates (table 92).
fn inline_filter(filter: &PdfObject) -> PdfObject {
    let full = |name: &[u8]| -> Vec<u8> {
        match name {
            b"AHx" => b"ASCIIHexDecode".to_vec(),
            b"A85" => b"ASCII85Decode".to_vec(),
            b"LZW" => b"LZWDecode".to_vec(),
            b"Fl" => b"FlateDecode".to_vec(),
            b"RL" => b"RunLengthDecode".to_vec(),
            b"CCF" => b"CCITTFaxDecode".to_vec(),
            b"DCT" => b"DCTDecode".to_vec(),
            other => other.to_vec(),
        }
    };
    match filter {
        PdfObject::Name(name) => PdfObject::Name(full(name)),
        PdfObject::Array(names) => PdfObject::Array(names.iter().map(inline_filter).collect()),
        other => other.clone(),
    }
}

/// Decode an inline image from the operands of its `BI` operation: its
/// dictionary's keys and values, abbreviated or not, then its data.
pub(super) fn load_inline(
    reader: &mut Reader,
    operands: &[PdfObject],
    resources: &Dictionary,
    fill: [f64; 3],
) -> Result<Option<Image>, DocumentError> {
    let Some((PdfObject::String(data), entries)) = operands.split_last() else {
        return Ok(None);
    };
    let mut image = Dictionary::new();
    for pair in entries.as_chunks::<2>().0 {
        let PdfObject::Name(key) = &pair[0] else {
            continue;
        };
        let key = inline_key(key);
        let value = if key == b"Filter" {
            inline_filter(&pair[1])
        } else {
            pair[1].clone()
        };
        image.insert(key.to_vec(), value);
    }
    let (Some(width), Some(height)) = (size_entry(&image, "Width"), size_entry(&image, "Height"))
    else {
        return Ok(None);
    };
    if width == 0 || height == 0 || width.saturating_mul(height) > MAX_PIXELS {
        return Ok(None);
    }
    let data = match reader.decode_stream(&image, data.clone(), 0) {
        Ok(data) => data,
        Err(DocumentError::Unsupported(_) | DocumentError::Stream { .. }) => return Ok(None),
        Err(error) => return Err(error),
    };
    decode(reader, &image, &data, resources, fill, 0, 1)
}

/// Turn an image's decoded samples into pixels, as its dictionary says.
/// `reduce` is what its size was divided by, rounding up, in decoding.
fn decode(
    reader: &mut Reader,
    image: &Dictionary,
    data: &[u8],
    resources: &Dictionary,
    fill: [f64; 3],
    depth: usize,
    reduce: usize,
) -> Result<Option<Image>, DocumentError> {
    let (Some(width), Some(height)) = (size_entry(image, "Width"), size_entry(image, "Height"))
    else {
        return Ok(None);
    };
    let (width, height) = (width.div_ceil(reduce), height.div_ceil(reduce));
    let numbers = |reader: &mut Reader, key: &str| -> Result<Vec<f32>, DocumentError> {
        let values = reader.resolve_entry(image, key)?;
        let values = values.as_array().unwrap_or_default();
        Ok(values
            .iter()
            .filter_map(|value| value.as_number().map(|value| value as f32))
            .collect())
    };
    let decode = numbers(reader, "Decode")?;
    let mut rgba = vec![255; width * height * 4];

    let is_mask = matches!(
        reader.resolve_entry(image, "ImageMask")?,
        PdfObject::Boolean(true)
    );
    if is_mask {
        // A stencil: one bit for each pixel says whether to paint the fill
        // color there. Zero paints, unless `/Decode` is `[1 0]`.
        let samples = Samples {
            data,
            bits: 1,
            row_bytes: width.div_ceil(8),
            count: 1,
        };
        let paints = u32::from(decode.first().is_some_and(|&first| first >= 0.5));
        let color = fill.map(|component| to_byte_f64(component.clamp(0.0, 1.0) * 255.0));
        for (index, pixel) in rgba.chunks_mut(4).enumerate() {
            let painted = samples.raw(index % width, index / width, 0) == paints;
            pixel[..3].copy_from_slice(&color);
            pixel[3] = if painted { 255 } else { 0 };
        }
        return Ok(Some(Image {
            width,
            height,
            rgba,
        }));
    }

    let bits = size_entry(image, "BitsPerComponent").unwrap_or(8);
    if !matches!(bits, 1 | 2 | 4 | 8 | 16) {
        return Ok(None);
    }
    let Some(space) = (match entry(image, "ColorSpace") {
        Some(space) => color_space(reader, space, resources, 0)?,
        None => None,
    }) else {
        return Ok(None);
    };
    let count = space.components();
    let samples = Samples {
        data,
        bits,
        row_bytes: (width * count * bits).div_ceil(8),
        count,
    };
    let highest = ((1_u64 << bits) - 1) as f32;
    // One component's value from the number its sample holds.
    let value_of = |component: usize, raw: u32| -> f32 {
        let fraction = raw as f32 / highest;
        // Lightness runs from 0 to 100 whatever the range says.
        if matches!(space, Space::Lab) {
            return fraction;
        }
        // `/Decode` gives the range each component is spread over.
        match (decode.get(component * 2), decode.get(component * 2 + 1)) {
            (Some(&low), Some(&high)) => low + fraction * (high - low),
            _ => fraction,
        }
    };
    // The sample picks an entry of an indexed space's table, which holds one
    // byte for each component of the base space.
    let indexed_color = |raw: u32| -> Option<[f32; 3]> {
        let Space::Indexed(base, table) = &space else {
            return None;
        };
        let size = base.components();
        let start = raw as usize * size;
        let entry: Vec<f32> = (0..size)
            .map(|offset| f32::from(table.get(start + offset).copied().unwrap_or(0)) / 255.0)
            .collect();
        Some(base.color(&entry))
    };
    let to_pixel =
        |color: [f32; 3]| color.map(|component| to_byte(component.clamp(0.0, 1.0) * 255.0));
    // With few possible sample values, what each one gives is worked out
    // once: the pixel itself for a single component or an index, and each
    // component's value otherwise.
    let few = bits <= 8;
    let pixels: Vec<[u8; 3]> = if few && count == 1 {
        (0..1_u32 << bits)
            .map(|raw| {
                to_pixel(indexed_color(raw).unwrap_or_else(|| space.color(&[value_of(0, raw)])))
            })
            .collect()
    } else {
        Vec::new()
    };
    let values: Vec<Vec<f32>> = if few && count > 1 {
        (0..count)
            .map(|component| {
                (0..1_u32 << bits)
                    .map(|raw| value_of(component, raw))
                    .collect()
            })
            .collect()
    } else {
        Vec::new()
    };
    // Eight-bit RGB and CMYK with every sample there: each byte comes from
    // a table of what the loop below computes for it. A CMYK byte depends
    // on its ink and the black, so its table is worth making only for an
    // image with many pixels.
    let complete = bits == 8 && !values.is_empty() && data.len() >= width * height * count;
    let rgb_table = complete && matches!(space, Space::Rgb);
    let cmyk_table = complete && matches!(space, Space::Cmyk) && width * height >= 1 << 16;
    if rgb_table {
        let tables: Vec<[u8; 256]> = (0..3)
            .map(|component| {
                std::array::from_fn(|raw| to_byte(values[component][raw].clamp(0.0, 1.0) * 255.0))
            })
            .collect();
        for (pixel, sample) in rgba
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(data.as_chunks::<3>().0)
        {
            for (component, table) in tables.iter().enumerate() {
                pixel[component] = table[usize::from(sample[component])];
            }
        }
    } else if cmyk_table {
        let black = &values[3];
        let tables: Vec<Vec<u8>> = (0..3)
            .map(|component| {
                let inks = &values[component];
                (0..1_usize << 16)
                    .map(|index| {
                        let (ink, black) = (inks[index >> 8], black[index & 255]);
                        to_byte(((1.0 - ink) * (1.0 - black)).clamp(0.0, 1.0) * 255.0)
                    })
                    .collect()
            })
            .collect();
        for (pixel, sample) in rgba
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(data.as_chunks::<4>().0)
        {
            let black = usize::from(sample[3]);
            for (component, table) in tables.iter().enumerate() {
                pixel[component] = table[usize::from(sample[component]) << 8 | black];
            }
        }
    }
    let mut components = vec![0.0_f32; count];
    for (y, row) in rgba.chunks_mut(width * 4).enumerate() {
        if rgb_table || cmyk_table {
            break;
        }
        for (x, pixel) in row.chunks_mut(4).enumerate() {
            let color = if !pixels.is_empty() {
                pixels[samples.raw(x, y, 0) as usize]
            } else if let Some(color) = indexed_color(samples.raw(x, y, 0)) {
                to_pixel(color)
            } else {
                for (component, value) in components.iter_mut().enumerate() {
                    let raw = samples.raw(x, y, component);
                    *value = match values.get(component) {
                        Some(values) => values[raw as usize],
                        None => value_of(component, raw),
                    };
                }
                to_pixel(space.color(&components))
            };
            pixel[..3].copy_from_slice(&color);
        }
    }

    // A soft mask gives each pixel its opacity; a stencil mask hides the
    // pixels where it has a one.
    let mask_alpha = |reader: &mut Reader, key: &str, stencil: bool| {
        let Some(PdfObject::Reference(mask)) = entry(image, key) else {
            return Ok::<_, DocumentError>(None);
        };
        let Some(mask) = load(reader, mask.number, resources, [0.0; 3], depth + 1, None)? else {
            return Ok(None);
        };
        // A gray soft mask holds opacity in its color; a stencil's alpha
        // says where it would paint, which is where the image shows.
        let alpha: Vec<u8> = mask
            .rgba
            .chunks(4)
            .map(|pixel| if stencil { pixel[3] } else { pixel[0] })
            .collect();
        Ok(Some((mask.width, mask.height, alpha)))
    };
    let soft = mask_alpha(reader, "SMask", false)?;
    let mask = match soft {
        Some(_) => soft,
        None => mask_alpha(reader, "Mask", true)?,
    };
    if let Some((mask_width, mask_height, alpha)) = mask {
        let columns: Vec<usize> = (0..width).map(|x| x * mask_width / width).collect();
        for (y, row) in rgba.chunks_mut(width * 4).enumerate() {
            let start = y * mask_height / height * mask_width;
            for (pixel, &x) in row.chunks_mut(4).zip(&columns) {
                pixel[3] = alpha.get(start + x).copied().unwrap_or(255);
            }
        }
    }
    Ok(Some(Image {
        width,
        height,
        rgba,
    }))
}

/// Shrink an image by a whole factor across and another down, averaging
/// the pixels that merge.
pub(super) fn shrink(image: &Image, across: usize, down: usize) -> Image {
    let width = image.width.div_ceil(across);
    let height = image.height.div_ceil(down);
    let mut rgba = vec![0; width * height * 4];
    for (index, pixel) in rgba.chunks_mut(4).enumerate() {
        let (left, top) = (index % width * across, index / width * down);
        // Colors are weighted by their alpha so that hidden pixels do not
        // tint the result.
        let mut sums = [0_u64; 4];
        let mut pixels = 0;
        for y in top..(top + down).min(image.height) {
            for x in left..(left + across).min(image.width) {
                let source = &image.rgba[(y * image.width + x) * 4..][..4];
                let alpha = u64::from(source[3]);
                for (sum, &value) in sums.iter_mut().zip(&source[..3]) {
                    *sum += u64::from(value) * alpha;
                }
                sums[3] += alpha;
                pixels += 1;
            }
        }
        let alpha = sums[3];
        for (value, sum) in pixel.iter_mut().zip(sums) {
            *value = sum.checked_div(alpha).unwrap_or(0) as u8;
        }
        pixel[3] = (alpha / pixels.max(1)) as u8;
    }
    Image {
        width,
        height,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Document;

    /// A file of the given objects, numbered from 1, with 1 the catalog;
    /// an object with data is a stream.
    fn file(objects: &[(&str, &[u8])]) -> Vec<u8> {
        let mut file = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (index, (entries, data)) in objects.iter().enumerate() {
            offsets.push(file.len());
            file.extend(format!("{} 0 obj\n", index + 1).as_bytes());
            if data.is_empty() {
                file.extend(entries.as_bytes());
            } else {
                file.extend(format!("<< {entries} /Length {} >>\nstream\n", data.len()).as_bytes());
                file.extend(*data);
                file.extend(b"\nendstream");
            }
            file.extend(b"\nendobj\n");
        }
        let table = file.len();
        file.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for offset in offsets {
            file.extend(format!("{offset:010} 00000 n \n").as_bytes());
        }
        let size = objects.len() + 1;
        file.extend(
            format!("trailer\n<< /Root 1 0 R /Size {size} >>\nstartxref\n{table}\n%%EOF\n")
                .as_bytes(),
        );
        file
    }

    fn kept_for(file: usize) -> usize {
        let cache = IMAGE_CACHE.lock().unwrap_or_else(PoisonError::into_inner);
        cache
            .entries
            .iter()
            .filter(|cached| cached.key.file == file)
            .count()
    }

    #[test]
    fn a_closed_document_frees_its_images() {
        let bytes = file(&[
            ("<< /Type /Catalog /Pages 2 0 R >>", b""),
            ("<< /Type /Pages /Count 1 /Kids [3 0 R] >>", b""),
            (
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 2 1] /Contents 4 0 R \
                 /Resources << /XObject << /X 5 0 R >> >> >>",
                b"",
            ),
            ("", b"q 2 0 0 1 0 0 cm /X Do Q"),
            (
                "/Subtype /Image /Width 2 /Height 1 /ColorSpace /DeviceGray \
                 /BitsPerComponent 8",
                &[10, 20],
            ),
        ]);
        let mut document = Document::open(bytes).unwrap();
        let file = Arc::as_ptr(document.reader.file()) as usize;
        document.render_page(0, 1.0).unwrap();
        assert_eq!(kept_for(file), 1);
        drop(document);
        assert_eq!(kept_for(file), 0);
    }

    #[test]
    fn stencils_in_many_colors_stay_within_the_entry_cap() {
        let mut content = String::new();
        for step in 0..1500 {
            let (red, green) = (step % 50, step / 50);
            content.push_str(&format!(
                "{} {} 0 rg q 1 0 0 1 0 0 cm /X Do Q ",
                f64::from(red) / 49.0,
                f64::from(green) / 29.0
            ));
        }
        let bytes = file(&[
            ("<< /Type /Catalog /Pages 2 0 R >>", b""),
            ("<< /Type /Pages /Count 1 /Kids [3 0 R] >>", b""),
            (
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Contents 4 0 R \
                 /Resources << /XObject << /X 5 0 R >> >> >>",
                b"",
            ),
            ("", content.as_bytes()),
            ("/Subtype /Image /Width 1 /Height 1 /ImageMask true", &[0]),
        ]);
        let mut document = Document::open(bytes).unwrap();
        document.render_page(0, 1.0).unwrap();
        let file = Arc::as_ptr(document.reader.file()) as usize;
        assert!(kept_for(file) <= CACHE_ENTRIES);
        let total = IMAGE_CACHE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entries
            .len();
        assert!(total <= CACHE_ENTRIES);
    }

    #[test]
    fn an_image_masked_by_an_image_draws_as_its_page_says() {
        // The mask is an image, not a stencil, whose color space only the
        // first page names; each page must draw as it would alone.
        let bytes = file(&[
            ("<< /Type /Catalog /Pages 2 0 R >>", b""),
            ("<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>", b""),
            (
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 2 1] /Contents 5 0 R \
                 /Resources << /XObject << /X 6 0 R >> /ColorSpace << /CS0 /DeviceGray >> >> >>",
                b"",
            ),
            (
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 2 1] /Contents 5 0 R \
                 /Resources << /XObject << /X 6 0 R >> >> >>",
                b"",
            ),
            ("", b"q 2 0 0 1 0 0 cm /X Do Q"),
            (
                "/Subtype /Image /Width 2 /Height 1 /ColorSpace /DeviceGray \
                 /BitsPerComponent 8 /Mask 7 0 R",
                &[0, 0],
            ),
            (
                "/Subtype /Image /Width 2 /Height 1 /ColorSpace /CS0 /BitsPerComponent 8 \
                 /SMask 8 0 R",
                &[0, 0],
            ),
            (
                "/Subtype /Image /Width 2 /Height 1 /ColorSpace /DeviceGray \
                 /BitsPerComponent 8",
                &[0, 255],
            ),
        ]);
        let alone = |page: usize| {
            let mut document = Document::open(bytes.clone()).unwrap();
            document.render_page(page, 1.0).unwrap()
        };
        let (first, second) = (alone(0), alone(1));
        let mut document = Document::open(bytes.clone()).unwrap();
        assert_eq!(document.render_page(1, 1.0).unwrap(), second);
        assert_eq!(document.render_page(0, 1.0).unwrap(), first);
        let mut document = Document::open(bytes).unwrap();
        assert_eq!(document.render_page(0, 1.0).unwrap(), first);
        assert_eq!(document.render_page(1, 1.0).unwrap(), second);
    }
}
