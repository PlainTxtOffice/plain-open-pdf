//! Opening whole documents built from original fixtures.

mod common;

use plain_open_pdf_engine::{
    Document, DocumentError, Information, LinkTarget, ObjectReference, OutlineItem, PdfObject,
    PdfVersion, PixelRegion, Rectangle, View, decode_text_string,
};

/// Assembles a PDF file and tracks where each object starts.
struct Builder {
    bytes: Vec<u8>,
    offsets: Vec<(u32, usize)>,
}

impl Builder {
    fn new() -> Self {
        Self {
            bytes: b"%PDF-1.7\n".to_vec(),
            offsets: Vec::new(),
        }
    }

    fn object(&mut self, number: u32, body: &str) {
        self.offsets.push((number, self.bytes.len()));
        let text = format!("{number} 0 obj\n{body}\nendobj\n");
        self.bytes.extend(text.as_bytes());
    }

    fn stream(&mut self, number: u32, entries: &str, data: &[u8]) {
        self.offsets.push((number, self.bytes.len()));
        let head = format!("{number} 0 obj\n<< {entries} >>\nstream\n");
        self.bytes.extend(head.as_bytes());
        self.bytes.extend(data);
        self.bytes.extend(b"\nendstream\nendobj\n");
    }

    fn offset(&self, number: u32) -> usize {
        let found = self.offsets.iter().rev().find(|entry| entry.0 == number);
        found.expect("object was written").1
    }

    /// Append a cross-reference table for the given object numbers.
    fn table(&mut self, numbers: &[u32], trailer: &str) -> usize {
        let start = self.bytes.len();
        let mut text = String::from("xref\n");
        for &number in numbers {
            let offset = self.offset(number);
            text += &format!("{number} 1\n{offset:010} 00000 n \n");
        }
        text += &format!("trailer\n<< /Root 1 0 R {trailer} >>\nstartxref\n{start}\n%%EOF\n");
        self.bytes.extend(text.as_bytes());
        start
    }
}

/// A three-page document that exercises inherited page attributes.
fn sample() -> Builder {
    let mut builder = Builder::new();
    builder.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
    builder.object(
        2,
        "<< /Type /Pages /Count 3 /Kids [3 0 R 4 0 R] /MediaBox [0 0 612 792] /Rotate 90 >>",
    );
    builder.object(3, "<< /Type /Page /Parent 2 0 R >>");
    builder.object(
        4,
        "<< /Type /Pages /Parent 2 0 R /Kids [5 0 R 6 0 R] /MediaBox [0 0 200 100] >>",
    );
    builder.object(
        5,
        "<< /Type /Page /Parent 4 0 R /Rotate 0 /CropBox [10 10 50 500] >>",
    );
    builder.object(
        6,
        "<< /Type /Page /Parent 4 0 R /MediaBox [300 400 100.5 200] /Rotate -90 >>",
    );
    builder
}

fn sample_file() -> Vec<u8> {
    let mut builder = sample();
    builder.table(&[1, 2, 3, 4, 5, 6], "/Size 7");
    builder.bytes
}

fn rectangle(left: f64, bottom: f64, right: f64, top: f64) -> Rectangle {
    Rectangle {
        left,
        bottom,
        right,
        top,
    }
}

fn assert_sample_pages(document: &Document) {
    let pages = document.pages();
    assert_eq!(document.page_count(), 3);
    let numbers: Vec<_> = pages
        .iter()
        .map(|page| page.reference.map(|reference| reference.number))
        .collect();
    assert_eq!(numbers, [Some(3), Some(5), Some(6)]);

    assert_eq!(pages[0].media_box, rectangle(0.0, 0.0, 612.0, 792.0));
    assert_eq!(pages[0].crop_box, pages[0].media_box);
    assert_eq!(pages[0].rotation, 90);

    assert_eq!(pages[1].media_box, rectangle(0.0, 0.0, 200.0, 100.0));
    assert_eq!(pages[1].crop_box, rectangle(10.0, 10.0, 50.0, 100.0));
    assert_eq!(pages[1].rotation, 0);

    assert_eq!(pages[2].media_box, rectangle(100.5, 200.0, 300.0, 400.0));
    assert_eq!(pages[2].media_box.width(), 199.5);
    assert_eq!(pages[2].rotation, 270);
}

/// Wrap data in a zlib stream made of one uncompressed block.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let length = u16::try_from(data.len()).expect("fixture fits one block");
    let mut stream = vec![0x78, 0x01, 0x01];
    stream.extend(length.to_le_bytes());
    stream.extend((!length).to_le_bytes());
    stream.extend(data);
    stream.extend([0; 4]);
    stream
}

/// Apply the PNG "up" predictor to rows of `columns` bytes.
fn png_up(data: &[u8], columns: usize) -> Vec<u8> {
    let mut encoded = Vec::new();
    let mut previous = vec![0u8; columns];
    for row in data.chunks(columns) {
        encoded.push(2);
        for (index, &byte) in row.iter().enumerate() {
            encoded.push(byte.wrapping_sub(previous[index]));
        }
        previous[..row.len()].copy_from_slice(row);
    }
    encoded
}

/// The sample pages stored in an object stream and located by a
/// cross-reference stream, as PDF 1.5 and later files usually are.
fn compressed_file(object_stream_length: &str) -> Vec<u8> {
    let bodies = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /MediaBox [0 0 612 792] /Rotate 90 >>",
        "<< /Type /Page /Parent 2 0 R >>",
        "<< /Type /Pages /Parent 2 0 R /Kids [5 0 R 6 0 R] /MediaBox [0 0 200 100] >>",
        "<< /Type /Page /Parent 4 0 R /Rotate 0 /CropBox [10 10 50 500] >>",
        "<< /Type /Page /Parent 4 0 R /MediaBox [300 400 100.5 200] /Rotate -90 >>",
    ];
    let mut header = String::new();
    let mut content = String::new();
    for (index, body) in bodies.iter().enumerate() {
        header += &format!("{} {} ", index + 1, content.len());
        content += body;
        content.push('\n');
    }
    let data = zlib_stored(format!("{header}{content}").as_bytes());

    let mut builder = Builder::new();
    let length = object_stream_length.replace("LENGTH", &data.len().to_string());
    builder.stream(
        7,
        &format!(
            "/Type /ObjStm /N 6 /First {} /Filter /FlateDecode /Length {length}",
            header.len()
        ),
        &data,
    );
    builder.object(9, &data.len().to_string());

    // Rows are type, two-byte field, one-byte field, for objects 0 through 9.
    let start = builder.bytes.len();
    let mut rows = vec![0, 0, 0, 0];
    for index in 0..6 {
        rows.extend([2, 0, 7, index]);
    }
    for offset in [builder.offset(7), 0, builder.offset(9)] {
        let [high, low] = u16::try_from(offset).expect("small fixture").to_be_bytes();
        rows.extend([u8::from(offset != 0), high, low, 0]);
    }
    rows[8 * 4..9 * 4].copy_from_slice(&[1, 0, 0, 0]);
    let [high, low] = u16::try_from(start).expect("small fixture").to_be_bytes();
    rows[8 * 4..9 * 4].copy_from_slice(&[1, high, low, 0]);
    let data = zlib_stored(&png_up(&rows, 4));
    builder.stream(
        8,
        &format!(
            "/Type /XRef /Size 10 /W [1 2 1] /Root 1 0 R /Filter /FlateDecode \
             /DecodeParms << /Predictor 12 /Columns 4 >> /Length {}",
            data.len()
        ),
        &data,
    );
    let tail = format!("startxref\n{start}\n%%EOF\n");
    builder.bytes.extend(tail.as_bytes());
    builder.bytes
}

#[test]
fn reads_pages_with_inherited_attributes_from_a_table() {
    let document = Document::open(sample_file()).unwrap();
    assert_eq!(document.version(), PdfVersion { major: 1, minor: 7 });
    assert_sample_pages(&document);
    assert_eq!(
        document.trailer()[b"Size".as_slice()],
        PdfObject::Integer(7)
    );
}

#[test]
fn reads_objects_from_object_and_cross_reference_streams() {
    for length in ["LENGTH", "9 0 R", "99999", "0"] {
        let document = Document::open(compressed_file(length)).unwrap();
        assert_sample_pages(&document);
    }
}

#[test]
fn loads_objects_by_reference() {
    for file in [sample_file(), compressed_file("LENGTH")] {
        let mut document = Document::open(file).unwrap();
        let reference = |number| ObjectReference {
            number,
            generation: 0,
        };
        let catalog = document.object(reference(1)).unwrap();
        let catalog = catalog.as_dictionary().expect("catalog is a dictionary");
        assert_eq!(
            catalog[b"Type".as_slice()].as_name(),
            Some(b"Catalog".as_slice())
        );
        assert_eq!(document.object(reference(500)).unwrap(), PdfObject::Null);
    }
}

#[test]
fn newer_sections_override_older_ones() {
    let mut builder = sample();
    let first = builder.table(&[1, 2, 3, 4, 5, 6], "/Size 7");
    builder.object(3, "<< /Type /Page /Parent 2 0 R /Rotate 180 >>");
    builder.table(&[3], &format!("/Size 7 /Prev {first}"));
    let document = Document::open(builder.bytes).unwrap();
    assert_eq!(document.page_count(), 3);
    assert_eq!(document.pages()[0].rotation, 180);
}

#[test]
fn offsets_count_from_the_header() {
    let mut file = b"leading bytes\r\n".to_vec();
    file.extend(sample_file());
    assert_sample_pages(&Document::open(file).unwrap());
    let mut file = b"leading bytes\r\n".to_vec();
    file.extend(compressed_file("LENGTH"));
    assert_sample_pages(&Document::open(file).unwrap());
}

#[test]
fn rebuilds_missing_or_wrong_cross_reference_data() {
    let file = sample_file();
    let table = file
        .windows(4)
        .rposition(|window| window == b"xref")
        .unwrap();
    let keyword = file.windows(9).rposition(|w| w == b"startxref").unwrap();

    // No table, trailer, or startxref at all.
    assert_sample_pages(&Document::open(file[..table - 5].to_vec()).unwrap());
    // A startxref that points nowhere useful.
    let mut wrong = file[..keyword].to_vec();
    wrong.extend(b"startxref\n17\n%%EOF\n");
    assert_sample_pages(&Document::open(wrong).unwrap());
    // Offsets shifted by bytes inserted after the table was written.
    let mut shifted = file[..20].to_vec();
    shifted.extend(b"%inserted\n");
    shifted.extend(&file[20..]);
    assert_sample_pages(&Document::open(shifted).unwrap());
    // A newer section whose /Prev is wrong, hiding the page tree.
    let mut builder = sample();
    builder.table(&[1, 2, 3, 4, 5, 6], "/Size 7");
    builder.object(3, "<< /Type /Page /Parent 2 0 R >>");
    builder.table(&[3], "/Size 7 /Prev 11");
    assert_sample_pages(&Document::open(builder.bytes).unwrap());

    // Compressed objects are found again through their object stream.
    let file = compressed_file("LENGTH");
    let keyword = file.windows(9).rposition(|w| w == b"startxref").unwrap();
    assert_sample_pages(&Document::open(file[..keyword].to_vec()).unwrap());
}

#[test]
fn page_tree_cycles_and_broken_nodes_do_not_stop_reading() {
    let mut builder = Builder::new();
    builder.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
    builder.object(
        2,
        "<< /Type /Pages /Kids [3 0 R 2 0 R 3 0 R 9 0 R 4 0 R null 5 0 R] >>",
    );
    builder.object(3, "<< /Type /Page /Parent 2 0 R >>");
    builder.object(4, "(not a page)");
    builder.object(5, "<< /Type /Page /Kids [3 0 R] /MediaBox [0 0 10 0] >>");
    builder.table(&[1, 2, 3, 4, 5], "/Size 6");
    let document = Document::open(builder.bytes).unwrap();
    assert_eq!(document.page_count(), 2);
    // Without a usable media box a page is US Letter.
    assert_eq!(
        document.pages()[1].media_box,
        rectangle(0.0, 0.0, 612.0, 792.0)
    );
}

#[test]
fn reports_what_cannot_be_opened() {
    assert!(matches!(
        Document::open(Vec::new()),
        Err(DocumentError::Header(_))
    ));
    assert!(matches!(
        Document::open(b"%PDF-1.7\n".to_vec()),
        Err(DocumentError::CrossReference { .. })
    ));

    let mut builder = sample();
    builder.table(&[1, 2, 3, 4, 5, 6], "/Size 7 /Encrypt 8 0 R");
    assert!(matches!(
        Document::open(builder.bytes),
        Err(DocumentError::Structure(_))
    ));
    for handler in ["/Filter /Custom", "/Filter /Standard /V 3 /R 3"] {
        let mut builder = sample();
        let trailer = format!("/Size 7 /Encrypt << {handler} >>");
        builder.table(&[1, 2, 3, 4, 5, 6], &trailer);
        assert_eq!(
            Document::open(builder.bytes).unwrap_err(),
            DocumentError::Unsupported("this encryption method")
        );
    }

    let mut builder = Builder::new();
    builder.object(1, "<< /Type /Font >>");
    builder.table(&[1], "/Size 2");
    assert!(matches!(
        Document::open(builder.bytes),
        Err(DocumentError::Structure(_))
    ));
}

#[test]
fn damaged_files_never_panic() {
    for file in [sample_file(), compressed_file("LENGTH")] {
        for length in 0..file.len() {
            let _ = Document::open(file[..length].to_vec());
        }
        for index in 0..file.len() {
            for byte in [0, b'9', b'<', 0xff] {
                let mut damaged = file.clone();
                damaged[index] = byte;
                let _ = Document::open(damaged);
            }
        }
    }
}

/// The sample document with information, an outline, and encoded streams.
fn described_file() -> Vec<u8> {
    let mut builder = sample();
    // Title in PDFDocEncoding (0x85 is an en dash), author in UTF-16BE.
    builder.object(
        7,
        "<< /Title (Report 2020\\2052024) /Author <FEFF004A00F800720067> \
         /Producer (\\357\\273\\277caf\\303\\251) /CreationDate (D:20240131120000Z) /Trapped /False >>",
    );
    builder.object(
        8,
        "<< /Type /Outlines /First 9 0 R /Last 12 0 R /Count 4 >>",
    );
    builder.object(
        9,
        "<< /Title (Chapter 1) /Parent 8 0 R /Next 12 0 R /First 10 0 R /Last 11 0 R \
         /Dest [3 0 R /Fit] >>",
    );
    builder.object(
        10,
        "<< /Title (Named in tree) /Parent 9 0 R /Next 11 0 R /Dest (beta) >>",
    );
    builder.object(
        11,
        "<< /Title <FEFF00530065006300740069006F006E002000B6> /Parent 9 0 R /Prev 10 0 R \
         /A << /S /GoTo /D /legacy >> >>",
    );
    // The last item points back at the first, which must not loop.
    builder.object(
        12,
        "<< /Title (Elsewhere) /Parent 8 0 R /Prev 9 0 R /Next 9 0 R \
         /A << /S /URI /URI (https://example.org) >> >>",
    );
    builder.object(13, "<< /Kids [14 0 R 15 0 R] >>");
    builder.object(
        14,
        "<< /Limits [(alpha) (alpha)] /Names [(alpha) [3 0 R /Fit]] >>",
    );
    builder.object(
        15,
        "<< /Limits [(beta) (gamma)] /Names [(beta) << /D [5 0 R /XYZ 0 0 0] >> (gamma) [3 0 R /Fit]] >>",
    );
    builder.object(16, "<< /legacy [6 0 R /Fit] >>");
    builder.stream(
        17,
        "/Length 12 /Filter [/ASCIIHexDecode /RunLengthDecode]",
        b"02414243FE44>",
    );
    builder.stream(18, "/Length 5", b"plain");
    builder.object(
        1,
        "<< /Type /Catalog /Pages 2 0 R /Outlines 8 0 R /Names << /Dests 13 0 R >> /Dests 16 0 R >>",
    );
    let numbers: Vec<u32> = (1..=18).collect();
    builder.table(&numbers, "/Size 19 /Info 7 0 R");
    builder.bytes
}

#[test]
fn reads_document_information_as_text() {
    let mut document = Document::open(described_file()).unwrap();
    let information = document.information().unwrap();
    assert_eq!(
        information.title.as_deref(),
        Some("Report 2020\u{2013}2024")
    );
    assert_eq!(information.author.as_deref(), Some("J\u{f8}rg"));
    assert_eq!(information.producer.as_deref(), Some("caf\u{e9}"));
    assert_eq!(
        information.creation_date.as_deref(),
        Some("D:20240131120000Z")
    );
    assert_eq!(information.subject, None);
    assert_eq!(information.modification_date, None);

    let mut plain = Document::open(sample_file()).unwrap();
    assert_eq!(plain.information().unwrap(), Information::default());
    assert_eq!(plain.outline().unwrap(), []);
}

#[test]
fn reads_outline_levels_titles_and_destination_pages() {
    let mut document = Document::open(described_file()).unwrap();
    let outline: Vec<_> = document
        .outline()
        .unwrap()
        .into_iter()
        .map(|item| (item.level, item.title, item.page))
        .collect();
    let expected = [
        (1, "Chapter 1", Some(0)),
        (2, "Named in tree", Some(1)),
        (2, "Section \u{b6}", Some(2)),
        (1, "Elsewhere", None),
    ];
    let expected: Vec<_> = expected
        .into_iter()
        .map(|(level, title, page)| (level, title.to_string(), page))
        .collect();
    assert_eq!(outline, expected);
}

#[test]
fn replaces_the_outline_and_turns_pages() {
    let item = |level, title: &str, page| OutlineItem {
        level,
        title: title.to_string(),
        page,
        view: None,
    };
    // Items written without a view lead to the top of their page; that
    // view is checked in `keeps_outline_views`.
    let without_views = |items: Vec<OutlineItem>| -> Vec<OutlineItem> {
        let items = items.into_iter();
        items
            .map(|item| OutlineItem { view: None, ..item })
            .collect()
    };
    let mut document = Document::open(sample_file()).unwrap();
    assert_eq!(document.outline().unwrap(), []);
    // A level that skips ahead goes one deeper, and a page that does not
    // exist leads nowhere.
    let given = [
        item(1, "First", Some(2)),
        item(5, "Inside caf\u{e9}", Some(0)),
        item(3, "Deeper", None),
        item(2, "Second inside", Some(9)),
        item(0, "Last", Some(1)),
    ];
    document.set_outline(&given).unwrap();
    let expected = [
        item(1, "First", Some(2)),
        item(2, "Inside caf\u{e9}", Some(0)),
        item(3, "Deeper", None),
        item(2, "Second inside", None),
        item(1, "Last", Some(1)),
    ];
    assert_eq!(without_views(document.outline().unwrap()), expected);

    document.set_page_rotation(0, -90).unwrap();
    document.set_page_rotation(1, 540).unwrap();
    let structure = |reason| Err(DocumentError::Structure(reason));
    assert_eq!(
        document.set_page_rotation(0, 45),
        structure("a page rotation is a multiple of 90 degrees")
    );
    assert_eq!(
        document.set_page_rotation(3, 90),
        structure("a page index is out of range")
    );

    let saved = document.save().unwrap();
    // Two top-level items, and three items under the closed first one.
    assert!(contains(&saved, "/Count 2 "));
    assert!(contains(&saved, "/Count -3 "));
    let mut reopened = Document::open(saved).unwrap();
    assert_eq!(without_views(reopened.outline().unwrap()), expected);
    let rotations: Vec<u16> = reopened.pages().iter().map(|page| page.rotation).collect();
    assert_eq!(rotations, [270, 180, 270]);

    // Replacing the outline again leaves the old one out of the file.
    reopened.set_outline(&[item(1, "Only", Some(0))]).unwrap();
    let saved = reopened.save().unwrap();
    assert!(!contains(&saved, "Deeper"));
    let mut reopened = Document::open(saved).unwrap();
    assert_eq!(
        without_views(reopened.outline().unwrap()),
        [item(1, "Only", Some(0))]
    );
    reopened.set_outline(&[]).unwrap();
    assert_eq!(reopened.outline().unwrap(), []);
    assert!(!contains(&reopened.save().unwrap(), "Outlines"));
}

#[test]
fn decodes_streams_through_chained_filters() {
    let mut document = Document::open(described_file()).unwrap();
    let reference = |number| ObjectReference {
        number,
        generation: 0,
    };
    assert_eq!(
        document.stream(reference(17)).unwrap().as_deref(),
        Some(b"ABCDDD".as_slice())
    );
    assert_eq!(
        document.stream(reference(18)).unwrap().as_deref(),
        Some(b"plain".as_slice())
    );
    assert_eq!(document.stream(reference(1)).unwrap(), None);
    assert_eq!(document.stream(reference(400)).unwrap(), None);

    let mut builder = sample();
    builder.stream(7, "/Length 4 /Filter /JPXDecode", b"jpx!");
    builder.table(&[1, 2, 3, 4, 5, 6, 7], "/Size 8");
    let mut document = Document::open(builder.bytes).unwrap();
    assert_eq!(
        document.stream(reference(7)),
        Err(DocumentError::Unsupported("a stream filter or predictor"))
    );
}

#[test]
fn decodes_each_text_string_encoding() {
    assert_eq!(decode_text_string(b"plain"), "plain");
    assert_eq!(
        decode_text_string(&[0x80, 0xa0, 0xe9, 0x18]),
        "\u{2022}\u{20ac}\u{e9}\u{2d8}"
    );
    assert_eq!(
        decode_text_string(&[0x7f, 0x9f, 0xad]),
        "\u{fffd}\u{fffd}\u{fffd}"
    );
    assert_eq!(
        decode_text_string(&[0xfe, 0xff, 0xd8, 0x3d, 0xde, 0x00]),
        "\u{1f600}"
    );
    assert_eq!(
        decode_text_string(&[0xfe, 0xff, 0x00, 0x41, 0xd8]),
        "A\u{fffd}"
    );
    assert_eq!(
        decode_text_string(&[0xef, 0xbb, 0xbf, 0xc3, 0xa9]),
        "\u{e9}"
    );
    assert_eq!(decode_text_string(b""), "");
}

/// The sample document with links on its first page and two embedded files.
fn linked_file() -> Vec<u8> {
    let mut builder = sample();
    builder.object(
        3,
        "<< /Type /Page /Parent 2 0 R /Annots [7 0 R 8 0 R 9 0 R 10 0 R 11 0 R 12 0 R 13 0 R \
         14 0 R 15 0 R 99 0 R] >>",
    );
    builder.object(
        7,
        "<< /Subtype /Link /Rect [10 20 110 40] /Dest [6 0 R /Fit] >>",
    );
    builder.object(
        8,
        "<< /Subtype /Link /Rect [110 40 10 20] /A << /S /GoTo /D (second) >> >>",
    );
    builder.object(
        9,
        "<< /Subtype /Link /Rect [0 0 5 5] /A << /S /URI /URI (https://example.org/a?b=1) >> >>",
    );
    builder.object(
        10,
        "<< /Subtype /Link /Rect [0 0 5 5] /A << /S /GoToR /D [4 /Fit] \
         /F << /Type /Filespec /F (plain.pdf) /UF <FEFF00FC006E0069002E007000640066> >> >> >>",
    );
    builder.object(
        11,
        "<< /Subtype /Link /Rect [0 0 5 5] /A << /S /Launch /F (notes.txt) >> >>",
    );
    builder.object(
        12,
        "<< /Subtype /Link /Rect [0 0 5 5] /A << /S /Named /N /NextPage >> >>",
    );
    // Not listed: another annotation type, an unsupported action, a link
    // without a rectangle, and a missing object.
    builder.object(13, "<< /Subtype /Text /Rect [0 0 5 5] /Contents (note) >>");
    builder.object(
        14,
        "<< /Subtype /Link /Rect [0 0 5 5] /A << /S /JavaScript /JS (app.beep\\(\\)) >> >>",
    );
    builder.object(15, "<< /Subtype /Link /Dest [6 0 R /Fit] >>");
    builder.object(16, "<< /Names [(second) [5 0 R /XYZ 0 0 0]] >>");
    builder.object(17, "<< /Kids [18 0 R 19 0 R] >>");
    builder.object(
        18,
        "<< /Names [(data.csv) << /Type /Filespec /F (data.csv) /Desc (Figures) \
         /EF << /F 20 0 R >> >>] >>",
    );
    builder.object(
        19,
        "<< /Names [(key two) << /Type /Filespec /F (plain.bin) \
         /UF <FEFF00FC006E0069002E00620069006E> /EF << /UF 21 0 R /F 20 0 R >> >> \
         (broken) << /Type /Filespec >>] >>",
    );
    builder.stream(20, "/Type /EmbeddedFile /Length 7", b"a,b\n1,2");
    builder.stream(
        21,
        "/Type /EmbeddedFile /Length 9 /Filter /ASCIIHexDecode",
        b"00FF1020>",
    );
    builder.object(
        1,
        "<< /Type /Catalog /Pages 2 0 R /Names << /Dests 16 0 R /EmbeddedFiles 17 0 R >> >>",
    );
    let numbers: Vec<u32> = (1..=21).collect();
    builder.table(&numbers, "/Size 22");
    builder.bytes
}

#[test]
fn reads_page_links_and_their_targets() {
    let mut document = Document::open(linked_file()).unwrap();
    let links = document.links(0).unwrap();
    let targets: Vec<_> = links.iter().map(|link| link.target.clone()).collect();
    assert_eq!(
        targets,
        [
            LinkTarget::Page(2),
            LinkTarget::Page(1),
            LinkTarget::Uri("https://example.org/a?b=1".to_string()),
            LinkTarget::RemoteDocument {
                file: "\u{fc}ni.pdf".to_string(),
                page: Some(4),
            },
            LinkTarget::Launch("notes.txt".to_string()),
            LinkTarget::Named("NextPage".to_string()),
        ]
    );
    // Rectangles are put in order whichever corners the file gives.
    assert_eq!(links[0].rectangle, rectangle(10.0, 20.0, 110.0, 40.0));
    assert_eq!(links[1].rectangle, links[0].rectangle);
    let views: Vec<_> = links.iter().map(|link| link.view).collect();
    let corner = View::Xyz {
        left: Some(0.0),
        top: Some(0.0),
        zoom: None,
    };
    assert_eq!(
        views,
        [
            Some(View::Fit),
            Some(corner),
            None,
            Some(View::Fit),
            None,
            None
        ]
    );

    assert_eq!(document.links(1).unwrap(), []);
    assert_eq!(document.links(99).unwrap(), []);
}

#[test]
fn reads_every_kind_of_destination_view() {
    let xyz = |left, top, zoom| View::Xyz { left, top, zoom };
    let views = [
        (
            "[6 0 R /XYZ 10 20.5 1.5]",
            Some(xyz(Some(10.0), Some(20.5), Some(1.5))),
        ),
        // A null or missing value keeps the viewer's, as a zoom of 0 does.
        ("[6 0 R /XYZ null 700]", Some(xyz(None, Some(700.0), None))),
        ("[6 0 R /XYZ 1 2 0]", Some(xyz(Some(1.0), Some(2.0), None))),
        ("[6 0 R /Fit]", Some(View::Fit)),
        ("[6 0 R /FitH 300]", Some(View::FitH { top: Some(300.0) })),
        ("[6 0 R /FitV null]", Some(View::FitV { left: None })),
        (
            "[6 0 R /FitR 100 200 50 20]",
            Some(View::FitR(rectangle(50.0, 20.0, 100.0, 200.0))),
        ),
        ("[6 0 R /FitB]", Some(View::FitB)),
        ("[6 0 R /FitBH 5]", Some(View::FitBH { top: Some(5.0) })),
        ("[6 0 R /FitBV 7]", Some(View::FitBV { left: Some(7.0) })),
        // The page is still found when the view cannot be read.
        ("[6 0 R /FitR 1 2 3]", None),
        ("[6 0 R /Zoomed 2]", None),
        ("[6 0 R]", None),
    ];
    let mut builder = sample();
    let first = 7;
    let numbers: Vec<u32> = (first..).take(views.len()).collect();
    let annotations: Vec<String> = numbers.iter().map(|n| format!("{n} 0 R")).collect();
    builder.object(
        3,
        &format!(
            "<< /Type /Page /Parent 2 0 R /Annots [{}] >>",
            annotations.join(" ")
        ),
    );
    for (&number, (destination, _)) in numbers.iter().zip(&views) {
        let link = format!("<< /Subtype /Link /Rect [0 0 5 5] /Dest {destination} >>");
        builder.object(number, &link);
    }
    let mut all = vec![1, 2, 3, 4, 5, 6];
    all.extend(&numbers);
    builder.table(&all, &format!("/Size {}", first as usize + views.len()));
    let mut document = Document::open(builder.bytes).unwrap();
    let links = document.links(0).unwrap();
    assert!(links.iter().all(|link| link.target == LinkTarget::Page(2)));
    let read: Vec<_> = links.iter().map(|link| link.view).collect();
    let expected: Vec<_> = views.iter().map(|(_, view)| *view).collect();
    assert_eq!(read, expected);

    let fit_area = View::FitR(rectangle(50.0, 20.0, 100.0, 200.0));
    assert_eq!(fit_area.point(), (Some(50.0), Some(200.0)));
    assert_eq!(fit_area.zoom(), None);
    assert_eq!(fit_area.name(), "FitR");
    assert_eq!(View::FitH { top: Some(3.0) }.point(), (None, Some(3.0)));
    assert_eq!(xyz(None, Some(1.0), Some(2.0)).zoom(), Some(2.0));
}

#[test]
fn resolves_named_destinations() {
    let mut document = Document::open(linked_file()).unwrap();
    let corner = View::Xyz {
        left: Some(0.0),
        top: Some(0.0),
        zoom: None,
    };
    assert_eq!(
        document.named_destination(b"second").unwrap(),
        Some((1, Some(corner)))
    );
    assert_eq!(document.named_destination(b"missing").unwrap(), None);
    // Names in the name tree come first; the older `/Dests` dictionary is
    // read when the tree has no such name.
    let mut described = Document::open(described_file()).unwrap();
    assert_eq!(
        described.named_destination(b"legacy").unwrap(),
        Some((2, Some(View::Fit)))
    );
    assert_eq!(
        described.named_destination(b"gamma").unwrap(),
        Some((0, Some(View::Fit)))
    );
}

#[test]
fn keeps_outline_views() {
    let mut document = Document::open(sample_file()).unwrap();
    let views = [
        Some(View::Xyz {
            left: Some(10.0),
            top: Some(500.0),
            zoom: Some(2.0),
        }),
        Some(View::FitR(rectangle(1.0, 2.0, 3.0, 4.0))),
        Some(View::FitBH { top: None }),
        None,
    ];
    let items: Vec<OutlineItem> = views
        .iter()
        .enumerate()
        .map(|(index, &view)| OutlineItem {
            level: 1,
            title: format!("Item {index}"),
            page: Some(index % 3),
            view,
        })
        .collect();
    document.set_outline(&items).unwrap();
    let mut reopened = Document::open(document.save().unwrap()).unwrap();
    let read: Vec<_> = reopened
        .outline()
        .unwrap()
        .into_iter()
        .map(|item| item.view)
        .collect();
    assert_eq!(read[..3], views[..3]);
    // Without a view, an item leads to the top of its page.
    let top = reopened.pages()[0].crop_box.top;
    let expected = View::Xyz {
        left: None,
        top: Some(top),
        zoom: None,
    };
    assert_eq!(read[3], Some(expected));

    // Keeping some pages keeps the views of the items that stay.
    reopened.select_pages(&[1, 0]).unwrap();
    let kept: Vec<_> = reopened.outline().unwrap();
    let kept: Vec<_> = kept
        .into_iter()
        .map(|item| (item.page, item.view))
        .collect();
    assert_eq!(
        kept,
        [
            (Some(1), views[0]),
            (Some(0), views[1]),
            (Some(1), Some(expected)),
        ]
    );
}

#[test]
fn lists_embedded_files_and_reads_their_data() {
    let mut document = Document::open(linked_file()).unwrap();
    let attachments = document.attachments().unwrap();
    let summary: Vec<_> = attachments
        .iter()
        .map(|file| {
            (
                file.name.as_str(),
                file.file_name.as_deref(),
                file.description.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("data.csv", Some("data.csv"), Some("Figures")),
            ("key two", Some("\u{fc}ni.bin"), None),
            ("broken", None, None),
        ]
    );
    let data: Vec<_> = attachments
        .iter()
        .map(|file| {
            file.stream
                .and_then(|stream| document.stream(stream).unwrap())
        })
        .collect();
    assert_eq!(
        data,
        [
            Some(b"a,b\n1,2".to_vec()),
            Some(vec![0x00, 0xff, 0x10, 0x20]),
            None
        ]
    );

    let mut plain = Document::open(sample_file()).unwrap();
    assert_eq!(plain.attachments().unwrap(), []);
    assert_eq!(plain.links(0).unwrap(), []);
}

#[test]
fn reads_page_content_from_one_stream_or_several() {
    let mut builder = sample();
    builder.object(3, "<< /Type /Page /Parent 2 0 R /Contents 7 0 R >>");
    builder.object(
        5,
        "<< /Type /Page /Parent 4 0 R /Contents [8 0 R 9 0 R 99 0 R] >>",
    );
    builder.stream(7, "/Length 14", b"BT (one) Tj ET");
    builder.stream(8, "/Length 1", b"q");
    builder.stream(9, "/Length 3 /Filter /ASCIIHexDecode", b"51>");
    builder.table(&[1, 2, 3, 4, 5, 6, 7, 8, 9], "/Size 10");
    let mut document = Document::open(builder.bytes).unwrap();

    assert_eq!(document.page_content(0).unwrap(), b"BT (one) Tj ET");
    assert_eq!(document.page_content(1).unwrap(), b"q\nQ\n");
    assert_eq!(document.page_content(2).unwrap(), b"");
    assert_eq!(document.page_content(50).unwrap(), b"");

    let operations =
        plain_open_pdf_engine::parse_content(&document.page_content(1).unwrap()).unwrap();
    let operators: Vec<_> = operations.iter().map(|o| o.operator.clone()).collect();
    assert_eq!(operators, [b"q".to_vec(), b"Q".to_vec()]);
}

#[test]
fn extracts_page_text_through_font_maps_and_line_moves() {
    let mut builder = sample();
    builder.object(
        2,
        "<< /Type /Pages /Count 3 /Kids [3 0 R 4 0 R] /MediaBox [0 0 612 792] \
         /Resources << /Font << /F1 7 0 R /F2 8 0 R >> >> >>",
    );
    builder.object(3, "<< /Type /Page /Parent 2 0 R /Contents 10 0 R >>");
    builder.object(7, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
    builder.object(
        8,
        "<< /Type /Font /Subtype /Type0 /BaseFont /Mapped /ToUnicode 9 0 R >>",
    );
    let map = b"1 begincodespacerange <0000> <FFFF> endcodespacerange\n\
                2 beginbfchar <0001> <0048> <0002> <00E9> endbfchar";
    builder.stream(9, &format!("/Length {}", map.len()), map);
    let content = b"BT /F1 12 Tf 1 0 0 1 72 700 Tm (Hello ) Tj [(Wor) -20 (ld)] TJ\n\
                    0 -14 Td (second line) Tj T* (third) Tj (fourth) '\n\
                    /F2 12 Tf 1 0 0 1 72 600 Tm <00010002> Tj 1 0 0 1 90 600 Tm <0001> Tj ET";
    builder.stream(10, &format!("/Length {}", content.len()), content);
    builder.table(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], "/Size 11");
    let mut document = Document::open(builder.bytes).unwrap();

    assert_eq!(
        document.page_text(0).unwrap(),
        "Hello World\nsecond line\nthird\nfourth\nH\u{e9}H\n"
    );
    // Pages without content, and indexes past the end, have no text.
    assert_eq!(document.page_text(1).unwrap(), "");
    assert_eq!(document.page_text(50).unwrap(), "");
}

#[test]
fn extracts_page_text_through_font_encodings() {
    let mut builder = sample();
    builder.object(
        2,
        "<< /Type /Pages /Count 3 /Kids [3 0 R 4 0 R] /MediaBox [0 0 612 792] \
         /Resources << /Font << /F1 7 0 R /F2 8 0 R /F3 9 0 R >> >> >>",
    );
    builder.object(3, "<< /Type /Page /Parent 2 0 R /Contents 12 0 R >>");
    builder.object(
        7,
        "<< /Type /Font /Subtype /TrueType /BaseFont /Arial /Encoding /WinAnsiEncoding \
         /ToUnicode 13 0 R >>",
    );
    // A simple font's codes are single bytes whatever codespace its map
    // declares, and codes the map lacks are read through the encoding.
    let map = b"1 begincodespacerange <0000> <FFFF> endcodespacerange
                1 beginbfchar <78> <00660069> endbfchar";
    builder.stream(13, &format!("/Length {}", map.len()), map);
    builder.object(
        8,
        "<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+Custom /Encoding 10 0 R >>",
    );
    builder.object(
        9,
        "<< /Type /Font /Subtype /Type0 /BaseFont /Unmapped /Encoding /Identity-H >>",
    );
    builder.object(
        10,
        "<< /Type /Encoding /BaseEncoding /MacRomanEncoding /Differences 11 0 R >>",
    );
    builder.object(11, "[1 /Omega /f_f 65 /uni2603]");
    let content = b"BT /F1 12 Tf <636166E920937894> Tj T* /F2 12 Tf <410102428E> Tj T*
                    /F3 12 Tf <000100020003> Tj T* /F9 12 Tf (it's) Tj ET (unset) Tj";
    builder.stream(12, &format!("/Length {}", content.len()), content);
    builder.table(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13], "/Size 14");
    let mut document = Document::open(builder.bytes).unwrap();

    assert_eq!(
        document.page_text(0).unwrap(),
        "caf\u{e9} \u{201c}fi\u{201d}\n\u{2603}\u{3a9}ffB\u{e9}\n\u{fffd}\u{fffd}\u{fffd}\n\
         it\u{2019}sunset\n"
    );
}

#[test]
fn places_glyphs_and_assembles_text_from_their_positions() {
    let mut builder = sample();
    builder.object(
        2,
        "<< /Type /Pages /Count 3 /Kids [3 0 R 4 0 R] /MediaBox [0 0 612 792] \
         /Resources << /Font << /F1 7 0 R >> /XObject << /Fm 8 0 R /Im 10 0 R >> >> >>",
    );
    builder.object(3, "<< /Type /Page /Parent 2 0 R /Contents 12 0 R >>");
    builder.object(
        7,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Custom /FirstChar 65 /Widths [600 700] >>",
    );
    // A form with its own font, moved right by its matrix.
    let form = b"BT /F2 10 Tf 0 700 Td (Hi) Tj ET /Fm Do";
    builder.stream(
        8,
        &format!(
            "/Type /XObject /Subtype /Form /Matrix [1 0 0 1 300 0] /Length {} \
             /Resources << /Font << /F2 9 0 R >> /XObject << /Fm 8 0 R >> >>",
            form.len()
        ),
        form,
    );
    builder.object(9, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
    builder.stream(10, "/Type /XObject /Subtype /Image /Length 3", b"(x)");
    let content = b"q 2 0 0 2 10 20 cm BT /F1 10 Tf 5 100 Td (AB) Tj [(A) -1000 (B)] TJ ET Q
                    BT /F1 10 Tf 100 50 Td (A) Tj 30 0 Td (B) Tj 0 -12 Td (A) Tj ET
                    /Im Do /Fm Do /Missing Do";
    builder.stream(12, &format!("/Length {}", content.len()), content);
    builder.table(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12], "/Size 13");
    let mut document = Document::open(builder.bytes).unwrap();

    // Leaving the baseline ends the line, and so does a gap along it wider
    // than 0.8 of the font size, as between columns: each gap here is an em
    // or more. The form draws itself again, which stops at the nesting
    // limit.
    assert_eq!(
        document.page_text(0).unwrap(),
        format!(
            "ABA{0}B{0}A{0}B{0}A{0}{1}",
            char::from(10),
            "Hi\n".repeat(8)
        )
    );

    let glyphs = document.page_glyphs(0).unwrap();
    let place = |index: usize| {
        let glyph: &plain_open_pdf_engine::TextGlyph = &glyphs[index];
        (
            glyph.text.as_str(),
            glyph.x,
            glyph.y,
            glyph.end_x,
            glyph.size,
        )
    };
    // Widths come from the font, scaled by its size and the matrix.
    assert_eq!(place(0), ("A", 20.0, 220.0, 32.0, 20.0));
    assert_eq!(place(1), ("B", 32.0, 220.0, 46.0, 20.0));
    assert_eq!(place(2), ("A", 46.0, 220.0, 58.0, 20.0));
    assert_eq!(place(3), ("B", 78.0, 220.0, 92.0, 20.0));
    assert_eq!(place(6), ("A", 130.0, 38.0, 136.0, 10.0));
    // Helvetica gives no widths; Adobe's metrics make its H 722
    // thousandths of the size wide.
    assert_eq!(place(7), ("H", 300.0, 700.0, 307.22, 10.0));
    assert_eq!(place(9), ("H", 600.0, 700.0, 607.22, 10.0));
    assert_eq!(document.page_glyphs(50).unwrap(), []);

    // A search ignores case and gives one rectangle for each line of a
    // match, a quarter of the size below the baseline to 0.85 above it; a
    // wide gap parts a line, so these matches have two parts.
    let hits = document.search_page(0, "a B").unwrap();
    assert_eq!(
        hits,
        [
            [
                rectangle(46.0, 215.0, 58.0, 237.0),
                rectangle(78.0, 215.0, 92.0, 237.0)
            ],
            [
                rectangle(100.0, 47.5, 106.0, 58.5),
                rectangle(130.0, 47.5, 137.0, 58.5)
            ]
        ]
    );
    // A line end matches a space, and the match has a part on each line.
    let hits = document.search_page(0, "b a").unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(
        hits[0],
        [
            rectangle(78.0, 215.0, 92.0, 237.0),
            rectangle(100.0, 47.5, 106.0, 58.5)
        ]
    );
    assert_eq!(document.search_page(0, "hi").unwrap().len(), 8);
    for nothing in ["", " ", "absent", "ab a b a b a hi hi hi hi hi hi hi hi hi"] {
        assert!(document.search_page(0, nothing).unwrap().is_empty());
    }
    assert!(document.search_page(50, "a").unwrap().is_empty());

    // Words carry their box and their block, line, and word numbers.
    let words = document.page_words(0).unwrap();
    let word = |index: usize| {
        let word: &plain_open_pdf_engine::TextWord = &words[index];
        (
            word.text.as_str(),
            word.bounds,
            word.block,
            word.line,
            word.word,
        )
    };
    assert_eq!(words.len(), 13);
    assert_eq!(
        word(0),
        ("ABA", rectangle(20.0, 215.0, 58.0, 237.0), 0, 0, 0)
    );
    assert_eq!(word(1), ("B", rectangle(78.0, 215.0, 92.0, 237.0), 0, 0, 1));
    // Far below starts a block; the next line just below does not.
    assert_eq!(word(2), ("A", rectangle(100.0, 47.5, 106.0, 58.5), 1, 0, 0));
    assert_eq!(word(3), ("B", rectangle(130.0, 47.5, 137.0, 58.5), 1, 0, 1));
    assert_eq!(word(4), ("A", rectangle(130.0, 35.5, 136.0, 46.5), 1, 1, 0));
    assert_eq!(
        word(5),
        ("Hi", rectangle(300.0, 697.5, 309.44, 708.5), 2, 0, 0)
    );
    assert_eq!(
        (word(12).0, word(12).2, word(12).3, word(12).4),
        ("Hi", 2, 0, 7)
    );

    // Spans join the words of a line that share a style, unless they are
    // far apart, as every gap here is.
    let spans = document.page_spans(0).unwrap();
    let texts: Vec<&str> = spans.iter().map(|span| span.text.as_str()).collect();
    assert_eq!(texts[..6], ["ABA", "B", "A", "B", "A", "Hi"]);
    assert_eq!(spans.len(), 13);
    let first = &spans[0];
    assert_eq!((first.x, first.y, first.size), (20.0, 220.0, 20.0));
    assert_eq!(first.bounds, rectangle(20.0, 215.0, 58.0, 237.0));
    assert_eq!((&*first.font, first.color), ("Custom", [0, 0, 0]));
    assert_eq!(&*spans[12].font, "Helvetica");
    assert!(document.page_words(50).unwrap().is_empty());
    assert!(document.page_spans(50).unwrap().is_empty());
}

#[test]
fn spans_change_with_the_fill_color() {
    let mut builder = sample();
    builder.object(
        3,
        "<< /Type /Page /Parent 2 0 R /Contents 8 0 R \
         /Resources << /Font << /F1 7 0 R >> >> >>",
    );
    builder.object(
        7,
        "<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+Helvetica >>",
    );
    let content = b"BT /F1 10 Tf 10 50 Td 0.5 g (a) Tj 0 0 1 rg (b) Tj (b) Tj \
                    0 1 1 0 k (c) Tj /Pattern cs /P1 scn (d) Tj ET";
    builder.stream(8, &format!("/Length {}", content.len()), content);
    builder.table(&[1, 2, 3, 4, 5, 6, 7, 8], "/Size 9");
    let mut document = Document::open(builder.bytes).unwrap();
    let spans: Vec<_> = document
        .page_spans(0)
        .unwrap()
        .into_iter()
        .map(|span| (span.text, span.color, span.font.to_string()))
        .collect();
    let helvetica = || "Helvetica".to_string();
    // Gray, RGB, and CMYK; a pattern leaves the color black.
    assert_eq!(
        spans,
        [
            ("a".to_string(), [128, 128, 128], helvetica()),
            ("bb".to_string(), [0, 0, 255], helvetica()),
            ("c".to_string(), [255, 0, 0], helvetica()),
            ("d".to_string(), [0, 0, 0], helvetica())
        ]
    );
}

#[test]
fn maps_page_points_to_the_displayed_page() {
    let document = Document::open(sample_file()).unwrap();
    // 612 by 792 points, turned a quarter clockwise: the bottom left of
    // the page is shown at the top left.
    let turned = document.pages()[0];
    assert_eq!(turned.view_size(), (792.0, 612.0));
    assert_eq!(turned.to_view(0.0, 0.0), (0.0, 0.0));
    assert_eq!(turned.to_view(612.0, 792.0), (792.0, 612.0));
    assert_eq!(turned.to_view(0.0, 792.0), (792.0, 0.0));
    // A crop box sets the origin, and no rotation only flips the page.
    let cropped = document.pages()[1];
    assert_eq!(cropped.rotation, 0);
    let (left, top) = (cropped.crop_box.left, cropped.crop_box.top);
    assert_eq!(cropped.to_view(left, top), (0.0, 0.0));
    assert_eq!(cropped.to_view(left + 5.0, top - 7.0), (5.0, 7.0));
    for page in document.pages() {
        for rotation in [0, 90, 180, 270] {
            let page = plain_open_pdf_engine::Page { rotation, ..*page };
            let (x, y) = page.to_view(123.0, 45.0);
            let (back_x, back_y) = page.from_view(x, y);
            assert!((back_x - 123.0).abs() < 1e-9 && (back_y - 45.0).abs() < 1e-9);
            let size = page.view_size();
            let corner = page.to_view(page.crop_box.right, page.crop_box.bottom);
            assert!(corner.0 >= -1e-9 && corner.0 <= size.0 + 1e-9, "{rotation}");
            assert!(corner.1 >= -1e-9 && corner.1 <= size.1 + 1e-9, "{rotation}");
        }
    }
}

fn contains(bytes: &[u8], part: &str) -> bool {
    bytes
        .windows(part.len())
        .any(|window| window == part.as_bytes())
}

#[test]
fn saves_a_document_that_opens_the_same() {
    // An object nothing refers to is left out of the saved file.
    let mut builder = sample();
    builder.object(9, "(orphan)");
    builder.table(&[1, 2, 3, 4, 5, 6, 9], "/Size 10");
    let mut document = Document::open(builder.bytes).unwrap();
    let saved = document.save().unwrap();
    assert!(saved.starts_with(b"%PDF-1.7\n"));
    assert!(saved.ends_with(b"%%EOF\n"));
    assert!(!contains(&saved, "orphan"));
    assert_sample_pages(&Document::open(saved).unwrap());

    // Objects held in object streams are written out one by one, and
    // stream data survives with its filters.
    let mut document = Document::open(described_file()).unwrap();
    let information = document.information().unwrap();
    let outline = document.outline().unwrap();
    let saved = document.save().unwrap();
    let mut reopened = Document::open(saved).unwrap();
    assert_eq!(reopened.information().unwrap(), information);
    assert_eq!(reopened.outline().unwrap(), outline);

    let mut document = Document::open(compressed_file("LENGTH")).unwrap();
    let saved = document.save().unwrap();
    assert!(!contains(&saved, "ObjStm"));
    assert_sample_pages(&Document::open(saved).unwrap());
}

#[test]
fn changes_are_read_back_and_saved() {
    let mut document = Document::open(sample_file()).unwrap();
    assert_eq!(document.information().unwrap(), Information::default());
    document
        .set_information("Title", Some("Caf\u{e9} (1)"))
        .unwrap();
    document.set_information("Author", Some("Someone")).unwrap();
    document.set_information("Author", None).unwrap();

    // A new content stream for the first page.
    let stream = document.add_object(PdfObject::Null);
    document.set_stream(stream, Default::default(), b"BT (new) Tj ET".to_vec());
    let page = document.pages()[0].reference.unwrap();
    let PdfObject::Dictionary(mut entries) = document.object(page).unwrap() else {
        panic!("a page is a dictionary");
    };
    entries.insert(b"Contents".to_vec(), PdfObject::Reference(stream));
    document.set_object(page, PdfObject::Dictionary(entries));
    assert_eq!(document.page_content(0).unwrap(), b"BT (new) Tj ET");

    let saved = document.save().unwrap();
    assert!(contains(&saved, "/Length 14"));
    let mut reopened = Document::open(saved).unwrap();
    assert_sample_pages(&reopened);
    assert_eq!(reopened.page_content(0).unwrap(), b"BT (new) Tj ET");
    let information = reopened.information().unwrap();
    assert_eq!(information.title.as_deref(), Some("Caf\u{e9} (1)"));
    assert_eq!(information.author, None);
}

#[test]
fn compresses_stream_data_it_is_given() {
    let mut document = Document::open(sample_file()).unwrap();
    let content = b"0 0 10 10 re f ".repeat(200);
    let stream = document.add_object(PdfObject::Null);
    let mut entries = plain_open_pdf_engine::Dictionary::new();
    entries.insert(
        b"Filter".to_vec(),
        PdfObject::Name(b"ASCIIHexDecode".to_vec()),
    );
    document.set_stream_data(stream, entries, &content);
    assert_eq!(
        document.stream(stream).unwrap().as_deref(),
        Some(&content[..])
    );

    let page = document.pages()[0].reference.unwrap();
    let PdfObject::Dictionary(mut entries) = document.object(page).unwrap() else {
        panic!("a page is a dictionary");
    };
    entries.insert(b"Contents".to_vec(), PdfObject::Reference(stream));
    document.set_object(page, PdfObject::Dictionary(entries));
    let saved = document.save().unwrap();
    assert!(contains(&saved, "/Filter /FlateDecode"));
    assert!(saved.len() < content.len());
    let mut reopened = Document::open(saved).unwrap();
    assert_eq!(reopened.page_content(0).unwrap(), content);
}

#[test]
fn removing_pages_removes_what_leads_to_them() {
    let mut builder = sample();
    builder.object(
        1,
        "<< /Type /Catalog /Pages 2 0 R /Outlines 9 0 R /Names << /Dests 13 0 R >> \
         /OpenAction [5 0 R /Fit] /StructTreeRoot 15 0 R >>",
    );
    builder.object(
        3,
        "<< /Type /Page /Parent 2 0 R /Annots [7 0 R 8 0 R 16 0 R] >>",
    );
    builder.object(
        7,
        "<< /Subtype /Link /Rect [0 0 10 10] /Dest [5 0 R /Fit] >>",
    );
    builder.object(
        8,
        "<< /Subtype /Link /Rect [0 0 10 10] /A << /S /GoTo /D [6 0 R /Fit] >> >>",
    );
    builder.object(
        16,
        "<< /Subtype /Link /Rect [0 0 10 10] /A << /S /GoTo /D [5 0 R /Fit] >> >>",
    );
    builder.object(
        9,
        "<< /Type /Outlines /First 10 0 R /Last 11 0 R /Count 2 >>",
    );
    builder.object(
        10,
        "<< /Title (Gone) /Parent 9 0 R /Next 11 0 R /Dest [5 0 R /Fit] \
         /First 12 0 R /Last 12 0 R /Count 1 >>",
    );
    builder.object(
        11,
        "<< /Title (Removed leaf) /Parent 9 0 R /Prev 10 0 R /Dest [5 0 R /Fit] >>",
    );
    builder.object(
        12,
        "<< /Title (Kept child) /Parent 10 0 R /Dest [6 0 R /Fit] >>",
    );
    builder.object(13, "<< /Kids [14 0 R] >>");
    builder.object(
        14,
        "<< /Names [(to-removed) [5 0 R /Fit] (to-kept) [3 0 R /Fit]] >>",
    );
    builder.object(15, "<< /Type /StructTreeRoot /K 5 0 R >>");
    builder.table(
        &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
        "/Size 17",
    );
    let mut document = Document::open(builder.bytes).unwrap();
    assert_eq!(document.links(0).unwrap().len(), 3);

    // The second page, object 5, goes, and the third becomes the second.
    document.select_pages(&[0, 2]).unwrap();
    let saved = document.save().unwrap();
    for gone in [
        "\n5 0 obj",
        "to-removed",
        "Removed leaf",
        "OpenAction",
        "StructTreeRoot",
    ] {
        assert!(!contains(&saved, gone), "{gone}");
    }
    assert!(contains(&saved, "to-kept"));
    let mut reopened = Document::open(saved).unwrap();
    assert_eq!(reopened.page_count(), 2);
    let targets: Vec<LinkTarget> = reopened
        .links(0)
        .unwrap()
        .into_iter()
        .map(|link| link.target)
        .collect();
    assert_eq!(targets, [LinkTarget::Page(1)]);
    // An item for a removed page stays only as the parent of a kept one.
    let outline: Vec<_> = reopened
        .outline()
        .unwrap()
        .into_iter()
        .map(|item| (item.level, item.title, item.page))
        .collect();
    assert_eq!(
        outline,
        [
            (1, "Gone".to_string(), None),
            (2, "Kept child".to_string(), Some(1))
        ]
    );
}

#[test]
fn breaks_a_page_in_two_and_inserts_blank_pages() {
    let mut builder = sample();
    builder.object(
        3,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Rotate 0 /Contents 8 0 R \
         /Resources << /Font << /F1 7 0 R >> >> /Annots [9 0 R 10 0 R] >>",
    );
    builder.object(7, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
    let content =
        b"BT /F1 10 Tf 20 250 Td (Top) Tj 0 -150 Td (Bottom) Tj ET 1 0 0 rg 10 80 40 40 re f";
    builder.stream(8, &format!("/Length {}", content.len()), content);
    builder.object(
        9,
        "<< /Subtype /Link /Rect [0 240 50 260] /A << /S /URI /URI (high) >> >>",
    );
    builder.object(
        10,
        "<< /Subtype /Link /Rect [0 90 50 110] /P 3 0 R /A << /S /URI /URI (low) >> >>",
    );
    builder.table(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], "/Size 11");
    let mut document = Document::open(builder.bytes).unwrap();
    assert_eq!(document.page_text(0).unwrap(), "Top\nBottom\n");

    let structure = |reason| Err(DocumentError::Structure(reason));
    for outside in [0.0, 300.0, -5.0, f64::NAN] {
        assert_eq!(
            document.break_page(0, outside),
            structure("the break is not inside the page")
        );
    }
    assert_eq!(
        document.break_page(9, 150.0),
        structure("a page index is out of range")
    );

    // What lies below 150 points moves to the top of a new second page.
    document.break_page(0, 150.0).unwrap();
    let saved = document.save().unwrap();
    for mut document in [document, Document::open(saved).unwrap()] {
        assert_eq!(document.page_count(), 4);
        assert_eq!(document.pages()[1].media_box, document.pages()[0].media_box);
        assert_eq!(document.page_text(0).unwrap(), "Top\n");
        assert_eq!(document.page_text(1).unwrap(), "Bottom\n");
        let glyph = &document.page_glyphs(1).unwrap()[0];
        assert_eq!((glyph.x, glyph.y), (20.0, 250.0));
        assert_eq!(
            document.search_page(0, "bottom").unwrap(),
            [] as [Vec<Rectangle>; 0]
        );
        assert_eq!(document.search_page(1, "bottom").unwrap().len(), 1);

        // The red square was 80 to 120 points up; it is now 230 to 270.
        let upper = document.render_page(0, 1.0).unwrap();
        let lower = document.render_page(1, 1.0).unwrap();
        assert_eq!(upper.pixel(30, 200), Some([255, 255, 255, 255]));
        assert_eq!(lower.pixel(30, 50), Some([255, 0, 0, 255]));
        assert_eq!(lower.pixel(30, 200), Some([255, 255, 255, 255]));

        // Each link stays with its part, and the lower one moves up.
        let links = |document: &mut Document, page| -> Vec<(LinkTarget, Rectangle)> {
            let links = document.links(page).unwrap();
            links
                .into_iter()
                .map(|link| (link.target, link.rectangle))
                .collect()
        };
        let uri = |text: &str| LinkTarget::Uri(text.to_string());
        assert_eq!(
            links(&mut document, 0),
            [(uri("high"), rectangle(0.0, 240.0, 50.0, 260.0))]
        );
        assert_eq!(
            links(&mut document, 1),
            [(uri("low"), rectangle(0.0, 240.0, 50.0, 260.0))]
        );
        // The other pages are as they were.
        assert_eq!(
            document.pages()[2].media_box,
            rectangle(0.0, 0.0, 200.0, 100.0)
        );
    }
}

#[test]
fn inserts_blank_pages() {
    let mut document = Document::open(sample_file()).unwrap();
    let before = document.pages().to_vec();
    document.insert_blank_page(0, 100.0, 50.0).unwrap();
    document.insert_blank_page(4, 20.5, 30.0).unwrap();
    document.insert_blank_page(2, 7.0, 9.0).unwrap();
    let structure = |reason| Err(DocumentError::Structure(reason));
    assert_eq!(
        document.insert_blank_page(7, 10.0, 10.0),
        structure("a page index is out of range")
    );
    for size in [0.0, -1.0, f64::INFINITY] {
        assert_eq!(
            document.insert_blank_page(0, size, 10.0),
            structure("a page size is positive")
        );
    }
    let saved = document.save().unwrap();
    for mut document in [document, Document::open(saved).unwrap()] {
        let sizes: Vec<(f64, f64)> = document
            .pages()
            .iter()
            .map(|page| (page.media_box.width(), page.media_box.height()))
            .collect();
        assert_eq!(
            sizes,
            [
                (100.0, 50.0),
                (612.0, 792.0),
                (7.0, 9.0),
                (200.0, 100.0),
                (199.5, 200.0),
                (20.5, 30.0)
            ]
        );
        // The pages that were there keep their rotation; new ones have none.
        let rotations: Vec<u16> = document.pages().iter().map(|page| page.rotation).collect();
        assert_eq!(
            rotations,
            [
                0,
                before[0].rotation,
                0,
                before[1].rotation,
                before[2].rotation,
                0
            ]
        );
        assert_eq!(document.page_text(0).unwrap(), "");
        let image = document.render_page(0, 1.0).unwrap();
        assert_eq!((image.width(), image.height()), (100, 50));
    }
}

#[test]
fn selects_reorders_and_repeats_pages() {
    let mut document = Document::open(sample_file()).unwrap();
    let before = document.pages().to_vec();
    let geometry =
        |page: &plain_open_pdf_engine::Page| (page.media_box, page.crop_box, page.rotation);

    let structure = |reason| Err(DocumentError::Structure(reason));
    assert_eq!(
        document.select_pages(&[]),
        structure("a document needs at least one page")
    );
    assert_eq!(
        document.select_pages(&[0, 3]),
        structure("a page index is out of range")
    );
    assert_eq!(document.pages(), before);

    // Each page keeps the attributes it inherited from the old tree.
    document.select_pages(&[2, 0, 0]).unwrap();
    let saved = document.save().unwrap();
    for document in [document, Document::open(saved.clone()).unwrap()] {
        let pages = document.pages();
        assert_eq!(pages.len(), 3);
        assert_eq!(geometry(&pages[0]), geometry(&before[2]));
        assert_eq!(geometry(&pages[1]), geometry(&before[0]));
        assert_eq!(geometry(&pages[2]), geometry(&before[0]));
        assert_eq!(pages[0].reference, before[2].reference);
        assert_ne!(pages[1].reference, pages[2].reference);
    }
    // The page left out and the old intermediate node are gone.
    assert!(!contains(&saved, "\n5 0 obj"));
    assert!(!contains(&saved, "\n4 0 obj"));
}

#[test]
fn a_page_fills_the_last_column_and_row_of_its_image() {
    // Neither side is a whole number of pixels at either scale.
    for rotation in [0, 90, 180, 270] {
        let mut builder = sample();
        builder.object(
            3,
            &format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100.3 50.6] \
                 /Rotate {rotation} /Contents 7 0 R >>"
            ),
        );
        let content = b"0 g 0 0 100.3 50.6 re f";
        builder.stream(7, &format!("/Length {}", content.len()), content);
        builder.table(&[1, 2, 3, 4, 5, 6, 7], "/Size 8");
        let mut document = Document::open(builder.bytes).unwrap();
        for scale in [1.0, 1.37] {
            let image = document.render_page(0, scale).unwrap();
            let (long, short) = ((100.3 * scale).ceil(), (50.6 * scale).ceil());
            let (width, height) = match rotation {
                90 | 270 => (short as usize, long as usize),
                _ => (long as usize, short as usize),
            };
            assert_eq!((image.width(), image.height()), (width, height));
            // The fill covers the whole page, so no pixel is left pale.
            let palest = image.data().chunks(4).map(|pixel| pixel[0]).max();
            assert!(palest <= Some(2), "{rotation} {scale} {palest:?}");
        }
    }
}

/// A 100 by 50 point page that fills, strokes, clips, and draws a form.
fn drawn_file(rotation: u16) -> Vec<u8> {
    let mut builder = sample();
    builder.object(
        3,
        &format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 50] /Rotate {rotation} \
             /Contents 7 0 R /Resources << /XObject << /Fm 8 0 R >> \
             /ExtGState << /Half << /ca 0.5 >> >> >> >>"
        ),
    );
    let content = b"1 0 0 rg 10 10 30 20 re f
                    0 0 1 RG 4 w 60 5 m 60 45 l S
                    q 70 0 20 50 re W n 0 g 0 0 100 25 re f Q
                    q /Half gs 0 1 0 rg 90 40 10 10 re f Q
                    /Fm Do BT /F1 12 Tf (not drawn) Tj ET";
    builder.stream(7, &format!("/Length {}", content.len()), content);
    // The form paints well past its bounding box, which clips it.
    let form = b"0 g 0 0 50 50 re f";
    builder.stream(
        8,
        &format!(
            "/Subtype /Form /BBox [0 0 5 5] /Matrix [1 0 0 1 0 45] /Length {}",
            form.len()
        ),
        form,
    );
    builder.table(&[1, 2, 3, 4, 5, 6, 7, 8], "/Size 9");
    builder.bytes
}

#[test]
fn renders_filled_stroked_and_clipped_paths() {
    const WHITE: Option<[u8; 4]> = Some([255, 255, 255, 255]);
    const RED: Option<[u8; 4]> = Some([255, 0, 0, 255]);
    const BLACK: Option<[u8; 4]> = Some([0, 0, 0, 255]);
    let mut document = Document::open(drawn_file(0)).unwrap();
    let image = document.render_page(0, 1.0).unwrap();
    assert_eq!((image.width(), image.height()), (100, 50));
    assert_eq!(image.data().len(), 100 * 50 * 4);
    // The page's origin is at the bottom left; the image's is at the top.
    assert_eq!(image.pixel(20, 30), RED);
    assert_eq!(image.pixel(20, 15), WHITE);
    assert_eq!(image.pixel(5, 5), WHITE);
    // A line four points wide, centered on x = 60.
    assert_eq!(image.pixel(59, 25), Some([0, 0, 255, 255]));
    assert_eq!(image.pixel(61, 25), Some([0, 0, 255, 255]));
    assert_eq!(image.pixel(56, 25), WHITE);
    assert_eq!(image.pixel(60, 2), WHITE);
    // The lower half is filled only inside the clip, which ends with Q.
    assert_eq!(image.pixel(80, 40), BLACK);
    assert_eq!(image.pixel(80, 10), WHITE);
    assert_eq!(image.pixel(50, 40), WHITE);
    // Half-transparent green over white.
    assert_eq!(image.pixel(95, 5), Some([128, 255, 128, 255]));
    // The form, moved to the top left and clipped to five points square.
    assert_eq!(image.pixel(2, 2), BLACK);
    assert_eq!(image.pixel(8, 2), WHITE);
    assert_eq!(image.pixel(2, 8), WHITE);

    let doubled = document.render_page(0, 2.0).unwrap();
    assert_eq!((doubled.width(), doubled.height()), (200, 100));
    assert_eq!(doubled.pixel(40, 60), RED);

    // A quarter turn clockwise: the left edge becomes the top edge.
    let mut turned = Document::open(drawn_file(90)).unwrap();
    let image = turned.render_page(0, 1.0).unwrap();
    assert_eq!((image.width(), image.height()), (50, 100));
    assert_eq!(image.pixel(20, 20), RED);
    assert_eq!(image.pixel(40, 20), WHITE);
    assert_eq!(image.pixel(47, 2), BLACK);

    let unsupported = Err(DocumentError::Unsupported("a page image of this size"));
    assert_eq!(document.render_page(0, 0.0), unsupported);
    assert_eq!(document.render_page(0, f64::NAN), unsupported);
    assert_eq!(document.render_page(0, 1e6), unsupported);
    assert_eq!(
        document.render_page(9, 1.0),
        Err(DocumentError::Structure("a page index is out of range"))
    );
    // A region is refused for the same reasons, whatever the region is.
    for (left, width) in [(0, 10), (5000, 10), (0, 0)] {
        let region = PixelRegion {
            left,
            top: 0,
            width,
            height: 10,
        };
        assert_eq!(document.render_page_region(0, 0.0, region), unsupported);
        assert_eq!(
            document.render_page_region(0, f64::NAN, region),
            unsupported
        );
        assert_eq!(document.render_page_region(0, 1e6, region), unsupported);
        assert_eq!(
            document.render_page_region(9, 1.0, region),
            Err(DocumentError::Structure("a page index is out of range"))
        );
    }
    // Any rectangle of the image is drawn as that part of the whole image,
    // however the page is turned and at whole and fractional scales.
    for rotation in [0, 90, 180, 270] {
        let mut turned = Document::open(drawn_file(rotation)).unwrap();
        for scale in [1.0, 2.0, 4.54] {
            common::assert_regions_match(&mut turned, 0, scale);
        }
    }
    let png = image.to_png();
    assert!(png.starts_with(b"\x89PNG"));
}

/// A 400 point square page with text, paths, an image, a form, and an
/// inline image, for redaction.
fn redaction_file() -> Vec<u8> {
    let mut builder = sample();
    builder.object(
        3,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Rotate 0 /Contents 7 0 R \
         /Resources << /Font << /F1 8 0 R >> /XObject << /Im 9 0 R /Fm 10 0 R >> >> >>",
    );
    // Glyphs of a font without widths are half the font size wide: 5
    // points at 10. A stray `Q` at the end has no `q`.
    let content = b"BT /F1 10 Tf 14 TL 100 300 Td (Hello World) Tj
                    0 -20 Td [(AB) -500 (CD)] TJ (after) ' ET
                    0 0 1 rg 300 300 20 20 re f 50 50 300 10 re f q 310 310 5 5 re W f Q
                    q 100 0 0 100 0 0 cm /Im Do Q
                    q 1 0 0 1 200 100 cm /Fm Do Q
                    q 10 0 0 10 380 380 cm BI /W 1 /H 1 /BPC 8 /CS /G ID \x80\nEI Q Q";
    builder.stream(7, &format!("/Length {}", content.len()), content);
    // A font that is not one of the standard 14 and gives no widths.
    builder.object(8, "<< /Type /Font /Subtype /Type1 /BaseFont /Plain >>");
    // Two by two black pixels.
    builder.stream(
        9,
        "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray \
         /BitsPerComponent 8 /Length 4",
        &[0, 0, 0, 0],
    );
    let form = b"BT /F1 10 Tf 0 10 Td (Form text) Tj ET";
    builder.stream(
        10,
        &format!(
            "/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Length {} \
             /Resources << /Font << /F1 8 0 R >> >>",
            form.len()
        ),
        form,
    );
    builder.table(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], "/Size 11");
    builder.bytes
}

/// The areas `redaction_file` is redacted with.
fn redaction_areas() -> Vec<Rectangle> {
    vec![
        // "World", whose glyphs' middles are at 132.5 to 152.5.
        rectangle(128.0, 295.0, 160.0, 310.0),
        // "B" of the second line.
        rectangle(104.0, 278.0, 110.0, 290.0),
        // The blue square and the clipping square; the long bar crosses it.
        rectangle(290.0, 290.0, 330.0, 330.0),
        // The image's lower left pixel, and part of the bar.
        rectangle(0.0, 0.0, 60.0, 60.0),
        // "For" in the form.
        rectangle(195.0, 105.0, 215.0, 120.0),
        // Part of the inline image.
        rectangle(385.0, 385.0, 400.0, 400.0),
    ]
}

#[test]
fn redacts_text_paths_images_and_forms() {
    let mut document = Document::open(redaction_file()).unwrap();
    let areas = redaction_areas();
    let before = document.page_glyphs(0).unwrap();
    let result = document.redact(0, &areas, Some([1.0, 1.0, 1.0])).unwrap();
    assert_eq!(
        result,
        plain_open_pdf_engine::Redaction {
            glyphs: 9,
            images: 1,
            graphics: 3,
            kept_images: 1,
        }
    );

    // The glyphs left are those whose middles lie outside the areas, and
    // they stay where they were.
    let middle_inside = |glyph: &plain_open_pdf_engine::TextGlyph| {
        let x = (glyph.x + glyph.end_x) / 2.0;
        let y = glyph.y + 0.3 * glyph.size;
        areas.iter().any(|area| {
            (area.left..=area.right).contains(&x) && (area.bottom..=area.top).contains(&y)
        })
    };
    let place = |glyph: &plain_open_pdf_engine::TextGlyph| (glyph.text.clone(), glyph.x, glyph.y);
    let kept: Vec<_> = before
        .iter()
        .filter(|glyph| !middle_inside(glyph))
        .map(place)
        .collect();
    let after: Vec<_> = document.page_glyphs(0).unwrap().iter().map(place).collect();
    assert_eq!(after, kept);
    assert_eq!(after.len(), before.len() - 9);
    let text = document.page_text(0).unwrap();
    for gone in ["World", "B", "For"] {
        assert!(!text.contains(gone), "{gone} in {text:?}");
    }
    for stays in ["Hello", "CD", "after", "m text"] {
        assert!(text.contains(stays), "{stays} not in {text:?}");
    }

    let content = String::from_utf8_lossy(&document.page_content(0).unwrap()).into_owned();
    // The covered square is gone and the clip stays without painting; the
    // bar that crosses an area stays whole.
    assert!(!content.contains("300 300 20 20 re"));
    assert!(content.contains("310 310 5 5 re\nW\nn\n"));
    assert!(content.contains("50 50 300 10 re"));
    // The stray `Q` is dropped, and each area is painted white.
    assert!(content.starts_with("q\n"));
    assert_eq!(content.matches("1 1 1 rg").count(), areas.len());
    // The image and form are replaced by redacted copies; the originals
    // stay as they were for anything else that uses them.
    assert!(content.contains("/Redacted1 Do") && content.contains("/Redacted2 Do"));
    assert!(!content.contains("/Im Do") && !content.contains("/Fm Do"));
    assert_eq!(
        document
            .stream(ObjectReference {
                number: 9,
                generation: 0
            })
            .unwrap(),
        Some(vec![0; 4])
    );

    // Saved, the page no longer refers to the originals, so they are left
    // out of the file.
    let saved = document.save().unwrap();
    assert!(!contains(&saved, "Form text"));
    let mut reopened = Document::open(saved).unwrap();
    assert_eq!(reopened.page_text(0).unwrap(), text);
    let page = reopened.pages()[0].reference.unwrap();
    let PdfObject::Dictionary(page) = reopened.object(page).unwrap() else {
        panic!("the page is not a dictionary");
    };
    let Some(PdfObject::Dictionary(resources)) = page.get(b"Resources".as_slice()) else {
        panic!("the page has no resources of its own");
    };
    let Some(PdfObject::Dictionary(objects)) = resources.get(b"XObject".as_slice()) else {
        panic!("the resources have no XObjects");
    };
    let names: Vec<&[u8]> = objects.keys().map(Vec::as_slice).collect();
    assert_eq!(names, [b"Redacted1".as_slice(), b"Redacted2"]);
}

#[test]
fn blanks_the_pixels_of_an_image_under_an_area() {
    const WHITE: Option<[u8; 4]> = Some([255, 255, 255, 255]);
    const BLACK: Option<[u8; 4]> = Some([0, 0, 0, 255]);
    let mut document = Document::open(redaction_file()).unwrap();
    // Without a fill, what is left of the image shows.
    document
        .redact(0, &[rectangle(0.0, 0.0, 40.0, 40.0)], None)
        .unwrap();
    let image = document.render_page(0, 1.0).unwrap();
    // The page's lower left pixel of the image is white; the others are
    // black. The image's origin is at the top left.
    assert_eq!(image.pixel(25, 375), WHITE);
    assert_eq!(image.pixel(75, 375), BLACK);
    assert_eq!(image.pixel(25, 325), BLACK);
    common::assert_regions_match(&mut document, 0, 1.0);
    let content = document.page_content(0).unwrap();
    assert!(!contains(&content, " rg\n0 0 40 40 re"));

    // An image wholly inside an area is removed.
    let mut whole = Document::open(redaction_file()).unwrap();
    let result = whole
        .redact(0, &[rectangle(-1.0, -1.0, 101.0, 101.0)], None)
        .unwrap();
    assert_eq!(result.images, 1);
    let image = whole.render_page(0, 1.0).unwrap();
    assert_eq!(image.pixel(75, 325), WHITE);
    common::assert_regions_match(&mut whole, 0, 1.3);
}

#[test]
fn redaction_keeps_what_lies_outside_the_areas() {
    let mut document = Document::open(redaction_file()).unwrap();
    let before = document.page_text(0).unwrap();
    // Nothing lies here, an empty area is ignored, and the corners of an
    // area may come in any order.
    let areas = [
        rectangle(5.0, 390.0, 6.0, 395.0),
        rectangle(10.0, 10.0, 10.0, 20.0),
        rectangle(160.0, 310.0, 128.0, 295.0),
    ];
    let result = document.redact(0, &areas, None).unwrap();
    assert_eq!(result.glyphs, 5);
    assert_eq!(result.images + result.graphics + result.kept_images, 0);
    let after = document.page_text(0).unwrap();
    assert!(after.starts_with("Hello") && !after.contains("World"));
    assert!(after.lines().skip(1).eq(before.lines().skip(1)));
    assert_eq!(
        document.redact(1, &areas, None),
        Ok(plain_open_pdf_engine::Redaction::default())
    );
    assert_eq!(
        document.redact(7, &areas, None),
        Err(DocumentError::Structure("a page index is out of range"))
    );
}

#[test]
fn inserts_text_in_a_standard_font() {
    use plain_open_pdf_engine::StandardFont;
    let mut builder = sample();
    // The page's content scales and colors what follows without `q`.
    builder.object(
        3,
        "<< /Type /Page /Parent 2 0 R /Rotate 0 /Contents 7 0 R >>",
    );
    let content = b"2 0 0 2 0 0 cm 0 0 1 rg 0 0 10 10 re f";
    builder.stream(7, &format!("/Length {}", content.len()), content);
    builder.table(&[1, 2, 3, 4, 5, 6, 7], "/Size 8");
    let mut document = Document::open(builder.bytes).unwrap();

    let red = [1.0, 0.0, 0.0];
    let font = StandardFont::Helvetica;
    let width = document
        .insert_text(
            0,
            (100.0, 200.0),
            "Caf\u{e9} \u{4e2d}\nNext",
            font,
            12.0,
            red,
        )
        .unwrap();
    assert_eq!(width, font.text_width("Caf\u{e9} ?", 12.0));
    document
        .insert_text(0, (100.0, 100.0), "More", font, 10.0, [0.0; 3])
        .unwrap();
    let glyphs = document.page_glyphs(0).unwrap();
    let text: String = glyphs.iter().map(|glyph| glyph.text.as_str()).collect();
    assert_eq!(text, "Caf\u{e9} ?NextMore");
    // The page's own scale does not reach the text.
    let first = &glyphs[0];
    assert_eq!((first.x, first.y, first.size), (100.0, 200.0, 12.0));
    assert_eq!((&*first.font, first.color), ("Helvetica", [255, 0, 0]));
    assert_eq!(glyphs[1].x, 100.0 + font.text_width("C", 12.0));
    // The next line starts 1.2 times the size lower.
    assert_eq!((glyphs[6].x, glyphs[6].y), (100.0, 200.0 - 14.4));
    assert_eq!((glyphs[10].y, glyphs[10].size), (100.0, 10.0));

    // Both insertions share one font resource, which gives its widths.
    let saved = document.save().unwrap();
    let mut reopened = Document::open(saved).unwrap();
    let page = reopened.pages()[0].reference.unwrap();
    let PdfObject::Dictionary(page) = reopened.object(page).unwrap() else {
        panic!("the page is not a dictionary");
    };
    let Some(PdfObject::Dictionary(resources)) = page.get(b"Resources".as_slice()) else {
        panic!("the page has no resources of its own");
    };
    let Some(PdfObject::Dictionary(fonts)) = resources.get(b"Font".as_slice()) else {
        panic!("the resources have no fonts");
    };
    assert_eq!(fonts.len(), 1);
    assert_eq!(reopened.page_glyphs(0).unwrap().len(), glyphs.len());
    let content = String::from_utf8_lossy(&reopened.page_content(0).unwrap()).into_owned();
    // Each insertion encloses what came before it in `q` and `Q`.
    let words: Vec<&str> = content.split_whitespace().take(3).collect();
    assert_eq!(words, ["q", "q", "2"]);

    // A page without content gets the text alone.
    assert_eq!(
        reopened
            .insert_text(1, (10.0, 20.0), "\u{3b1}", StandardFont::Symbol, 9.0, red)
            .unwrap(),
        StandardFont::Symbol.text_width("\u{3b1}", 9.0)
    );
    assert_eq!(reopened.page_text(1).unwrap().trim(), "\u{3b1}");
    let structure = |reason| Err(DocumentError::Structure(reason));
    assert_eq!(
        reopened.insert_text(9, (0.0, 0.0), "x", font, 9.0, red),
        structure("a page index is out of range")
    );
    assert_eq!(
        reopened.insert_text(0, (0.0, 0.0), "x", font, 0.0, red),
        structure("a font size is positive")
    );
}

#[test]
fn adds_replaces_and_removes_embedded_files() {
    let mut document = Document::open(linked_file()).unwrap();
    assert_eq!(
        document.attachment_data("data.csv").unwrap(),
        Some(b"a,b\n1,2".to_vec())
    );
    assert_eq!(document.attachment_data("missing").unwrap(), None);

    document
        .add_attachment("notes \u{e9}.txt", b"first", None, Some("Notes"))
        .unwrap();
    document
        .add_attachment("notes \u{e9}.txt", b"second", Some("n.txt"), None)
        .unwrap();
    assert!(document.remove_attachment("data.csv").unwrap());
    assert!(!document.remove_attachment("data.csv").unwrap());

    let saved = document.save().unwrap();
    let mut reopened = Document::open(saved).unwrap();
    let names: Vec<String> = reopened
        .attachments()
        .unwrap()
        .into_iter()
        .map(|file| file.name)
        .collect();
    // The tree is written again sorted by its keys' bytes; a key outside
    // PDFDocEncoding is UTF-16, whose byte order mark sorts last.
    assert_eq!(names, ["broken", "key two", "notes \u{e9}.txt"]);
    assert_eq!(
        reopened.attachment_data("notes \u{e9}.txt").unwrap(),
        Some(b"second".to_vec())
    );
    let notes = reopened.attachments().unwrap().pop().unwrap();
    assert_eq!(notes.file_name.as_deref(), Some("n.txt"));
    assert_eq!(notes.description, None);
    // The other trees of the name dictionary stay.
    assert!(reopened.named_destination(b"second").unwrap().is_some());

    // Removing every file removes the tree.
    for name in names {
        reopened.remove_attachment(&name).unwrap();
    }
    assert_eq!(reopened.attachments().unwrap(), []);
    assert!(reopened.named_destination(b"second").unwrap().is_some());
}

#[test]
fn sets_reads_and_removes_xmp_metadata() {
    let mut document = Document::open(sample_file()).unwrap();
    assert_eq!(document.xmp_metadata().unwrap(), None);
    let xmp = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"></x:xmpmeta>";
    document.set_xmp_metadata(Some(xmp)).unwrap();
    assert_eq!(document.xmp_metadata().unwrap(), Some(xmp.to_vec()));
    let saved = document.save().unwrap();
    // XMP is stored uncompressed, so other tools can find it.
    assert!(contains(&saved, "<x:xmpmeta"));
    let mut reopened = Document::open(saved).unwrap();
    assert_eq!(reopened.xmp_metadata().unwrap(), Some(xmp.to_vec()));
    reopened.set_xmp_metadata(None).unwrap();
    assert_eq!(reopened.xmp_metadata().unwrap(), None);
    assert!(!contains(&reopened.save().unwrap(), "xmpmeta"));
}

#[test]
fn builds_a_document_from_nothing() {
    use plain_open_pdf_engine::{StandardFont, View};
    let mut document = Document::default();
    assert_eq!(document.page_count(), 0);
    for _ in 0..3 {
        document.insert_blank_page(0, 612.0, 792.0).unwrap();
    }
    document
        .insert_text(
            0,
            (72.0, 720.0),
            "Link here",
            StandardFont::Helvetica,
            12.0,
            [0.0; 3],
        )
        .unwrap();
    let area = rectangle(72.0, 700.0, 152.0, 724.0);
    let to_page = LinkTarget::Page(1);
    let view = Some(View::Xyz {
        left: Some(20.0),
        top: Some(542.0),
        zoom: None,
    });
    document.add_link(0, area, &to_page, view).unwrap();
    let web = LinkTarget::Uri("https://example.com/spec".to_string());
    document
        .add_link(0, rectangle(152.0, 724.0, 72.0, 700.0), &web, None)
        .unwrap();
    let other = LinkTarget::RemoteDocument {
        file: "other.pdf".to_string(),
        page: Some(3),
    };
    document.add_link(1, area, &other, None).unwrap();
    document
        .add_link(1, area, &LinkTarget::Launch("notes.txt".to_string()), None)
        .unwrap();
    document
        .add_link(1, area, &LinkTarget::Named("NextPage".to_string()), None)
        .unwrap();
    document
        .add_link(2, area, &LinkTarget::Page(0), None)
        .unwrap();
    assert_eq!(
        document.add_link(0, area, &LinkTarget::Page(7), None),
        Err(DocumentError::Structure("a page index is out of range"))
    );
    document
        .set_information("Department", Some("Caf\u{e9}"))
        .unwrap();
    document.set_information("Title", Some("Built")).unwrap();

    let mut reopened = Document::open(document.save().unwrap()).unwrap();
    assert_eq!(reopened.page_count(), 3);
    assert_eq!(reopened.page_text(0).unwrap().trim(), "Link here");
    let first = reopened.links(0).unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!((first[0].target.clone(), first[0].view), (to_page, view));
    assert_eq!(first[0].rectangle, area);
    // Corners in any order give the same rectangle.
    assert_eq!((first[1].target.clone(), first[1].rectangle), (web, area));
    let second: Vec<_> = reopened
        .links(1)
        .unwrap()
        .into_iter()
        .map(|link| link.target)
        .collect();
    assert_eq!(
        second,
        [
            other,
            LinkTarget::Launch("notes.txt".to_string()),
            LinkTarget::Named("NextPage".to_string()),
        ]
    );
    // Without a view, a link opens the top of its page.
    let back = &reopened.links(2).unwrap()[0];
    let top = View::Xyz {
        left: None,
        top: Some(792.0),
        zoom: None,
    };
    assert_eq!(
        (back.target.clone(), back.view),
        (LinkTarget::Page(0), Some(top))
    );
    let mut entries = reopened.information_entries().unwrap();
    entries.sort();
    assert_eq!(
        entries,
        [
            ("Department".to_string(), "Caf\u{e9}".to_string()),
            ("Title".to_string(), "Built".to_string()),
        ]
    );
}

#[test]
fn gives_rendered_pixels_as_rgb_or_gray() {
    let mut document = Document::open(drawn_file(0)).unwrap();
    let image = document.render_page(0, 1.0).unwrap();
    let rgb = image.rgb();
    let gray = image.gray();
    assert_eq!((rgb.len(), gray.len()), (100 * 50 * 3, 100 * 50));
    // (20, 30) is red, (5, 5) white, and (80, 40) black.
    let at = |x: usize, y: usize| y * 100 + x;
    assert_eq!(rgb[at(20, 30) * 3..at(20, 30) * 3 + 3], [255, 0, 0]);
    assert_eq!(
        (gray[at(20, 30)], gray[at(5, 5)], gray[at(80, 40)]),
        (76, 255, 0)
    );
}

#[test]
fn draws_inline_images() {
    let mut builder = sample();
    builder.object(
        3,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Rotate 0 /Contents 7 0 R >>",
    );
    // Two RGB pixels, a gray pixel in hexadecimal, and a stencil mask
    // painted in the fill color.
    let content: &[u8] =
        b"q 40 0 0 20 10 10 cm BI /W 2 /H 1 /BPC 8 /CS /RGB ID \xff\x00\x00\x00\x00\xff\nEI Q
        q 10 0 0 10 60 60 cm BI /W 1 /H 1 /BPC 8 /CS /G /F /AHx ID 80>\nEI Q
        0 1 0 rg q 10 0 0 10 80 10 cm BI /IM true /W 1 /H 1 ID \x00\nEI Q
        q 10 0 0 10 80 60 cm BI /W 1 /H 1 /BPC 8 /CS /G /F /JPX ID \x00\nEI Q";
    builder.stream(7, &format!("/Length {}", content.len()), content);
    builder.table(&[1, 2, 3, 4, 5, 6, 7], "/Size 8");
    let mut document = Document::open(builder.bytes).unwrap();
    let image = document.render_page(0, 1.0).unwrap();
    // The page's origin is at the bottom left; the image's at the top.
    assert_eq!(image.pixel(20, 80), Some([255, 0, 0, 255]));
    assert_eq!(image.pixel(40, 80), Some([0, 0, 255, 255]));
    assert_eq!(image.pixel(65, 35), Some([128, 128, 128, 255]));
    assert_eq!(image.pixel(85, 85), Some([0, 255, 0, 255]));
    // An image in a filter that is not supported is left out.
    assert_eq!(image.pixel(85, 35), Some([255, 255, 255, 255]));
    assert_eq!(image.pixel(5, 5), Some([255, 255, 255, 255]));
    for scale in [1.0, 0.3, 2.75] {
        common::assert_regions_match(&mut document, 0, scale);
    }
}

#[test]
fn draws_shadings_patterns_and_separation_colors() {
    let mut builder = sample();
    builder.object(
        3,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Rotate 0 /Contents 7 0 R \
         /Resources << /Shading << /Sh1 8 0 R /Sh2 12 0 R >> \
         /Pattern << /P1 13 0 R /P2 14 0 R /P3 15 0 R >> \
         /ColorSpace << /Cs1 [/Pattern /DeviceRGB] /Sep [/Separation /Red /DeviceRGB 16 0 R] \
         /Sep2 [/Separation /Green /DeviceRGB 17 0 R] >> >> >>",
    );
    let content = b"q 0 0 100 50 re W n /Sh1 sh Q
        q 100 0 100 50 re W n /Sh2 sh Q
        /Pattern cs /P1 scn 0 50 50 50 re f
        /P2 scn 50 50 50 50 re f
        /Cs1 cs 0 0 1 /P3 scn 100 50 50 50 re f
        /Sep cs 1 sc 150 50 25 25 re f
        /Sep2 cs 0.5 sc 175 50 25 25 re f";
    builder.stream(7, &format!("/Length {}", content.len()), content);
    // Red to green to blue, left to right, stitched from two functions.
    builder.object(
        8,
        "<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 100 0] /Function 9 0 R \
         /Extend [true true] >>",
    );
    builder.object(
        9,
        "<< /FunctionType 3 /Domain [0 1] /Functions [10 0 R 11 0 R] /Bounds [0.5] \
         /Encode [0 1 0 1] >>",
    );
    builder.object(
        10,
        "<< /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 1 0] /N 1 >>",
    );
    builder.object(
        11,
        "<< /FunctionType 2 /Domain [0 1] /C0 [0 1 0] /C1 [0 0 1] /N 1 >>",
    );
    // White at the center to black at the edge of a circle 25 across.
    builder.object(
        12,
        "<< /ShadingType 3 /ColorSpace /DeviceGray /Coords [150 25 0 150 25 25] \
         /Function << /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >> >>",
    );
    // The first shading, squeezed to half its width, as a pattern.
    builder.object(
        13,
        "<< /PatternType 2 /Shading 8 0 R /Matrix [0.5 0 0 1 0 50] >>",
    );
    let cell = b"0 0 5 5 re f";
    for (number, paint) in [(14, 1), (15, 2)] {
        builder.stream(
            number,
            &format!(
                "/PatternType 1 /PaintType {paint} /TilingType 1 /BBox [0 0 10 10] \
                 /XStep 10 /YStep 10 /Resources << >> /Length {}",
                cell.len()
            ),
            cell,
        );
    }
    // A tint as red, green, blue: (t, 0, 1 - t).
    let program = b"{ dup 0 exch 1 exch sub }";
    builder.stream(
        16,
        &format!(
            "/FunctionType 4 /Domain [0 1] /Range [0 1 0 1 0 1] /Length {}",
            program.len()
        ),
        program,
    );
    // Sampled: no ink is white, all of it green.
    builder.stream(
        17,
        "/FunctionType 0 /Domain [0 1] /Range [0 1 0 1 0 1] /Size [2] /BitsPerSample 8 /Length 6",
        &[255, 255, 255, 0, 255, 0],
    );
    let numbers: Vec<u32> = (1..=17).collect();
    builder.table(&numbers, "/Size 18");
    let mut document = Document::open(builder.bytes).unwrap();
    let image = document.render_page(0, 1.0).unwrap();
    let near = |x: usize, y: usize, expected: [u8; 3]| {
        let pixel = image.pixel(x, y).unwrap();
        let close = pixel.iter().zip(expected).all(|(&a, b)| a.abs_diff(b) <= 4);
        assert!(close, "({x}, {y}) is {pixel:?}, not near {expected:?}");
    };
    // Page heights are flipped: a page point (x, y) is pixel (x, 100 - y).
    // The axial shading: t is 0.025, 0.505, and 0.975 at these pixels.
    near(2, 75, [242, 13, 0]);
    near(50, 75, [0, 252, 3]);
    near(97, 75, [0, 13, 242]);
    // The radial shading: 0.5 and 20.5 points from the center, and
    // nothing past its edge.
    near(150, 74, [250, 250, 250]);
    near(170, 74, [46, 46, 46]);
    near(105, 99, [255, 255, 255]);
    // The shading pattern, at half width: t is 0.05 at x = 2.5.
    near(2, 25, [230, 26, 0]);
    // The colored tiling pattern: a black square in each 10-point cell.
    near(52, 47, [0, 0, 0]);
    near(57, 47, [255, 255, 255]);
    near(62, 37, [0, 0, 0]);
    // The uncolored one, in the blue given with it.
    near(102, 47, [0, 0, 255]);
    near(107, 47, [255, 255, 255]);
    // Separation colors through a calculator and a sampled function.
    near(160, 40, [255, 0, 0]);
    near(185, 40, [128, 255, 128]);
    for scale in [1.0, 0.45, 2.3] {
        common::assert_regions_match(&mut document, 0, scale);
    }
}

#[test]
fn draws_type3_glyphs_and_annotation_appearances() {
    let mut builder = sample();
    builder.object(
        3,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Rotate 0 /Contents 7 0 R \
         /Resources << /Font << /F3 8 0 R >> >> /Annots [11 0 R 13 0 R 15 0 R] >>",
    );
    let content = b"1 0 0 rg BT /F3 20 Tf 10 10 Td (AAB) Tj ET";
    builder.stream(7, &format!("/Length {}", content.len()), content);
    builder.object(
        8,
        "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] \
         /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /square 9 0 R /blue 10 0 R >> \
         /Encoding << /Type /Encoding /Differences [65 /square /blue] >> \
         /FirstChar 65 /LastChar 66 /Widths [1000 1000] /Resources << >> >>",
    );
    // A shape in the text's color, and a glyph with its own color.
    let square = b"1000 0 0 0 750 750 d1 0 0 750 750 re f";
    builder.stream(9, &format!("/Length {}", square.len()), square);
    let blue = b"1000 0 d0 0 0 1 rg 0 0 500 500 re f";
    builder.stream(10, &format!("/Length {}", blue.len()), blue);
    // A green square appearance, stretched from 10 points onto 40.
    builder.object(
        11,
        "<< /Type /Annot /Subtype /Square /Rect [100 50 140 90] /AP << /N 12 0 R >> >>",
    );
    let green = b"0 1 0 rg 0 0 10 10 re f";
    builder.stream(
        12,
        &format!(
            "/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Length {}",
            green.len()
        ),
        green,
    );
    // A hidden annotation is not drawn.
    builder.object(
        13,
        "<< /Type /Annot /Subtype /Square /F 2 /Rect [150 50 190 90] /AP << /N 12 0 R >> >>",
    );
    // An appearance chosen by state.
    builder.object(
        15,
        "<< /Type /Annot /Subtype /Widget /Rect [100 10 120 30] /AS /Off \
         /AP << /N << /On 12 0 R /Off 16 0 R >> >> >>",
    );
    let gray = b"0.5 g 0 0 1 1 re f";
    builder.stream(
        16,
        &format!(
            "/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length {}",
            gray.len()
        ),
        gray,
    );
    let numbers: Vec<u32> = (1..=13).chain([15, 16]).collect();
    builder.table(&numbers, "/Size 17");
    let mut document = Document::open(builder.bytes).unwrap();
    let image = document.render_page(0, 1.0).unwrap();
    // A page point (x, y) is pixel (x, 100 - y). The squares are 15
    // points wide at 20 points, one glyph of 20 points apart.
    assert_eq!(image.pixel(15, 85), Some([255, 0, 0, 255]));
    assert_eq!(image.pixel(35, 85), Some([255, 0, 0, 255]));
    assert_eq!(image.pixel(27, 85), Some([255, 255, 255, 255]));
    assert_eq!(image.pixel(55, 85), Some([0, 0, 255, 255]));
    assert_eq!(image.pixel(65, 85), Some([255, 255, 255, 255]));
    assert_eq!(image.pixel(120, 30), Some([0, 255, 0, 255]));
    assert_eq!(image.pixel(170, 30), Some([255, 255, 255, 255]));
    assert_eq!(image.pixel(110, 80), Some([128, 128, 128, 255]));
    for scale in [1.0, 1.7] {
        common::assert_regions_match(&mut document, 0, scale);
    }
}

#[test]
fn blends_with_multiply_as_highlights_do() {
    let mut builder = sample();
    builder.object(
        3,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Rotate 0 /Contents 7 0 R \
         /Resources << /ExtGState << /Multiply << /BM /Multiply >> \
         /Darken << /BM [/Hue /Darken] >> >> >> >>",
    );
    // A black bar and a gray bar, then yellow over both with Multiply, and
    // blue over the gray bar with Darken.
    let content = b"0 g 10 10 20 80 re f 0.5 g 60 10 20 80 re f
        q /Multiply gs 1 1 0 rg 0 40 100 20 re f Q
        q /Darken gs 0 0 1 rg 60 70 20 20 re f Q";
    builder.stream(7, &format!("/Length {}", content.len()), content);
    builder.table(&[1, 2, 3, 4, 5, 6, 7], "/Size 8");
    let mut document = Document::open(builder.bytes).unwrap();
    let image = document.render_page(0, 1.0).unwrap();
    // Multiplying keeps black black and turns white yellow, and gray into
    // a darker yellow; without blending the yellow would cover the bars.
    assert_eq!(image.pixel(20, 50), Some([0, 0, 0, 255]));
    assert_eq!(image.pixel(45, 50), Some([255, 255, 0, 255]));
    assert_eq!(image.pixel(70, 50), Some([128, 128, 0, 255]));
    // Darken keeps the darker of each component; an unknown mode before
    // it in the list is skipped.
    assert_eq!(image.pixel(70, 20), Some([0, 0, 128, 255]));
    common::assert_regions_match(&mut document, 0, 1.0);
    common::assert_regions_match(&mut document, 0, 3.1);
}
