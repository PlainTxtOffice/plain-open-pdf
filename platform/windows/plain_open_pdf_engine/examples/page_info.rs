//! Print what the engine reads from each PDF file named on the command
//! line, as tab-separated lines: the page count, then one line per page.
//!
//! Options, given before the file names:
//!
//! - `--password TEXT` supplies a password.
//! - `--info` also prints the document information entries.
//! - `--outline` also prints each outline item's level, page, and title.
//! - `--streams` also prints the decoded length and a checksum of every
//!   stream, or the reason it could not be decoded.
//! - `--links` also prints each page's links: page, rectangle, and target.
//! - `--attachments` also prints each embedded file's names, length, and
//!   checksum.
//! - `--content` also prints each page's content length, checksum, and
//!   number of operations.
//! - `--cmaps` also prints the number of mappings in each ToUnicode CMap.
//! - `--text` also prints each page's extracted text.

use plain_open_pdf_engine::{
    Document, LinkTarget, ObjectReference, PdfObject, ToUnicodeMap, parse_content,
};

fn main() {
    let mut arguments: Vec<String> = std::env::args().skip(1).collect();
    let mut password = String::new();
    let (mut info, mut outline, mut streams) = (false, false, false);
    let (mut links, mut attachments, mut content) = (false, false, false);
    let (mut cmaps, mut text) = (false, false);
    loop {
        match arguments.first().map(String::as_str) {
            Some("--password") if arguments.len() > 1 => {
                password = arguments.remove(1);
            }
            Some("--info") => info = true,
            Some("--outline") => outline = true,
            Some("--streams") => streams = true,
            Some("--links") => links = true,
            Some("--attachments") => attachments = true,
            Some("--content") => content = true,
            Some("--cmaps") => cmaps = true,
            Some("--text") => text = true,
            _ => break,
        }
        arguments.remove(0);
    }
    for path in arguments {
        let document = std::fs::read(&path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                Document::open_with_password(bytes, password.as_bytes())
                    .map_err(|error| error.to_string())
            });
        let mut document = match document {
            Ok(document) => document,
            Err(error) => {
                println!("{path}\tERROR {error}");
                continue;
            }
        };
        println!("{path}\t{}", document.page_count());
        for page in document.pages() {
            let media = page.media_box;
            let crop = page.crop_box;
            println!(
                "\t{} {} {} {}\t{} {} {} {}\t{}",
                media.left,
                media.bottom,
                media.right,
                media.top,
                crop.left,
                crop.bottom,
                crop.right,
                crop.top,
                page.rotation
            );
        }
        if info {
            print_information(&mut document);
        }
        if outline {
            for item in document.outline().unwrap_or_default() {
                let page = item.page.map_or(-1, |page| page as i64 + 1);
                println!("outline\t{}\t{page}\t{:?}", item.level, item.title);
            }
        }
        if streams {
            print_streams(&mut document);
        }
        if links {
            print_links(&mut document);
        }
        if attachments {
            print_attachments(&mut document);
        }
        if content {
            print_content(&mut document);
        }
        if cmaps {
            print_cmaps(&mut document);
        }
        if text {
            for index in 0..document.page_count() {
                match document.page_text(index) {
                    Ok(text) => println!("text\t{index}\t{text:?}"),
                    Err(error) => println!("text\t{index}\tERROR {error}"),
                }
            }
        }
    }
}

fn print_information(document: &mut Document) {
    let Ok(information) = document.information() else {
        return;
    };
    let entries = [
        ("title", information.title),
        ("author", information.author),
        ("subject", information.subject),
        ("keywords", information.keywords),
        ("creator", information.creator),
        ("producer", information.producer),
        ("creationDate", information.creation_date),
        ("modDate", information.modification_date),
    ];
    for (name, value) in entries {
        if let Some(value) = value {
            println!("info\t{name}\t{value:?}");
        }
    }
}

/// The FNV-1a hash of some bytes.
fn checksum(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Print the decoded length and checksum of each stream object.
fn print_streams(document: &mut Document) {
    let size = match document.trailer().get(b"Size".as_slice()) {
        Some(PdfObject::Integer(size)) => u32::try_from(*size).unwrap_or(0),
        _ => 0,
    };
    for number in 1..size {
        let reference = ObjectReference {
            number,
            generation: 0,
        };
        match document.stream(reference) {
            Ok(Some(data)) => {
                println!("stream\t{number}\t{}\t{:016x}", data.len(), checksum(&data));
            }
            Ok(None) => {}
            Err(error) => println!("stream\t{number}\tERROR {error}"),
        }
    }
}

/// Print each link's page index, rectangle, and target.
fn print_links(document: &mut Document) {
    for index in 0..document.page_count() {
        for link in document.links(index).unwrap_or_default() {
            let target = match link.target {
                LinkTarget::Page(page) => format!("page\t{page}"),
                LinkTarget::Uri(uri) => format!("uri\t{uri:?}"),
                LinkTarget::RemoteDocument { file, page } => {
                    format!("remote\t{file:?}\t{}", page.map_or(-1, |page| page as i64))
                }
                LinkTarget::Launch(file) => format!("launch\t{file:?}"),
                LinkTarget::Named(name) => format!("named\t{name:?}"),
            };
            let area = link.rectangle;
            println!(
                "link\t{index}\t{} {} {} {}\t{target}",
                area.left, area.bottom, area.right, area.top
            );
        }
    }
}

/// Print each embedded file's key, file name, length, and checksum.
fn print_attachments(document: &mut Document) {
    for attachment in document.attachments().unwrap_or_default() {
        let data = attachment
            .stream
            .and_then(|stream| document.stream(stream).ok()?)
            .unwrap_or_default();
        println!(
            "attachment\t{:?}\t{:?}\t{}\t{:016x}",
            attachment.name,
            attachment.file_name.unwrap_or_default(),
            data.len(),
            checksum(&data)
        );
    }
}

/// Print each page's content length, checksum, and operation count.
fn print_content(document: &mut Document) {
    for index in 0..document.page_count() {
        match document.page_content(index) {
            Ok(data) => {
                let operations = match parse_content(&data) {
                    Ok(operations) => operations.len().to_string(),
                    Err(error) => format!("ERROR {error}"),
                };
                println!(
                    "content\t{index}\t{}\t{:016x}\t{operations}",
                    data.len(),
                    checksum(&data)
                );
            }
            Err(error) => println!("content\t{index}\tERROR {error}"),
        }
    }
}

/// Print the mapping count of every stream that is a CMap.
fn print_cmaps(document: &mut Document) {
    let size = match document.trailer().get(b"Size".as_slice()) {
        Some(PdfObject::Integer(size)) => u32::try_from(*size).unwrap_or(0),
        _ => 0,
    };
    for number in 1..size {
        let reference = ObjectReference {
            number,
            generation: 0,
        };
        let Ok(Some(data)) = document.stream(reference) else {
            continue;
        };
        if data.windows(9).any(|window| window == b"begincmap") {
            match ToUnicodeMap::parse(&data) {
                Ok(map) => println!("cmap\t{number}\t{}", map.len()),
                Err(error) => println!("cmap\t{number}\tERROR {error}"),
            }
        }
    }
}
