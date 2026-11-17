//! Color spaces (ISO 32000-2 section 8.6): how color components become the
//! red, green, and blue the renderer paints.

use super::DocumentError;
use super::function::{self, Function};
use super::reader::{Reader, entry};
use crate::{Dictionary, PdfObject};

/// How deeply color spaces may refer to one another.
const MAX_DEPTH: usize = 8;

/// A color space.
#[derive(Debug, Clone)]
pub(super) enum Space {
    /// Gray, and calibrated gray, whose calibration is not applied.
    Gray,
    /// RGB, and calibrated RGB.
    Rgb,
    Cmyk,
    /// CIE L*a*b*, with its white point and the range of a* and b*.
    Lab {
        white: [f64; 3],
        range: [f64; 4],
    },
    /// Indexes into a table of colors of a base space.
    Indexed {
        base: Box<Space>,
        table: Vec<u8>,
    },
    /// Separation and DeviceN: amounts of ink, which a tint transform turns
    /// into colors of an alternate space. Without a usable transform, more
    /// ink is darker gray.
    Tinted {
        count: usize,
        alternate: Box<Space>,
        transform: Option<Function>,
    },
    /// Patterns, perhaps with the space an uncolored pattern's color is in.
    Pattern(Option<Box<Space>>),
}

impl Space {
    /// How many components a color in the space has.
    pub(super) fn components(&self) -> usize {
        match self {
            Self::Gray | Self::Indexed { .. } => 1,
            Self::Rgb | Self::Lab { .. } => 3,
            Self::Cmyk => 4,
            Self::Tinted { count, .. } => *count,
            Self::Pattern(base) => base.as_ref().map_or(0, |base| base.components()),
        }
    }

    /// The color a space starts with: black, or all the ink.
    pub(super) fn initial(&self) -> Vec<f64> {
        match self {
            Self::Cmyk => vec![0.0, 0.0, 0.0, 1.0],
            Self::Lab { .. } => vec![0.0; 3],
            Self::Tinted { count, .. } => vec![1.0; *count],
            other => vec![0.0; other.components()],
        }
    }

    /// The red, green, and blue, from 0 to 1, of a color in the space.
    pub(super) fn rgb(&self, components: &[f64]) -> [f64; 3] {
        let at = |index: usize| components.get(index).copied().unwrap_or(0.0);
        let unit = |value: f64| value.clamp(0.0, 1.0);
        match self {
            Self::Gray => [unit(at(0)); 3],
            Self::Rgb => [unit(at(0)), unit(at(1)), unit(at(2))],
            Self::Cmyk => {
                let black = unit(at(3));
                [at(0), at(1), at(2)].map(|ink| (1.0 - unit(ink)) * (1.0 - black))
            }
            Self::Lab { white, range } => {
                let lightness = at(0).clamp(0.0, 100.0);
                let a = at(1).clamp(range[0], range[1]);
                let b = at(2).clamp(range[2], range[3]);
                lab_to_rgb(lightness, a, b, white)
            }
            Self::Indexed { base, table } => {
                let size = base.components();
                let index = at(0).max(0.0).round() as usize;
                let entry: Vec<f64> = (0..size)
                    .map(|offset| {
                        let byte = table.get(index * size + offset).copied().unwrap_or(0);
                        f64::from(byte) / 255.0
                    })
                    .collect();
                // A Lab base spreads the table's bytes over its ranges.
                match base.as_ref() {
                    Self::Lab { range, .. } => {
                        let spread = [
                            entry[0] * 100.0,
                            range[0] + entry[1] * (range[1] - range[0]),
                            range[2] + entry[2] * (range[3] - range[2]),
                        ];
                        base.rgb(&spread)
                    }
                    _ => base.rgb(&entry),
                }
            }
            Self::Tinted {
                alternate,
                transform,
                ..
            } => match transform {
                Some(transform) => alternate.rgb(&transform.evaluate(components)),
                None => {
                    let ink: f64 = components.iter().map(|&tint| unit(tint)).sum();
                    [1.0 - ink.min(1.0); 3]
                }
            },
            Self::Pattern(base) => base.as_ref().map_or([0.0; 3], |base| base.rgb(components)),
        }
    }
}

/// Convert L*a*b* under a white point to sRGB, from 0 to 1.
fn lab_to_rgb(lightness: f64, a: f64, b: f64, white: &[f64; 3]) -> [f64; 3] {
    let inverse = |value: f64| {
        if value > 6.0 / 29.0 {
            value.powi(3)
        } else {
            3.0 * (6.0_f64 / 29.0).powi(2) * (value - 4.0 / 29.0)
        }
    };
    let fy = (lightness + 16.0) / 116.0;
    let (fx, fz) = (fy + a / 500.0, fy - b / 200.0);
    // Relative to the white point, which is taken to be D65, as sRGB's is;
    // the white point given is not adapted to it.
    let _ = white;
    let d65 = [0.9505, 1.0, 1.089];
    let (x, y, z) = (
        inverse(fx) * d65[0],
        inverse(fy) * d65[1],
        inverse(fz) * d65[2],
    );
    let linear = [
        3.2406 * x - 1.5372 * y - 0.4986 * z,
        -0.9689 * x + 1.8758 * y + 0.0415 * z,
        0.0557 * x - 0.2040 * y + 1.0570 * z,
    ];
    linear.map(|value| {
        let value = value.clamp(0.0, 1.0);
        if value <= 0.003_130_8 {
            12.92 * value
        } else {
            1.055 * value.powf(1.0 / 2.4) - 0.055
        }
    })
}

/// Read a color space from its name or array form; a name that is not a
/// family is looked up among the resources. `None` for one that cannot be
/// read.
pub(super) fn load(
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
    let numbers = |reader: &mut Reader, object: Option<&PdfObject>, key: &str| -> Vec<f64> {
        let Some(object) = object else {
            return Vec::new();
        };
        match reader.resolve(object) {
            Ok(PdfObject::Dictionary(dictionary)) => match reader.resolve_entry(&dictionary, key) {
                Ok(PdfObject::Array(values)) => {
                    values.iter().filter_map(PdfObject::as_number).collect()
                }
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    };
    Ok(match family {
        b"DeviceGray" | b"G" | b"CalGray" => Some(Space::Gray),
        b"DeviceRGB" | b"RGB" | b"CalRGB" => Some(Space::Rgb),
        b"DeviceCMYK" | b"CMYK" => Some(Space::Cmyk),
        b"Lab" => {
            let white = numbers(reader, parts.first(), "WhitePoint");
            let range = numbers(reader, parts.first(), "Range");
            Some(Space::Lab {
                white: match white[..] {
                    [x, y, z] => [x, y, z],
                    _ => [0.9505, 1.0, 1.089],
                },
                range: match range[..] {
                    [a, b, c, d] => [a, b, c, d],
                    _ => [-100.0, 100.0, -100.0, 100.0],
                },
            })
        }
        // An ICC profile is not applied: its alternate space is used, or
        // the device space with its number of components.
        b"ICCBased" => {
            let profile = match parts.first() {
                Some(profile) => reader.resolve(profile)?,
                None => PdfObject::Null,
            };
            let profile = profile.as_dictionary();
            let alternate = match profile.and_then(|profile| entry(profile, "Alternate")) {
                Some(alternate) => load(reader, &alternate.clone(), resources, depth + 1)?,
                None => None,
            };
            let count = profile
                .and_then(|profile| entry(profile, "N"))
                .and_then(PdfObject::as_integer);
            alternate.or(match count {
                Some(1) => Some(Space::Gray),
                Some(4) => Some(Space::Cmyk),
                _ => Some(Space::Rgb),
            })
        }
        b"Indexed" | b"I" => {
            let Some(base) = parts.first() else {
                return Ok(None);
            };
            let Some(base) = load(reader, base, resources, depth + 1)? else {
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
            Some(Space::Indexed {
                base: Box::new(base),
                table,
            })
        }
        b"Separation" | b"DeviceN" => {
            let count = if family == b"Separation" {
                1
            } else {
                match parts.first() {
                    Some(names) => reader
                        .resolve(names)?
                        .as_array()
                        .map_or(1, <[PdfObject]>::len),
                    None => 1,
                }
            };
            let alternate = match parts.get(1) {
                Some(alternate) => load(reader, alternate, resources, depth + 1)?,
                None => None,
            };
            let transform = parts
                .get(2)
                .and_then(|transform| function::load(reader, transform, 0));
            Some(Space::Tinted {
                count: count.max(1),
                alternate: Box::new(alternate.unwrap_or(Space::Gray)),
                transform,
            })
        }
        b"Pattern" => {
            let base = match parts.first() {
                Some(base) => load(reader, base, resources, depth + 1)?.map(Box::new),
                None => None,
            };
            Some(Space::Pattern(base))
        }
        // Any other name is looked up among the resources.
        name if parts.is_empty() => match reader.resolve_entry(resources, "ColorSpace")? {
            PdfObject::Dictionary(spaces) => match spaces.get(name).cloned() {
                Some(space) => load(reader, &space, resources, depth + 1)?,
                None => None,
            },
            _ => None,
        },
        _ => None,
    })
}
