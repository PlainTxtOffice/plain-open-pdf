//! Opening documents encrypted with the standard security handler.

mod fixtures;

use fixtures::ENCRYPTED;
use plain_open_pdf_engine::{Document, DocumentError, PdfObject, Permissions};

fn unhex(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text
        .bytes()
        .map(|byte| (byte as char).to_digit(16).expect("hex digit") as u8)
        .collect();
    digits
        .chunks(2)
        .map(|pair| pair[0] << 4 | pair[1])
        .collect()
}

/// The decrypted title from the document information dictionary.
fn title(document: &mut Document) -> Vec<u8> {
    let Some(PdfObject::Reference(info)) = document.trailer().get(b"Info".as_slice()) else {
        panic!("the trailer names an information dictionary");
    };
    let info = document.object(*info).unwrap();
    let info = info.as_dictionary().expect("information is a dictionary");
    match &info[b"Title".as_slice()] {
        PdfObject::String(title) => title.clone(),
        other => panic!("title is a string, not {other:?}"),
    }
}

#[test]
fn covers_every_cipher_and_both_storage_layouts() {
    let names: Vec<_> = ENCRYPTED.iter().map(|fixture| fixture.0).collect();
    assert_eq!(
        names,
        [
            "rc4-40-direct",
            "rc4-40-objects",
            "rc4-128-direct",
            "rc4-128-objects",
            "aes-128-direct",
            "aes-128-objects",
            "aes-256-direct",
            "aes-256-objects",
        ]
    );
}

#[test]
fn opens_with_the_user_or_owner_password() {
    for (name, file) in ENCRYPTED {
        for password in [b"user".as_slice(), b"owner"] {
            let mut document = Document::open_with_password(unhex(file), password)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(document.page_count(), 1, "{name}");
            let page = document.pages()[0];
            assert_eq!(page.media_box.width(), 300.0, "{name}");
            assert_eq!(page.media_box.height(), 500.0, "{name}");
            assert_eq!(page.rotation, 90, "{name}");
            assert_eq!(title(&mut document), b"Secret title", "{name}");
        }
    }
}

#[test]
fn saves_decrypted_documents() {
    for (name, file) in ENCRYPTED {
        let mut document = Document::open_with_password(unhex(file), b"user").unwrap();
        let saved = document.save_decrypted().unwrap();
        let mut reopened = Document::open(saved).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(!reopened.trailer().contains_key(b"Encrypt".as_slice()));
        assert_eq!(reopened.pages(), document.pages(), "{name}");
        assert_eq!(title(&mut reopened), b"Secret title", "{name}");
        assert_eq!(
            reopened.page_content(0).unwrap(),
            document.page_content(0).unwrap(),
            "{name}"
        );
    }
}

#[test]
fn refuses_a_missing_or_wrong_password() {
    for (name, file) in ENCRYPTED {
        assert_eq!(
            Document::open(unhex(file)).unwrap_err(),
            DocumentError::PasswordRequired,
            "{name}"
        );
        // A password that is given and wrong is told apart from none.
        for password in [b"User".as_slice(), &[0xff; 200]] {
            assert_eq!(
                Document::open_with_password(unhex(file), password).unwrap_err(),
                DocumentError::WrongPassword,
                "{name}"
            );
        }
    }
}

#[test]
fn damaged_encrypted_files_never_panic() {
    for (name, file) in ENCRYPTED {
        let file = unhex(file);
        // AES-256 password checks are slow by design, so sample them thinly.
        let step = if name.starts_with("aes-256") { 53 } else { 3 };
        for index in (0..file.len()).step_by(step) {
            let mut damaged = file.clone();
            damaged[index] ^= 0x41;
            let _ = Document::open_with_password(damaged, b"user");
        }
    }
}

#[test]
fn keeps_encryption_when_saving() {
    for (name, file) in ENCRYPTED {
        for opened_with in [b"user".as_slice(), b"owner"] {
            let mut document = Document::open_with_password(unhex(file), opened_with).unwrap();
            let method = document.encryption().map(str::to_owned);
            let cipher = if name.starts_with("rc4") {
                "RC4"
            } else {
                "AES"
            };
            assert!(
                method
                    .as_deref()
                    .is_some_and(|method| method.ends_with(cipher)),
                "{name}"
            );
            document
                .set_information("Subject", Some("Changed"))
                .unwrap();
            let saved = document.save().unwrap();
            // Without a password the saved file does not open, and its
            // strings are not stored in the clear.
            assert_eq!(
                Document::open(saved.clone()).unwrap_err(),
                DocumentError::PasswordRequired,
                "{name}"
            );
            assert!(!saved.windows(7).any(|part| part == b"Changed"), "{name}");
            for password in [b"user".as_slice(), b"owner"] {
                let mut reopened = Document::open_with_password(saved.clone(), password)
                    .unwrap_or_else(|error| panic!("{name}: {error}"));
                assert_eq!(reopened.encryption().map(str::to_owned), method, "{name}");
                assert_eq!(reopened.permissions(), document.permissions(), "{name}");
                assert_eq!(title(&mut reopened), b"Secret title", "{name}");
                let information = reopened.information().unwrap();
                assert_eq!(information.subject.as_deref(), Some("Changed"), "{name}");
                assert_eq!(
                    reopened.page_content(0).unwrap(),
                    document.page_content(0).unwrap(),
                    "{name}"
                );
            }
        }
    }
}

#[test]
fn sets_new_aes_256_protection() {
    let (_, file) = ENCRYPTED[0];
    let mut document = Document::open_with_password(unhex(file), b"owner").unwrap();
    let allowed = Permissions {
        copy: false,
        modify: false,
        ..Permissions::ALL
    };
    let saved = document
        .save_encrypted("new user", "new owner", allowed)
        .unwrap();
    assert_eq!(
        Document::open(saved.clone()).unwrap_err(),
        DocumentError::PasswordRequired
    );
    for password in [b"user".as_slice(), b"owner"] {
        assert_eq!(
            Document::open_with_password(saved.clone(), password).unwrap_err(),
            DocumentError::WrongPassword
        );
    }
    for password in [b"new user".as_slice(), b"new owner"] {
        let mut reopened = Document::open_with_password(saved.clone(), password).unwrap();
        assert_eq!(reopened.encryption(), Some("Standard V5 R6 256-bit AES"));
        assert_eq!(reopened.permissions(), Some(allowed));
        assert_eq!(title(&mut reopened), b"Secret title");
        assert_eq!(
            reopened.page_content(0).unwrap(),
            document.page_content(0).unwrap()
        );
        // Saving again keeps the new protection.
        let again = reopened.save().unwrap();
        let mut third = Document::open_with_password(again, b"new user").unwrap();
        assert_eq!(title(&mut third), b"Secret title");
    }
    // An empty user password lets anyone open the file.
    let open = document
        .save_encrypted("", "owner only", Permissions::NONE)
        .unwrap();
    let mut reopened = Document::open(open).unwrap();
    assert_eq!(reopened.permissions(), Some(Permissions::NONE));
    assert_eq!(title(&mut reopened), b"Secret title");
}

#[test]
fn permissions_round_trip_through_their_bits() {
    assert_eq!(
        Permissions::from_bits(Permissions::ALL.bits()),
        Permissions::ALL
    );
    assert_eq!(
        Permissions::from_bits(Permissions::NONE.bits()),
        Permissions::NONE
    );
    // The reserved bits are set: -3904 as a signed number.
    assert_eq!(Permissions::NONE.bits() as i32, -3904);
    assert_eq!(Permissions::ALL.bits() as i32, -4);
    let print_only = Permissions {
        print: true,
        ..Permissions::NONE
    };
    assert_eq!(print_only.bits() & 0xFFF, 0x0C4);
}
