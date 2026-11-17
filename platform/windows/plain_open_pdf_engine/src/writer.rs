//! Writing PDF object syntax (ISO 32000-2 section 7.3).

use crate::PdfObject;

/// Append an object in PDF syntax, so that [`parse_object`](crate::parse_object)
/// reads the same value back.
///
/// A string is written in literal form when it holds only printable ASCII
/// and in hexadecimal form otherwise. A real number is written in full,
/// without an exponent.
pub fn write_object(object: &PdfObject, output: &mut Vec<u8>) {
    match object {
        PdfObject::Null => output.extend(b"null"),
        PdfObject::Boolean(value) => output.extend(value.to_string().as_bytes()),
        PdfObject::Integer(value) => output.extend(value.to_string().as_bytes()),
        // PDF has no syntax for infinities or NaN; they are written as 0.
        PdfObject::Real(value) if value.is_finite() => output.extend(value.to_string().as_bytes()),
        PdfObject::Real(_) => output.push(b'0'),
        PdfObject::Name(name) => write_name(name, output),
        PdfObject::String(bytes) => write_string(bytes, output),
        PdfObject::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(b' ');
                }
                write_object(value, output);
            }
            output.push(b']');
        }
        PdfObject::Dictionary(entries) => {
            output.extend(b"<<");
            for (key, value) in entries {
                output.push(b' ');
                write_name(key, output);
                output.push(b' ');
                write_object(value, output);
            }
            output.extend(b" >>");
        }
        PdfObject::Reference(reference) => {
            let text = format!("{} {} R", reference.number, reference.generation);
            output.extend(text.as_bytes());
        }
    }
}

/// A name, with `#xx` for every byte that cannot stand for itself.
fn write_name(name: &[u8], output: &mut Vec<u8>) {
    output.push(b'/');
    for &byte in name {
        if byte.is_ascii_graphic() && !b"()<>[]{}/%#".contains(&byte) {
            output.push(byte);
        } else {
            output.extend(format!("#{byte:02X}").as_bytes());
        }
    }
}

fn write_string(bytes: &[u8], output: &mut Vec<u8>) {
    if bytes.iter().all(|byte| matches!(byte, 0x20..=0x7E)) {
        output.push(b'(');
        for &byte in bytes {
            if matches!(byte, b'(' | b')' | 0x5C) {
                output.push(0x5C);
            }
            output.push(byte);
        }
        output.push(b')');
    } else {
        output.push(b'<');
        for byte in bytes {
            output.extend(format!("{byte:02X}").as_bytes());
        }
        output.push(b'>');
    }
}
