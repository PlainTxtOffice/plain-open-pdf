//! `DCTDecode`: baseline, extended sequential, and progressive JPEG images
//! with Huffman coding (ITU-T T.81), decoded to eight-bit samples.
//!
//! Lossless and arithmetic-coded images report `Unsupported`.

use super::FilterError;
use crate::raster::to_byte;

/// The order coefficients are stored in, from lowest frequency up.
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// A Huffman table: for each code length, the first code, the last code,
/// and where its symbols start.
#[derive(Clone)]
struct Huffman {
    first: [i32; 17],
    last: [i32; 17],
    start: [usize; 17],
    symbols: Vec<u8>,
    /// For each value of the next `LOOKUP_BITS` bits, the symbol and code
    /// length that reading bit by bit finds, or a length of 0 when the
    /// code is longer.
    lookup: Vec<(u8, u8)>,
}

/// How many bits the Huffman lookup table decodes at once.
const LOOKUP_BITS: u32 = 9;

struct Component {
    identifier: u8,
    /// How many blocks the component has across and down one unit.
    across: usize,
    down: usize,
    quantization: usize,
    dc_table: usize,
    ac_table: usize,
    /// The DC value of the block before, which the next one adds to.
    predictor: i32,
    /// The decoded samples, in rows of `stride`.
    plane: Vec<u8>,
    stride: usize,
    /// Blocks in each row of the component.
    blocks_across: usize,
    /// In a progressive image, each block's 64 coefficients in zigzag
    /// order, gathered over the scans, before quantization is undone.
    coefficients: Vec<i32>,
}

/// What one scan of a progressive image holds (ITU-T T.81 annex G).
#[derive(Clone, Copy)]
struct Pass {
    /// The first and last coefficient, in zigzag order.
    start: usize,
    end: usize,
    /// The bit position this scan refines from, or 0 for a first scan.
    high: u32,
    /// The bit position of the scan's values.
    low: u32,
}

impl Pass {
    /// Decode one block's part of a progressive scan into its
    /// coefficients. `bands` counts the blocks still to be skipped by an
    /// end-of-band run.
    fn block(
        self,
        bits: &mut Bits<'_>,
        coefficients: &mut [i32],
        predictor: &mut i32,
        dc: Option<&Huffman>,
        ac: Option<&Huffman>,
        bands: &mut u32,
    ) -> Result<(), FilterError> {
        const CORRUPT: FilterError = FilterError::Corrupt;
        let (positive, negative) = (1_i32 << self.low, -1_i32 << self.low);
        if self.start == 0 {
            // DC: the difference from the block before, or one more bit.
            if self.high == 0 {
                let size = u32::from(bits.symbol(dc.ok_or(CORRUPT)?)).min(16);
                *predictor += bits.signed(size);
                coefficients[0] = *predictor << self.low;
            } else if bits.bits(1) == 1 {
                coefficients[0] |= positive;
            }
            return Ok(());
        }
        let ac = ac.ok_or(CORRUPT)?;
        let end = self.end.min(63);
        if self.high == 0 {
            // A first AC scan: runs of zeros, values, and end-of-band runs
            // that cover whole blocks.
            if *bands > 0 {
                *bands -= 1;
                return Ok(());
            }
            let mut index = self.start;
            while index <= end {
                let symbol = bits.symbol(ac);
                let (run, size) = (u32::from(symbol >> 4), u32::from(symbol & 15));
                if size == 0 {
                    if run < 15 {
                        *bands = (1 << run) - 1 + bits.bits(run);
                        break;
                    }
                    index += 16;
                    continue;
                }
                index += run as usize;
                if index > end {
                    break;
                }
                coefficients[index] = bits.signed(size) * positive;
                index += 1;
            }
            return Ok(());
        }
        // A refining AC scan: each coefficient that is already nonzero
        // gets a correction bit, and new ones, all of one bit, are placed
        // after runs of coefficients that are still zero.
        let refine = |bits: &mut Bits<'_>, value: &mut i32| {
            if bits.bits(1) == 1 && *value & positive == 0 {
                *value += if *value >= 0 { positive } else { negative };
            }
        };
        let mut index = self.start;
        if *bands == 0 {
            while index <= end {
                let symbol = bits.symbol(ac);
                let (mut run, size) = (i32::from(symbol >> 4), symbol & 15);
                let mut placed = 0;
                if size != 0 {
                    placed = if bits.bits(1) == 1 {
                        positive
                    } else {
                        negative
                    };
                } else if run != 15 {
                    *bands = (1 << run) + bits.bits(run as u32);
                    break;
                }
                while index <= end {
                    if coefficients[index] != 0 {
                        refine(bits, &mut coefficients[index]);
                    } else {
                        if run == 0 {
                            break;
                        }
                        run -= 1;
                    }
                    index += 1;
                }
                if placed != 0 && index <= end {
                    coefficients[index] = placed;
                }
                index += 1;
            }
        }
        if *bands > 0 {
            while index <= end {
                if coefficients[index] != 0 {
                    refine(bits, &mut coefficients[index]);
                }
                index += 1;
            }
            *bands -= 1;
        }
        Ok(())
    }
}

/// Reads bits from entropy-coded data, in which a 0xFF byte is followed by
/// a zero byte and anything else after 0xFF is a marker.
struct Bits<'a> {
    data: &'a [u8],
    position: usize,
    pending: u32,
    count: u32,
    /// Whether a marker or the end of the data was reached; zero bits
    /// follow.
    ended: bool,
}

impl Bits<'_> {
    fn fill(&mut self) {
        while self.count <= 24 {
            let mut byte = 0;
            if !self.ended {
                match (
                    self.data.get(self.position),
                    self.data.get(self.position + 1),
                ) {
                    (Some(0xFF), Some(0)) => {
                        byte = 0xFF;
                        self.position += 2;
                    }
                    (Some(0xFF), _) | (None, _) => self.ended = true,
                    (Some(&value), _) => {
                        byte = value;
                        self.position += 1;
                    }
                }
            }
            self.pending |= u32::from(byte) << (24 - self.count);
            self.count += 8;
        }
    }

    fn bits(&mut self, count: u32) -> u32 {
        if count == 0 {
            return 0;
        }
        self.fill();
        let value = self.pending >> (32 - count);
        self.pending <<= count;
        self.count -= count;
        value
    }

    /// A value of `size` bits, where a leading zero bit means negative.
    fn signed(&mut self, size: u32) -> i32 {
        if size == 0 {
            return 0;
        }
        let value = self.bits(size) as i32;
        if value < 1 << (size - 1) {
            value - (1 << size) + 1
        } else {
            value
        }
    }

    fn symbol(&mut self, table: &Huffman) -> u8 {
        self.fill();
        let (symbol, length) = table.lookup[(self.pending >> (32 - LOOKUP_BITS)) as usize];
        if length > 0 {
            self.pending <<= length;
            self.count -= u32::from(length);
            return symbol;
        }
        let mut code = 0;
        for length in 1..=16 {
            code = code << 1 | self.bits(1) as i32;
            if code <= table.last[length] {
                let index = table.start[length] + (code - table.first[length]) as usize;
                return table.symbols.get(index).copied().unwrap_or(0);
            }
        }
        0
    }

    /// Move past a restart marker and start reading afresh.
    fn restart(&mut self) {
        self.pending = 0;
        self.count = 0;
        self.ended = false;
        while let Some(&byte) = self.data.get(self.position) {
            let marker = self.data.get(self.position + 1).copied().unwrap_or(0);
            if byte == 0xFF && (0xD0..=0xD7).contains(&marker) {
                self.position += 2;
                return;
            }
            if byte == 0xFF && marker != 0 && marker != 0xFF {
                return;
            }
            self.position += 1;
        }
    }
}

fn u16_at(data: &[u8], offset: usize) -> Option<usize> {
    Some(usize::from(*data.get(offset)?) << 8 | usize::from(*data.get(offset + 1)?))
}

fn huffman(counts: &[u8], symbols: &[u8]) -> Huffman {
    let mut table = Huffman {
        first: [0; 17],
        last: [-1; 17],
        start: [0; 17],
        symbols: symbols.to_vec(),
        lookup: Vec::new(),
    };
    let (mut code, mut index) = (0_i32, 0);
    for length in 1..=16 {
        let count = usize::from(counts[length - 1]);
        table.start[length] = index;
        table.first[length] = code;
        if count > 0 {
            table.last[length] = code + count as i32 - 1;
        }
        index += count;
        code = (code + count as i32) << 1;
    }
    // The lookup table finds what reading bit by bit would, for every
    // value the next bits can have.
    table.lookup = (0..1_i32 << LOOKUP_BITS)
        .map(|bits| {
            for length in 1..=LOOKUP_BITS as usize {
                let code = bits >> (LOOKUP_BITS as usize - length);
                if code <= table.last[length] {
                    let index = table.start[length] + (code - table.first[length]) as usize;
                    let symbol = table.symbols.get(index).copied().unwrap_or(0);
                    return (symbol, length as u8);
                }
            }
            (0, 0)
        })
        .collect();
    table
}

/// Decode a JPEG image to interleaved eight-bit samples: one component for
/// gray, three for RGB, or four for CMYK.
///
/// Three-component images are converted from YCbCr to RGB, and
/// four-component ones from YCCK to CMYK, unless an Adobe marker says the
/// image is stored without that transform.
pub(crate) fn dct_decode(data: &[u8], max_output: usize) -> Result<Vec<u8>, FilterError> {
    dct_decode_reduced(data, max_output, 1)
}

/// Decode a JPEG image as `dct_decode` does, at its size divided by
/// `reduce`, which is 1, 2, 4, or 8, rounding up; another value counts as 1.
///
/// Each block gives 8 / `reduce` samples across and down, from its
/// lowest frequencies, as a scaled inverse transform does; with `reduce` 1
/// the result is exactly `dct_decode`'s.
pub(crate) fn dct_decode_reduced(
    data: &[u8],
    max_output: usize,
    reduce: usize,
) -> Result<Vec<u8>, FilterError> {
    const CORRUPT: FilterError = FilterError::Corrupt;
    let reduce = if matches!(reduce, 2 | 4 | 8) {
        reduce
    } else {
        1
    };
    // Samples across and down each block gives.
    let side = 8 / reduce;
    let mut quantization = [[1_i32; 64]; 4];
    let mut dc_tables: [Option<Huffman>; 4] = [None, None, None, None];
    let mut ac_tables: [Option<Huffman>; 4] = [None, None, None, None];
    let mut components: Vec<Component> = Vec::new();
    let (mut width, mut height) = (0, 0);
    let (mut most_across, mut most_down) = (1, 1);
    let mut restart_interval = 0;
    let mut adobe_transform: Option<u8> = None;
    let mut progressive = false;
    // The basis of the inverse transform: row `u` holds frequency `u`.
    let mut basis = [[0.0_f32; 8]; 8];
    for (frequency, row) in basis.iter_mut().enumerate() {
        let scale = if frequency == 0 { 0.5_f64.sqrt() } else { 1.0 } / 2.0;
        for (sample, value) in row.iter_mut().enumerate() {
            let angle = (2 * sample + 1) as f64 * frequency as f64 * std::f64::consts::PI / 16.0;
            *value = (scale * angle.cos()) as f32;
        }
    }
    // The basis of a smaller transform, which takes each block's lowest
    // frequencies to `size` samples at the centers of the pixels they
    // stand for.
    let mut reduced_basis = [[0.0_f32; 8]; 8];
    for (frequency, row) in reduced_basis.iter_mut().enumerate().take(side) {
        let scale = if frequency == 0 { 0.5_f64.sqrt() } else { 1.0 } / 2.0;
        for (sample, value) in row.iter_mut().enumerate().take(side) {
            let angle = (2 * sample + 1) as f64 * frequency as f64 * std::f64::consts::PI
                / (2 * side) as f64;
            *value = (scale * angle.cos()) as f32;
        }
    }

    let mut position = 0;
    loop {
        // Find the next marker; fill bytes of 0xFF may come before it.
        while *data.get(position).ok_or(CORRUPT)? != 0xFF {
            position += 1;
        }
        while data.get(position + 1) == Some(&0xFF) {
            position += 1;
        }
        let marker = *data.get(position + 1).ok_or(CORRUPT)?;
        position += 2;
        if marker == 0xD9 {
            break;
        }
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        let length = u16_at(data, position).ok_or(CORRUPT)?;
        let segment = data
            .get(position + 2..position + length.max(2))
            .ok_or(CORRUPT)?;
        position += length.max(2);
        match marker {
            // Quantization tables, of bytes or of words.
            0xDB => {
                let mut rest = segment;
                while let Some((&info, values)) = rest.split_first() {
                    let table = quantization
                        .get_mut(usize::from(info & 15))
                        .ok_or(CORRUPT)?;
                    let wide = info >> 4 != 0;
                    let size = if wide { 128 } else { 64 };
                    let values = values.get(..size).ok_or(CORRUPT)?;
                    for (index, entry) in table.iter_mut().enumerate() {
                        *entry = if wide {
                            i32::from(values[index * 2]) << 8 | i32::from(values[index * 2 + 1])
                        } else {
                            i32::from(values[index])
                        };
                    }
                    rest = &rest[1 + size..];
                }
            }
            // Huffman tables.
            0xC4 => {
                let mut rest = segment;
                while let Some((&info, values)) = rest.split_first() {
                    let counts = values.get(..16).ok_or(CORRUPT)?;
                    let total: usize = counts.iter().map(|&count| usize::from(count)).sum();
                    let symbols = values.get(16..16 + total).ok_or(CORRUPT)?;
                    let tables = if info >> 4 == 0 {
                        &mut dc_tables
                    } else {
                        &mut ac_tables
                    };
                    *tables.get_mut(usize::from(info & 15)).ok_or(CORRUPT)? =
                        Some(huffman(counts, symbols));
                    rest = &rest[17 + total..];
                }
            }
            // The frame: baseline, extended sequential, or progressive.
            0xC0..=0xC2 => {
                progressive = marker == 0xC2;
                if *segment.first().ok_or(CORRUPT)? != 8 {
                    return Err(FilterError::Unsupported);
                }
                height = u16_at(segment, 1).ok_or(CORRUPT)?;
                width = u16_at(segment, 3).ok_or(CORRUPT)?;
                let count = usize::from(*segment.get(5).ok_or(CORRUPT)?);
                if width == 0
                    || height == 0
                    || !matches!(count, 1 | 3 | 4)
                    || !components.is_empty()
                {
                    return Err(CORRUPT);
                }
                if width.saturating_mul(height).saturating_mul(count) > max_output {
                    return Err(FilterError::LimitExceeded);
                }
                for index in 0..count {
                    let entry = segment.get(6 + index * 3..9 + index * 3).ok_or(CORRUPT)?;
                    let (across, down) = (usize::from(entry[1] >> 4), usize::from(entry[1] & 15));
                    if !(1..=4).contains(&across) || !(1..=4).contains(&down) || entry[2] > 3 {
                        return Err(CORRUPT);
                    }
                    most_across = most_across.max(across);
                    most_down = most_down.max(down);
                    components.push(Component {
                        identifier: entry[0],
                        across,
                        down,
                        quantization: usize::from(entry[2]),
                        dc_table: 0,
                        ac_table: 0,
                        predictor: 0,
                        plane: Vec::new(),
                        stride: 0,
                        blocks_across: 0,
                        coefficients: Vec::new(),
                    });
                }
                let units_across = width.div_ceil(8 * most_across);
                let units_down = height.div_ceil(8 * most_down);
                for component in &mut components {
                    component.blocks_across = units_across * component.across;
                    let blocks_down = units_down * component.down;
                    component.stride = component.blocks_across * side;
                    component.plane = vec![0; component.stride * blocks_down * side];
                    if progressive {
                        component.coefficients =
                            vec![0; component.blocks_across * blocks_down * 64];
                    }
                }
            }
            // Lossless and arithmetic-coded frames.
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCF => return Err(FilterError::Unsupported),
            0xDD => restart_interval = u16_at(segment, 0).ok_or(CORRUPT)?,
            // An Adobe marker tells whether the colors were transformed.
            0xEE if segment.starts_with(b"Adobe") => adobe_transform = segment.get(11).copied(),
            // A scan: entropy-coded data for some of the components.
            0xDA => {
                let count = usize::from(*segment.first().ok_or(CORRUPT)?);
                let mut scanned = Vec::new();
                for index in 0..count {
                    let entry = segment.get(1 + index * 2..3 + index * 2).ok_or(CORRUPT)?;
                    let which = components
                        .iter()
                        .position(|component| component.identifier == entry[0])
                        .ok_or(CORRUPT)?;
                    components[which].dc_table = usize::from(entry[1] >> 4) & 3;
                    components[which].ac_table = usize::from(entry[1] & 15) & 3;
                    scanned.push(which);
                }
                if scanned.is_empty() {
                    return Err(CORRUPT);
                }
                let tail = segment.get(1 + count * 2..4 + count * 2).ok_or(CORRUPT)?;
                let pass = Pass {
                    start: usize::from(tail[0]),
                    end: usize::from(tail[1]),
                    high: u32::from(tail[2] >> 4),
                    low: u32::from(tail[2] & 15).min(13),
                };
                let mut bits = Bits {
                    data,
                    position,
                    pending: 0,
                    count: 0,
                    ended: false,
                };
                // One component alone is coded block by block; several
                // together are coded in units that hold blocks of each.
                let alone = scanned.len() == 1;
                let (units_across, units_down) = if alone {
                    let component = &components[scanned[0]];
                    let size = |length: usize, factor: usize, most: usize| {
                        (length * factor).div_ceil(most).div_ceil(8)
                    };
                    (
                        size(width, component.across, most_across),
                        size(height, component.down, most_down),
                    )
                } else {
                    (
                        width.div_ceil(8 * most_across),
                        height.div_ceil(8 * most_down),
                    )
                };
                let mut until_restart = restart_interval;
                let mut bands = 0;
                for unit in 0..units_across * units_down {
                    if restart_interval > 0 && until_restart == 0 {
                        bits.restart();
                        until_restart = restart_interval;
                        bands = 0;
                        for &which in &scanned {
                            components[which].predictor = 0;
                        }
                    }
                    until_restart = until_restart.saturating_sub(1);
                    let (unit_x, unit_y) = (unit % units_across, unit / units_across);
                    if progressive {
                        for &which in &scanned {
                            let component = &mut components[which];
                            let (across, down) = if alone {
                                (1, 1)
                            } else {
                                (component.across, component.down)
                            };
                            let per_row = component.blocks_across;
                            for block in 0..across * down {
                                let block_x = unit_x * across + block % across;
                                let block_y = unit_y * down + block / across;
                                let start = (block_y * per_row + block_x) * 64;
                                let Some(coefficients) =
                                    component.coefficients.get_mut(start..start + 64)
                                else {
                                    continue;
                                };
                                let dc = dc_tables[component.dc_table].as_ref();
                                let ac = ac_tables[component.ac_table].as_ref();
                                pass.block(
                                    &mut bits,
                                    coefficients,
                                    &mut component.predictor,
                                    dc,
                                    ac,
                                    &mut bands,
                                )?;
                            }
                        }
                        continue;
                    }
                    for &which in &scanned {
                        let component = &mut components[which];
                        let (across, down) = if alone {
                            (1, 1)
                        } else {
                            (component.across, component.down)
                        };
                        let dc = dc_tables[component.dc_table].as_ref().ok_or(CORRUPT)?;
                        let ac = ac_tables[component.ac_table].as_ref().ok_or(CORRUPT)?;
                        let quantization = &quantization[component.quantization];
                        for block in 0..across * down {
                            let block_x = unit_x * across + block % across;
                            let block_y = unit_y * down + block / across;
                            let mut coefficients = [0.0_f32; 64];
                            let size = u32::from(bits.symbol(dc)).min(16);
                            component.predictor += bits.signed(size);
                            coefficients[0] = (component.predictor * quantization[0]) as f32;
                            let mut index = 1;
                            while index < 64 {
                                let symbol = bits.symbol(ac);
                                let (run, size) =
                                    (usize::from(symbol >> 4), u32::from(symbol & 15));
                                if size == 0 {
                                    if run != 15 {
                                        break;
                                    }
                                    index += 16;
                                    continue;
                                }
                                index += run;
                                if index > 63 {
                                    break;
                                }
                                let value = bits.signed(size) * quantization[index];
                                coefficients[ZIGZAG[index]] = value as f32;
                                index += 1;
                            }
                            let samples = if side == 8 {
                                inverse_transform(&coefficients, &basis)
                            } else {
                                reduced_transform(&coefficients, &reduced_basis, side)
                            };
                            for (row, values) in samples[..side * side].chunks(side).enumerate() {
                                let start =
                                    (block_y * side + row) * component.stride + block_x * side;
                                if let Some(target) = component.plane.get_mut(start..start + side) {
                                    target.copy_from_slice(values);
                                }
                            }
                        }
                    }
                }
                position = bits.position;
            }
            _ => {}
        }
    }
    if components.is_empty() {
        return Err(CORRUPT);
    }
    // A progressive image's coefficients are complete once every scan is
    // read; then quantization is undone and the blocks transformed.
    if progressive {
        for component in &mut components {
            let quantization = &quantization[component.quantization];
            let per_row = component.blocks_across;
            for (block, values) in component
                .coefficients
                .as_chunks::<64>()
                .0
                .iter()
                .enumerate()
            {
                let mut coefficients = [0.0_f32; 64];
                for (index, &value) in values.iter().enumerate() {
                    coefficients[ZIGZAG[index]] = (value * quantization[index]) as f32;
                }
                let samples = if side == 8 {
                    inverse_transform(&coefficients, &basis)
                } else {
                    reduced_transform(&coefficients, &reduced_basis, side)
                };
                let (block_x, block_y) = (block % per_row, block / per_row);
                for (row, values) in samples[..side * side].chunks(side).enumerate() {
                    let start = (block_y * side + row) * component.stride + block_x * side;
                    if let Some(target) = component.plane.get_mut(start..start + side) {
                        target.copy_from_slice(values);
                    }
                }
            }
        }
    }

    // Bring every component to full size and interleave them.
    let (width, height) = (width.div_ceil(reduce), height.div_ceil(reduce));
    let count = components.len();
    let mut output = vec![0; width * height * count];
    for (index, component) in components.iter().enumerate() {
        if component.across == most_across && component.down == most_down {
            for y in 0..height {
                let row = &component.plane[y * component.stride..][..width];
                for (x, &sample) in row.iter().enumerate() {
                    output[(y * width + x) * count + index] = sample;
                }
            }
            continue;
        }
        // A component with fewer samples is stretched by blending its
        // nearest samples, with each sample at the center of the pixels it
        // covers.
        let columns = (width * component.across).div_ceil(most_across).max(1);
        let rows = (height * component.down).div_ceil(most_down).max(1);
        let place = |position: usize, factor: usize, most: usize, last: usize| {
            let source = (position as f32 + 0.5) * factor as f32 / most as f32 - 0.5;
            let source = source.clamp(0.0, last as f32);
            let low = source.floor() as usize;
            (low, (low + 1).min(last), source - low as f32)
        };
        // Every row takes its samples from the same columns.
        let places: Vec<(usize, usize, f32)> = (0..width)
            .map(|x| place(x, component.across, most_across, columns - 1))
            .collect();
        for y in 0..height {
            let (top, bottom, down) = place(y, component.down, most_down, rows - 1);
            let (top, bottom) = (top * component.stride, bottom * component.stride);
            for (x, &(left, right, across)) in places.iter().enumerate() {
                let sample = |offset: usize| f32::from(component.plane[offset]);
                let upper = sample(top + left) * (1.0 - across) + sample(top + right) * across;
                let lower =
                    sample(bottom + left) * (1.0 - across) + sample(bottom + right) * across;
                let value = upper * (1.0 - down) + lower * down;
                output[(y * width + x) * count + index] = to_byte(value);
            }
        }
    }
    // Three components are YCbCr and four are YCCK unless the Adobe marker
    // says they are not.
    let transformed = match (count, adobe_transform) {
        (3, transform) => transform != Some(0),
        (4, transform) => transform == Some(2),
        _ => false,
    };
    if transformed {
        // Each product of a chroma sample, worked out once: the sums below
        // take the same values in the same order.
        let products = |factor: f32| -> [f32; 256] {
            std::array::from_fn(|sample| factor * (sample as f32 - 128.0))
        };
        let (red_from_cr, blue_from_cb) = (products(1.402), products(1.772));
        let (green_from_cb, green_from_cr) = (products(0.344_136), products(0.714_136));
        for pixel in output.chunks_exact_mut(count) {
            let luma = f32::from(pixel[0]);
            let (blue, red) = (usize::from(pixel[1]), usize::from(pixel[2]));
            let colors = [
                luma + red_from_cr[red],
                luma - green_from_cb[blue] - green_from_cr[red],
                luma + blue_from_cb[blue],
            ];
            // YCCK holds the reverse of the cyan, magenta, and yellow inks
            // as if they were red, green, and blue; black is as it is.
            for (value, color) in pixel.iter_mut().zip(colors) {
                *value = if count == 4 {
                    255 - to_byte(color)
                } else {
                    to_byte(color)
                };
            }
        }
    }
    Ok(output)
}

/// Turn a block's lowest `size` frequencies across and down into `size`
/// samples across and down, at the start of the result.
fn reduced_transform(coefficients: &[f32; 64], basis: &[[f32; 8]; 8], size: usize) -> [u8; 64] {
    // A block with only its first coefficient is flat, as at full size.
    if coefficients[1..].iter().all(|&value| value == 0.0) {
        return [to_byte(coefficients[0] / 8.0 + 128.0); 64];
    }
    let mut rows = [0.0_f32; 64];
    for row in 0..size {
        for sample in 0..size {
            rows[row * 8 + sample] = (0..size)
                .map(|frequency| coefficients[row * 8 + frequency] * basis[frequency][sample])
                .sum();
        }
    }
    let mut samples = [0; 64];
    for column in 0..size {
        for sample in 0..size {
            let sum: f32 = (0..size)
                .map(|frequency| rows[frequency * 8 + column] * basis[frequency][sample])
                .sum();
            samples[sample * size + column] = to_byte(sum + 128.0);
        }
    }
    samples
}

/// Turn a block of 64 frequency coefficients into 64 samples.
fn inverse_transform(coefficients: &[f32; 64], basis: &[[f32; 8]; 8]) -> [u8; 64] {
    // A block with only its first coefficient is flat.
    if coefficients[1..].iter().all(|&value| value == 0.0) {
        let flat = to_byte(coefficients[0] / 8.0 + 128.0);
        return [flat; 64];
    }
    // Along each row of coefficients, then down each column. Terms whose
    // coefficient is zero add nothing, so they are skipped; the sums of the
    // rest are taken in the same order, so the result is the same.
    let mut rows = [0.0_f32; 64];
    let mut used_rows = [false; 8];
    for (row, used) in used_rows.iter_mut().enumerate() {
        let values = &coefficients[row * 8..row * 8 + 8];
        let Some(last) = values.iter().rposition(|&value| value != 0.0) else {
            continue;
        };
        *used = true;
        for sample in 0..8 {
            let mut sum = 0.0_f32;
            for (frequency, &value) in values[..=last].iter().enumerate() {
                if value != 0.0 {
                    sum += value * basis[frequency][sample];
                }
            }
            rows[row * 8 + sample] = sum;
        }
    }
    let mut samples = [0; 64];
    for column in 0..8 {
        for sample in 0..8 {
            let mut sum = 0.0_f32;
            for (frequency, &used) in used_rows.iter().enumerate() {
                let value = rows[frequency * 8 + column];
                if used && value != 0.0 {
                    sum += value * basis[frequency][sample];
                }
            }
            samples[sample * 8 + column] = to_byte(sum + 128.0);
        }
    }
    samples
}
