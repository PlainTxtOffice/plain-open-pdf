//! Header acceptance and rejection cases at the public library boundary.

use plain_open_pdf_engine::{HeaderError, PdfVersion, parse_header};

#[test]
fn recognizes_standard_versions() {
    let versions = (0..=7).map(|minor| (1, minor)).chain([(2, 0)]);
    for (major, minor) in versions {
        for ending in ["\n", "\r", "\r\n"] {
            let input = format!("%PDF-{major}.{minor}{ending}");
            assert_eq!(
                parse_header(input.as_bytes()),
                Ok(PdfVersion { major, minor })
            );
        }
    }
}

#[test]
fn rejects_every_truncated_prefix() {
    let input = b"%PDF-1.7\n";
    for length in 0..8 {
        assert_eq!(
            parse_header(&input[..length]),
            Err(HeaderError::TruncatedHeader)
        );
    }
    assert_eq!(
        parse_header(&input[..8]),
        Ok(PdfVersion { major: 1, minor: 7 })
    );
    assert_eq!(
        parse_header(b"junk %PDF-1."),
        Err(HeaderError::TruncatedHeader)
    );
}

#[test]
fn rejects_missing_or_late_signatures() {
    assert_eq!(
        parse_header(b"%PDX-1.7\n"),
        Err(HeaderError::InvalidSignature)
    );
    let mut late = vec![b' '; 1024];
    late.extend(b"%PDF-1.7\n");
    assert_eq!(parse_header(&late), Err(HeaderError::InvalidSignature));
}

#[test]
fn accepts_leading_bytes_before_signature() {
    let mut latest = vec![b' '; 1023];
    latest.extend(b"%PDF-1.7\n");
    for input in [
        b" %PDF-1.7\n".as_slice(),
        b"\xef\xbb\xbf%PDF-1.7\n",
        b"mail header\r\n%PDF-1.7\n",
        &latest,
    ] {
        assert_eq!(parse_header(input), Ok(PdfVersion { major: 1, minor: 7 }));
    }
}

#[test]
fn rejects_malformed_versions() {
    for input in [
        b"%PDF-x.7\n",
        b"%PDF-1.x\n",
        b"%PDF-1-7\n",
        b"%PDF-\xff.7\n",
    ] {
        assert_eq!(parse_header(input), Err(HeaderError::InvalidVersion));
    }
}

#[test]
fn rejects_unknown_versions() {
    for (major, minor) in [(0, 9), (1, 8), (2, 1), (9, 9)] {
        let input = format!("%PDF-{major}.{minor}\n");
        assert_eq!(
            parse_header(input.as_bytes()),
            Err(HeaderError::UnsupportedVersion(PdfVersion { major, minor }))
        );
    }
}

#[test]
fn ignores_bytes_after_version() {
    for suffix in [b' ', b'\t', 0, b'0', b'%'] {
        let mut input = b"%PDF-1.7".to_vec();
        input.push(suffix);
        assert_eq!(parse_header(&input), Ok(PdfVersion { major: 1, minor: 7 }));
    }
}

#[test]
fn accepts_binary_bytes_after_header() {
    assert_eq!(
        parse_header(b"%PDF-1.7\n%\xff\xfe\x80\x81\n"),
        Ok(PdfVersion { major: 1, minor: 7 })
    );
}
