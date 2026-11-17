//! Shadings (ISO 32000-2 section 8.7.4.5): colors that vary smoothly over
//! an area, for the `sh` operator and shading patterns. Function-based,
//! axial, and radial shadings are drawn; mesh shadings are not.

use super::DocumentError;
use super::color::{self, Space};
use super::function::{self, Function};
use super::page_text::{IDENTITY, Matrix};
use super::reader::{Reader, entry};
use crate::{Dictionary, PdfObject};

/// How many colors along `t` are worked out ahead for axial and radial
/// shadings.
const STEPS: usize = 512;

#[derive(Debug, Clone)]
enum Kind {
    /// Type 1: a function of the point, over a domain, after a matrix.
    Function { domain: [f64; 4], inverse: Matrix },
    /// Type 2: along the line between two points.
    Axial { coords: [f64; 4] },
    /// Type 3: between two circles.
    Radial { coords: [f64; 6] },
}

/// A shading that can be drawn.
#[derive(Debug, Clone)]
pub(super) struct Shading {
    kind: Kind,
    space: Space,
    function: Option<Function>,
    /// Whether the shading goes on past its start and its end.
    extend: [bool; 2],
    /// The colors at `STEPS` points from the start of `domain` to its end.
    colors: Vec<[f64; 3]>,
    /// The area the shading is limited to, in its own space.
    pub bounds: Option<[f64; 4]>,
    /// The color outside the shading, for a shading pattern.
    pub background: Option<[f64; 3]>,
}

/// The inverse of a matrix; `None` when it has none.
pub(super) fn invert(matrix: &Matrix) -> Option<Matrix> {
    let [a, b, c, d, e, f] = *matrix;
    let determinant = a * d - b * c;
    if !determinant.is_finite() || determinant.abs() < 1e-12 {
        return None;
    }
    Some([
        d / determinant,
        -b / determinant,
        -c / determinant,
        a / determinant,
        (c * f - d * e) / determinant,
        (b * e - a * f) / determinant,
    ])
}

fn numbers(reader: &mut Reader, dictionary: &Dictionary, key: &str) -> Vec<f64> {
    match reader.resolve_entry(dictionary, key) {
        Ok(PdfObject::Array(values)) => values.iter().filter_map(PdfObject::as_number).collect(),
        _ => Vec::new(),
    }
}

impl Shading {
    /// Read a shading dictionary or stream; `None` for a kind that is not
    /// drawn or one that cannot be read.
    pub(super) fn load(
        reader: &mut Reader,
        object: &PdfObject,
        resources: &Dictionary,
    ) -> Result<Option<Self>, DocumentError> {
        let resolved = reader.resolve(object)?;
        let Some(dictionary) = resolved.as_dictionary() else {
            return Ok(None);
        };
        let Some(space) = entry(dictionary, "ColorSpace").cloned() else {
            return Ok(None);
        };
        let Some(space) = color::load(reader, &space, resources, 0)? else {
            return Ok(None);
        };
        let function = match entry(dictionary, "Function") {
            Some(function) => function::load(reader, &function.clone(), 0),
            None => None,
        };
        let coords = numbers(reader, dictionary, "Coords");
        let domain = numbers(reader, dictionary, "Domain");
        let kind = match entry(dictionary, "ShadingType").and_then(PdfObject::as_integer) {
            Some(1) => {
                let matrix = numbers(reader, dictionary, "Matrix");
                let matrix = <Matrix>::try_from(matrix.as_slice()).unwrap_or(IDENTITY);
                let Some(inverse) = invert(&matrix) else {
                    return Ok(None);
                };
                let domain = match domain[..] {
                    [x0, x1, y0, y1] => [x0, x1, y0, y1],
                    _ => [0.0, 1.0, 0.0, 1.0],
                };
                Kind::Function { domain, inverse }
            }
            Some(2) => match coords[..] {
                [x0, y0, x1, y1] => Kind::Axial {
                    coords: [x0, y0, x1, y1],
                },
                _ => return Ok(None),
            },
            Some(3) => match coords[..] {
                [x0, y0, r0, x1, y1, r1] => Kind::Radial {
                    coords: [x0, y0, r0, x1, y1, r1],
                },
                _ => return Ok(None),
            },
            _ => return Ok(None),
        };
        if function.is_none() {
            return Ok(None);
        }
        let extend = match reader.resolve_entry(dictionary, "Extend")? {
            PdfObject::Array(values) => [
                values.first() == Some(&PdfObject::Boolean(true)),
                values.get(1) == Some(&PdfObject::Boolean(true)),
            ],
            _ => [false, false],
        };
        let bounds = match numbers(reader, dictionary, "BBox")[..] {
            [a, b, c, d] => Some([a.min(c), b.min(d), a.max(c), b.max(d)]),
            _ => None,
        };
        let background = numbers(reader, dictionary, "Background");
        let background = (!background.is_empty()).then(|| space.rgb(&background));
        let domain = match (&kind, domain.as_slice()) {
            (Kind::Function { .. }, _) => [0.0, 1.0],
            (_, &[t0, t1]) => [t0, t1],
            _ => [0.0, 1.0],
        };
        let mut shading = Self {
            kind,
            space,
            function,
            extend,
            colors: Vec::new(),
            bounds,
            background,
        };
        if !matches!(shading.kind, Kind::Function { .. }) {
            shading.colors = (0..STEPS)
                .map(|step| {
                    let t = domain[0] + (domain[1] - domain[0]) * step as f64 / (STEPS - 1) as f64;
                    shading.color_of(&[t])
                })
                .collect();
        }
        Ok(Some(shading))
    }

    fn color_of(&self, inputs: &[f64]) -> [f64; 3] {
        let components = match &self.function {
            Some(function) => function.evaluate(inputs),
            None => inputs.to_vec(),
        };
        self.space.rgb(&components)
    }

    /// The color at a point of the shading's space, or `None` where the
    /// shading does not paint.
    pub(super) fn color_at(&self, x: f64, y: f64) -> Option<[f64; 3]> {
        if let Some([left, bottom, right, top]) = self.bounds
            && !((left..=right).contains(&x) && (bottom..=top).contains(&y))
        {
            return None;
        }
        // Where along the domain the point falls, from 0 to 1.
        let place = match &self.kind {
            Kind::Function { domain, inverse } => {
                let u = x * inverse[0] + y * inverse[2] + inverse[4];
                let v = x * inverse[1] + y * inverse[3] + inverse[5];
                let inside = |value: f64, low: f64, high: f64| {
                    (low.min(high)..=high.max(low)).contains(&value)
                };
                if !inside(u, domain[0], domain[1]) || !inside(v, domain[2], domain[3]) {
                    return None;
                }
                return Some(self.color_of(&[u, v]));
            }
            Kind::Axial { coords } => {
                let [x0, y0, x1, y1] = *coords;
                let (dx, dy) = (x1 - x0, y1 - y0);
                let length = dx * dx + dy * dy;
                if length == 0.0 {
                    return None;
                }
                ((x - x0) * dx + (y - y0) * dy) / length
            }
            Kind::Radial { coords } => self.radial(coords, x, y)?,
        };
        let place = if place < 0.0 {
            if !self.extend[0] {
                return None;
            }
            0.0
        } else if place > 1.0 {
            if !self.extend[1] {
                return None;
            }
            1.0
        } else {
            place
        };
        let position = place * (STEPS - 1) as f64;
        let low = position.floor() as usize;
        let high = (low + 1).min(STEPS - 1);
        let fraction = position - low as f64;
        let (a, b) = (self.colors[low], self.colors[high]);
        Some([0, 1, 2].map(|index| a[index] + (b[index] - a[index]) * fraction))
    }

    /// For a radial shading, the largest `s` whose circle passes through
    /// the point with a radius that is not negative; it may lie past 0 or
    /// 1, where the shading extends.
    fn radial(&self, coords: &[f64; 6], x: f64, y: f64) -> Option<f64> {
        let [x0, y0, r0, x1, y1, r1] = *coords;
        let (cx, cy, dr) = (x1 - x0, y1 - y0, r1 - r0);
        let (px, py) = (x - x0, y - y0);
        let a = cx * cx + cy * cy - dr * dr;
        let b = px * cx + py * cy + r0 * dr;
        let c = px * px + py * py - r0 * r0;
        let usable = |s: f64| {
            r0 + s * dr >= 0.0 && (s <= 1.0 || self.extend[1]) && (s >= 0.0 || self.extend[0])
        };
        if a.abs() < 1e-12 {
            if b.abs() < 1e-12 {
                return None;
            }
            let s = c / (2.0 * b);
            return usable(s).then_some(s);
        }
        let discriminant = b * b - a * c;
        if discriminant < 0.0 {
            return None;
        }
        let root = discriminant.sqrt();
        let (first, second) = ((b + root) / a, (b - root) / a);
        let (large, small) = (first.max(second), first.min(second));
        if usable(large) {
            Some(large)
        } else if usable(small) {
            Some(small)
        } else {
            None
        }
    }
}
