//! Functions (ISO 32000-2 section 7.10): sampled, exponential, stitching,
//! and PostScript calculator functions, which shadings and color spaces
//! use to compute colors.

use super::reader::{Reader, entry};
use crate::PdfObject;

/// How deeply stitching functions may nest.
const MAX_DEPTH: usize = 8;
/// The most samples a sampled function may hold.
const MAX_SAMPLES: usize = 1 << 22;
/// The most operations a calculator function may run for one evaluation.
const MAX_STEPS: usize = 10_000;

/// A function from some numbers to others.
#[derive(Debug, Clone)]
pub(super) enum Function {
    /// Type 0: samples on a grid, interpolated linearly.
    Sampled {
        domain: Vec<f64>,
        range: Vec<f64>,
        size: Vec<usize>,
        encode: Vec<f64>,
        decode: Vec<f64>,
        /// Each sample's outputs, from 0 to 1, grid point after grid point
        /// with the first input varying fastest.
        samples: Vec<f64>,
    },
    /// Type 2: `c0 + x^exponent * (c1 - c0)`.
    Exponential {
        domain: Vec<f64>,
        range: Vec<f64>,
        c0: Vec<f64>,
        c1: Vec<f64>,
        exponent: f64,
    },
    /// Type 3: one input split among other functions.
    Stitching {
        domain: Vec<f64>,
        range: Vec<f64>,
        functions: Vec<Function>,
        bounds: Vec<f64>,
        encode: Vec<f64>,
    },
    /// Type 4: a program in a small part of PostScript.
    Calculator {
        domain: Vec<f64>,
        range: Vec<f64>,
        program: Vec<Instruction>,
    },
    /// An array of functions of one output each, taken together.
    Several(Vec<Function>),
}

/// One step of a calculator function.
#[derive(Debug, Clone)]
pub(super) enum Instruction {
    Number(f64),
    Operator(Vec<u8>),
    /// `if` or `ifelse`, with its procedures.
    If(Vec<Instruction>, Option<Vec<Instruction>>),
}

/// A value on a calculator function's stack.
#[derive(Clone, Copy)]
enum Value {
    Number(f64),
    Boolean(bool),
}

impl Value {
    fn number(self) -> f64 {
        match self {
            Self::Number(value) => value,
            Self::Boolean(value) => f64::from(u8::from(value)),
        }
    }
}

fn numbers(reader: &mut Reader, dictionary: &crate::Dictionary, key: &str) -> Vec<f64> {
    match reader.resolve_entry(dictionary, key) {
        Ok(PdfObject::Array(values)) => values.iter().filter_map(PdfObject::as_number).collect(),
        _ => Vec::new(),
    }
}

/// Read a function from its dictionary or stream, or from an array of
/// functions. A function that cannot be read gives `None`.
pub(super) fn load(reader: &mut Reader, object: &PdfObject, depth: usize) -> Option<Function> {
    if depth > MAX_DEPTH {
        return None;
    }
    let number = match object {
        PdfObject::Reference(reference) => Some(reference.number),
        _ => None,
    };
    let resolved = reader.resolve(object).ok()?;
    if let PdfObject::Array(parts) = &resolved {
        let functions: Option<Vec<Function>> = parts
            .iter()
            .map(|part| load(reader, part, depth + 1))
            .collect();
        return functions.map(Function::Several);
    }
    let dictionary = resolved.as_dictionary()?;
    let domain = numbers(reader, dictionary, "Domain");
    let range = numbers(reader, dictionary, "Range");
    if domain.len() < 2 {
        return None;
    }
    match entry(dictionary, "FunctionType")?.as_integer()? {
        0 => {
            let size: Vec<usize> = numbers(reader, dictionary, "Size")
                .iter()
                .map(|&value| value.max(1.0) as usize)
                .collect();
            let inputs = domain.len() / 2;
            let outputs = range.len() / 2;
            let bits = entry(dictionary, "BitsPerSample")?.as_integer()?;
            if size.len() != inputs
                || outputs == 0
                || !matches!(bits, 1 | 2 | 4 | 8 | 12 | 16 | 24 | 32)
            {
                return None;
            }
            let count = size
                .iter()
                .try_fold(outputs, |count, &side| count.checked_mul(side))?;
            if count > MAX_SAMPLES {
                return None;
            }
            let data = reader.stream(number?).ok()??;
            let bits = bits as usize;
            let highest = ((1_u64 << bits) - 1) as f64;
            let samples = (0..count)
                .map(|index| {
                    // Samples are packed without padding, high bits first.
                    let mut value = 0_u64;
                    for bit in index * bits..(index + 1) * bits {
                        let byte = data.get(bit / 8).copied().unwrap_or(0);
                        value = value << 1 | u64::from(byte >> (7 - bit % 8) & 1);
                    }
                    value as f64 / highest
                })
                .collect();
            let mut encode = numbers(reader, dictionary, "Encode");
            if encode.len() != inputs * 2 {
                encode = size
                    .iter()
                    .flat_map(|&side| [0.0, (side - 1) as f64])
                    .collect();
            }
            let mut decode = numbers(reader, dictionary, "Decode");
            if decode.len() != outputs * 2 {
                decode = range.clone();
            }
            Some(Function::Sampled {
                domain,
                range,
                size,
                encode,
                decode,
                samples,
            })
        }
        2 => {
            let mut c0 = numbers(reader, dictionary, "C0");
            let mut c1 = numbers(reader, dictionary, "C1");
            if c0.is_empty() {
                c0 = vec![0.0];
            }
            if c1.is_empty() {
                c1 = vec![1.0];
            }
            let exponent = reader.resolve_entry(dictionary, "N").ok()?.as_number()?;
            Some(Function::Exponential {
                domain,
                range,
                c0,
                c1,
                exponent,
            })
        }
        3 => {
            let PdfObject::Array(parts) = reader.resolve_entry(dictionary, "Functions").ok()?
            else {
                return None;
            };
            let functions: Vec<Function> = parts
                .iter()
                .map(|part| load(reader, part, depth + 1))
                .collect::<Option<_>>()?;
            let bounds = numbers(reader, dictionary, "Bounds");
            let encode = numbers(reader, dictionary, "Encode");
            if functions.is_empty()
                || bounds.len() + 1 != functions.len()
                || encode.len() != functions.len() * 2
            {
                return None;
            }
            Some(Function::Stitching {
                domain,
                range,
                functions,
                bounds,
                encode,
            })
        }
        4 => {
            let data = reader.stream(number?).ok()??;
            let mut tokens = tokens(&data).into_iter();
            if tokens.next().as_deref() != Some(b"{".as_slice()) {
                return None;
            }
            let program = procedure(&mut tokens, 0)?;
            Some(Function::Calculator {
                domain,
                range,
                program,
            })
        }
        _ => None,
    }
}

/// Split a calculator program into braces, numbers, and operators.
fn tokens(data: &[u8]) -> Vec<Vec<u8>> {
    let mut tokens = Vec::new();
    let mut current = Vec::new();
    for &byte in data {
        match byte {
            b'{' | b'}' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                tokens.push(vec![byte]);
            }
            b' ' | b'\t' | b'\r' | b'\n' | b'\x0C' | 0 => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(byte),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Read a procedure up to its closing brace; `if` and `ifelse` take the
/// procedures before them.
fn procedure(tokens: &mut impl Iterator<Item = Vec<u8>>, depth: usize) -> Option<Vec<Instruction>> {
    if depth > MAX_DEPTH {
        return None;
    }
    let mut instructions = Vec::new();
    let mut pending: Vec<Vec<Instruction>> = Vec::new();
    while let Some(token) = tokens.next() {
        match token.as_slice() {
            b"{" => pending.push(procedure(tokens, depth + 1)?),
            b"}" => return Some(instructions),
            b"if" => {
                let then = pending.pop()?;
                instructions.push(Instruction::If(then, None));
            }
            b"ifelse" => {
                let otherwise = pending.pop()?;
                let then = pending.pop()?;
                instructions.push(Instruction::If(then, Some(otherwise)));
            }
            _ => match std::str::from_utf8(&token)
                .ok()
                .and_then(|text| text.parse::<f64>().ok())
            {
                Some(value) => instructions.push(Instruction::Number(value)),
                None => instructions.push(Instruction::Operator(token)),
            },
        }
    }
    None
}

fn clip(values: &mut [f64], limits: &[f64]) {
    for (index, value) in values.iter_mut().enumerate() {
        if let (Some(&low), Some(&high)) = (limits.get(index * 2), limits.get(index * 2 + 1)) {
            *value = value.clamp(low.min(high), high.max(low));
        }
    }
}

/// Where `x` falls on the way from `from` to `to`, mapped to the same place
/// between `low` and `high`.
fn interpolate(x: f64, from: f64, to: f64, low: f64, high: f64) -> f64 {
    if to == from {
        return low;
    }
    low + (x - from) * (high - low) / (to - from)
}

impl Function {
    /// The outputs for some inputs; inputs outside the domain are clipped
    /// to it, and outputs to the range when there is one.
    pub(super) fn evaluate(&self, inputs: &[f64]) -> Vec<f64> {
        match self {
            Self::Several(functions) => functions
                .iter()
                .flat_map(|function| function.evaluate(inputs))
                .collect(),
            Self::Exponential {
                domain,
                range,
                c0,
                c1,
                exponent,
            } => {
                let mut x = inputs.first().copied().unwrap_or(0.0);
                x = x.clamp(domain[0].min(domain[1]), domain[1].max(domain[0]));
                let power = x.powf(*exponent);
                let power = if power.is_finite() { power } else { 0.0 };
                let mut outputs: Vec<f64> = c0
                    .iter()
                    .zip(c1.iter().chain(std::iter::repeat(&1.0)))
                    .map(|(&low, &high)| low + power * (high - low))
                    .collect();
                clip(&mut outputs, range);
                outputs
            }
            Self::Stitching {
                domain,
                range,
                functions,
                bounds,
                encode,
            } => {
                let x = inputs.first().copied().unwrap_or(0.0);
                let x = x.clamp(domain[0].min(domain[1]), domain[1].max(domain[0]));
                let index = bounds.iter().take_while(|&&bound| x >= bound).count();
                let from = if index == 0 {
                    domain[0]
                } else {
                    bounds[index - 1]
                };
                let to = bounds.get(index).copied().unwrap_or(domain[1]);
                let x = interpolate(x, from, to, encode[index * 2], encode[index * 2 + 1]);
                let mut outputs = functions[index].evaluate(&[x]);
                clip(&mut outputs, range);
                outputs
            }
            Self::Sampled {
                domain,
                range,
                size,
                encode,
                decode,
                samples,
            } => {
                let outputs = range.len() / 2;
                // Each input becomes a position on its side of the grid.
                let mut positions = Vec::with_capacity(size.len());
                for (index, &side) in size.iter().enumerate() {
                    let mut x = inputs.get(index).copied().unwrap_or(0.0);
                    let (low, high) = (domain[index * 2], domain[index * 2 + 1]);
                    x = x.clamp(low.min(high), high.max(low));
                    let e = interpolate(x, low, high, encode[index * 2], encode[index * 2 + 1]);
                    positions.push(e.clamp(0.0, (side - 1) as f64));
                }
                // Blend the samples at the corners of the grid cell.
                let mut result = vec![0.0; outputs];
                let corners = 1_usize << size.len().min(8);
                for corner in 0..corners {
                    let mut weight = 1.0;
                    let mut offset = 0;
                    let mut stride = 1;
                    for (index, (&position, &side)) in positions.iter().zip(size).enumerate() {
                        let low = position.floor() as usize;
                        let fraction = position - low as f64;
                        let high = (low + 1).min(side - 1);
                        let (point, part) = if corner >> index & 1 == 1 {
                            (high, fraction)
                        } else {
                            (low, 1.0 - fraction)
                        };
                        weight *= part;
                        offset += point * stride;
                        stride *= side;
                    }
                    if weight == 0.0 {
                        continue;
                    }
                    for (output, value) in result.iter_mut().enumerate() {
                        *value += weight
                            * samples
                                .get(offset * outputs + output)
                                .copied()
                                .unwrap_or(0.0);
                    }
                }
                for (output, value) in result.iter_mut().enumerate() {
                    *value =
                        interpolate(*value, 0.0, 1.0, decode[output * 2], decode[output * 2 + 1]);
                }
                clip(&mut result, range);
                result
            }
            Self::Calculator {
                domain,
                range,
                program,
            } => {
                let mut stack: Vec<Value> = inputs
                    .iter()
                    .enumerate()
                    .map(|(index, &value)| {
                        let (low, high) = (
                            domain.get(index * 2).copied().unwrap_or(f64::NEG_INFINITY),
                            domain.get(index * 2 + 1).copied().unwrap_or(f64::INFINITY),
                        );
                        Value::Number(value.clamp(low.min(high), high.max(low)))
                    })
                    .collect();
                let mut steps = 0;
                run(program, &mut stack, &mut steps);
                let outputs = range.len() / 2;
                let start = stack.len().saturating_sub(outputs);
                let mut result: Vec<f64> =
                    stack[start..].iter().map(|value| value.number()).collect();
                result.resize(outputs, 0.0);
                clip(&mut result, range);
                result
            }
        }
    }
}

/// Run calculator instructions on a stack. Errors, such as too few
/// operands, leave the stack as it is.
fn run(program: &[Instruction], stack: &mut Vec<Value>, steps: &mut usize) {
    for instruction in program {
        *steps += 1;
        if *steps > MAX_STEPS || stack.len() > 1000 {
            return;
        }
        match instruction {
            Instruction::Number(value) => stack.push(Value::Number(*value)),
            Instruction::If(then, otherwise) => {
                let Some(Value::Boolean(condition)) = stack.pop() else {
                    return;
                };
                if condition {
                    run(then, stack, steps);
                } else if let Some(otherwise) = otherwise {
                    run(otherwise, stack, steps);
                }
            }
            Instruction::Operator(name) => {
                if operate(name, stack).is_none() {
                    return;
                }
            }
        }
    }
}

/// Apply one calculator operator; `None` when its operands are missing.
fn operate(name: &[u8], stack: &mut Vec<Value>) -> Option<()> {
    let number = |value: Option<Value>| value.map(Value::number);
    let result = match name {
        b"true" => Value::Boolean(true),
        b"false" => Value::Boolean(false),
        b"abs" | b"neg" | b"ceiling" | b"floor" | b"round" | b"truncate" | b"sqrt" | b"sin"
        | b"cos" | b"ln" | b"log" | b"cvi" | b"cvr" => {
            let x = number(stack.pop())?;
            Value::Number(match name {
                b"abs" => x.abs(),
                b"neg" => -x,
                b"ceiling" => x.ceil(),
                b"floor" => x.floor(),
                b"round" => (x + 0.5).floor(),
                b"truncate" | b"cvi" => x.trunc(),
                b"sqrt" => x.max(0.0).sqrt(),
                // Angles are in degrees.
                b"sin" => x.to_radians().sin(),
                b"cos" => x.to_radians().cos(),
                b"ln" => x.ln(),
                b"log" => x.log10(),
                _ => x,
            })
        }
        b"not" => match stack.pop()? {
            Value::Boolean(value) => Value::Boolean(!value),
            Value::Number(value) => Value::Number(!(value as i64) as f64),
        },
        b"add" | b"sub" | b"mul" | b"div" | b"idiv" | b"mod" | b"exp" | b"atan" | b"bitshift" => {
            let b = number(stack.pop())?;
            let a = number(stack.pop())?;
            Value::Number(match name {
                b"add" => a + b,
                b"sub" => a - b,
                b"mul" => a * b,
                b"div" if b != 0.0 => a / b,
                b"idiv" if b as i64 != 0 => ((a as i64) / (b as i64)) as f64,
                b"mod" if b as i64 != 0 => ((a as i64) % (b as i64)) as f64,
                b"exp" => a.powf(b),
                // `num den atan`: the angle whose tangent is num / den,
                // in degrees from 0 to 360.
                b"atan" => {
                    let degrees = a.atan2(b).to_degrees();
                    if degrees < 0.0 {
                        degrees + 360.0
                    } else {
                        degrees
                    }
                }
                b"bitshift" => {
                    let (value, shift) = (a as i64, b as i64);
                    if shift >= 0 {
                        (value << shift.min(63)) as f64
                    } else {
                        (value >> (-shift).min(63)) as f64
                    }
                }
                _ => 0.0,
            })
        }
        b"eq" | b"ne" | b"gt" | b"ge" | b"lt" | b"le" => {
            let b = number(stack.pop())?;
            let a = number(stack.pop())?;
            Value::Boolean(match name {
                b"eq" => a == b,
                b"ne" => a != b,
                b"gt" => a > b,
                b"ge" => a >= b,
                b"lt" => a < b,
                _ => a <= b,
            })
        }
        b"and" | b"or" | b"xor" => {
            let b = stack.pop()?;
            let a = stack.pop()?;
            match (a, b) {
                (Value::Boolean(a), Value::Boolean(b)) => Value::Boolean(match name {
                    b"and" => a && b,
                    b"or" => a || b,
                    _ => a != b,
                }),
                (a, b) => {
                    let (a, b) = (a.number() as i64, b.number() as i64);
                    Value::Number(match name {
                        b"and" => a & b,
                        b"or" => a | b,
                        _ => a ^ b,
                    } as f64)
                }
            }
        }
        b"pop" => {
            stack.pop()?;
            return Some(());
        }
        b"dup" => *stack.last()?,
        b"exch" => {
            let b = stack.pop()?;
            let a = stack.pop()?;
            stack.push(b);
            a
        }
        b"copy" => {
            let count = number(stack.pop())?.max(0.0) as usize;
            let start = stack.len().checked_sub(count)?;
            let copied: Vec<Value> = stack[start..].to_vec();
            stack.extend(copied);
            return Some(());
        }
        b"index" => {
            let depth = number(stack.pop())?.max(0.0) as usize;
            let index = stack.len().checked_sub(depth + 1)?;
            stack[index]
        }
        b"roll" => {
            let shift = number(stack.pop())? as i64;
            let count = number(stack.pop())?.max(0.0) as usize;
            let start = stack.len().checked_sub(count)?;
            if count > 0 {
                let part = &mut stack[start..];
                let shift = shift.rem_euclid(count as i64) as usize;
                part.rotate_right(shift);
            }
            return Some(());
        }
        _ => return None,
    };
    stack.push(result);
    Some(())
}
