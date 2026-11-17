//! The standard security handler: passwords, file keys, encryption, and
//! decryption (ISO 32000-1 section 7.6 and ISO 32000-2 section 7.6).

use super::DocumentError;
use super::reader::{Reader, entry, size_entry};
use crate::crypto::{cbc_decrypt, cbc_encrypt, md5, random_bytes, rc4, sha256, sha384, sha512};
use crate::{Dictionary, PdfObject};

/// What the user of an encrypted document may do without the owner
/// password (ISO 32000-2 section 7.6.4.2, table 22). Viewers are trusted to
/// honor these; they do not change how the document is encrypted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    /// Print the document.
    pub print: bool,
    /// Change the document in ways the other permissions do not cover.
    pub modify: bool,
    /// Copy or otherwise extract text and graphics.
    pub copy: bool,
    /// Add or change annotations, and fill in form fields.
    pub annotate: bool,
    /// Fill in form fields, even without `annotate`.
    pub fill_forms: bool,
    /// Extract text and graphics for accessibility.
    pub extract_for_accessibility: bool,
    /// Insert, rotate, or delete pages and change bookmarks.
    pub assemble: bool,
    /// Print at full quality, not only a low-resolution image.
    pub print_high_quality: bool,
}

impl Permissions {
    /// Everything allowed.
    pub const ALL: Self = Self {
        print: true,
        modify: true,
        copy: true,
        annotate: true,
        fill_forms: true,
        extract_for_accessibility: true,
        assemble: true,
        print_high_quality: true,
    };

    /// Nothing allowed.
    pub const NONE: Self = Self {
        print: false,
        modify: false,
        copy: false,
        annotate: false,
        fill_forms: false,
        extract_for_accessibility: false,
        assemble: false,
        print_high_quality: false,
    };

    /// The permissions an encryption dictionary's `/P` value grants.
    pub fn from_bits(bits: u32) -> Self {
        let bit = |number: u32| bits & 1 << (number - 1) != 0;
        Self {
            print: bit(3),
            modify: bit(4),
            copy: bit(5),
            annotate: bit(6),
            fill_forms: bit(9),
            extract_for_accessibility: bit(10),
            assemble: bit(11),
            print_high_quality: bit(12),
        }
    }

    /// The `/P` value for these permissions, with the bits the standard
    /// reserves set as it requires.
    pub fn bits(self) -> u32 {
        let flags = [
            (3, self.print),
            (4, self.modify),
            (5, self.copy),
            (6, self.annotate),
            (9, self.fill_forms),
            (10, self.extract_for_accessibility),
            (11, self.assemble),
            (12, self.print_high_quality),
        ];
        let reserved = 0xFFFF_F0C0;
        flags
            .iter()
            .filter(|(_, allowed)| *allowed)
            .fold(reserved, |bits, (number, _)| bits | 1 << (number - 1))
    }
}

/// The bytes that pad or stand in for a password in revisions 2 through 4.
const PASSWORD_PADDING: [u8; 32] = [
    0x28, 0xbf, 0x4e, 0x5e, 0x4e, 0x75, 0x8a, 0x41, 0x64, 0x00, 0x4e, 0x56, 0xff, 0xfa, 0x01, 0x08,
    0x2e, 0x2e, 0x00, 0xb6, 0xd0, 0x68, 0x3e, 0x80, 0x2f, 0x0c, 0xa9, 0xfe, 0x64, 0x53, 0x69, 0x7a,
];

/// How strings or streams are encrypted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cipher {
    /// Left as written.
    Identity,
    /// RC4 with a key derived for each object.
    Rc4,
    /// AES-128 in CBC mode with a key derived for each object.
    Aes128,
    /// AES-256 in CBC mode with the file key.
    Aes256,
}

/// The entries of an encryption dictionary that key derivation reads.
struct Parameters<'a> {
    revision: i64,
    key_length: usize,
    owner: &'a [u8],
    user: &'a [u8],
    owner_key: &'a [u8],
    user_key: &'a [u8],
    permissions: u32,
    id: &'a [u8],
    encrypt_metadata: bool,
}

/// Decrypts the strings and streams of one document, and encrypts them
/// again the same way.
#[derive(Debug, Clone)]
pub(super) struct Decryptor {
    key: Vec<u8>,
    strings: Cipher,
    streams: Cipher,
    /// The `/P` value.
    pub permissions: u32,
    /// How the document is encrypted, as "Standard V4 R4 128-bit AES".
    pub description: String,
    /// Whether the document's XMP metadata stream is encrypted.
    pub encrypt_metadata: bool,
    /// The encryption dictionary's object number; its strings are not
    /// encrypted.
    pub dictionary_object: Option<u32>,
}

/// Prepare decryption when the trailer names an encryption dictionary.
///
/// Does nothing for an unencrypted document or one already prepared.
pub(super) fn install(
    reader: &mut Reader,
    trailer: &Dictionary,
    password: &[u8],
) -> Result<(), DocumentError> {
    let Some(encrypt) = entry(trailer, "Encrypt") else {
        return Ok(());
    };
    if reader.has_decryptor() {
        return Ok(());
    }
    let dictionary_object = match encrypt {
        PdfObject::Reference(reference) => Some(reference.number),
        _ => None,
    };
    let PdfObject::Dictionary(dictionary) = reader.resolve(encrypt)? else {
        return Err(DocumentError::Structure(
            "the encryption dictionary is missing",
        ));
    };
    let id = match reader.resolve_entry(trailer, "ID")? {
        PdfObject::Array(parts) => match parts.into_iter().next() {
            Some(PdfObject::String(id)) => id,
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    let mut decryptor = Decryptor::new(&dictionary, &id, password)?;
    decryptor.dictionary_object = dictionary_object;
    reader.set_decryptor(decryptor);
    Ok(())
}

fn bytes<'a>(dictionary: &'a Dictionary, key: &str) -> &'a [u8] {
    match entry(dictionary, key) {
        Some(PdfObject::String(bytes)) => bytes,
        _ => &[],
    }
}

impl Decryptor {
    /// Check a password against an encryption dictionary and derive the key.
    ///
    /// The password is tried as the user password, then as the owner
    /// password. `id` is the first element of the trailer's `/ID`.
    fn new(encrypt: &Dictionary, id: &[u8], password: &[u8]) -> Result<Self, DocumentError> {
        const UNSUPPORTED: DocumentError = DocumentError::Unsupported("this encryption method");
        if entry(encrypt, "Filter").and_then(PdfObject::as_name) != Some(b"Standard") {
            return Err(UNSUPPORTED);
        }
        let integer = |key| entry(encrypt, key).and_then(PdfObject::as_integer);
        let version = integer("V").unwrap_or(0);
        let revision = integer("R").unwrap_or(0);
        let length_bits = size_entry(encrypt, "Length");

        let (strings, streams, key_length) = match version {
            1 => (Cipher::Rc4, Cipher::Rc4, 5),
            2 => (Cipher::Rc4, Cipher::Rc4, length_bits.unwrap_or(40) / 8),
            4 | 5 => {
                let filter = |key| crypt_filter(encrypt, entry(encrypt, key));
                let default = if version == 5 { 32 } else { 16 };
                let key_length = length_bits.map_or(default, |bits| bits / 8);
                (filter("StrF")?, filter("StmF")?, key_length)
            }
            _ => return Err(UNSUPPORTED),
        };
        let parameters = Parameters {
            revision,
            key_length: if version == 5 {
                32
            } else {
                key_length.clamp(5, 16)
            },
            owner: bytes(encrypt, "O"),
            user: bytes(encrypt, "U"),
            owner_key: bytes(encrypt, "OE"),
            user_key: bytes(encrypt, "UE"),
            // The value is a signed 32-bit integer read as its bit pattern.
            permissions: integer("P").unwrap_or(0) as u32,
            id,
            encrypt_metadata: entry(encrypt, "EncryptMetadata") != Some(&PdfObject::Boolean(false)),
        };
        let unlock = |password: &[u8]| match revision {
            2..=4 => Ok(legacy_key(&parameters, password)),
            5 | 6 => Ok(modern_key(&parameters, password)),
            _ => Err(UNSUPPORTED),
        };
        // A document with an empty user password opens for anyone, so a
        // password that does not match is no reason to refuse it.
        let key = match unlock(password)? {
            None if !password.is_empty() => unlock(b"")?,
            key => key,
        };
        let method = match streams {
            Cipher::Aes128 | Cipher::Aes256 => "AES",
            Cipher::Rc4 => "RC4",
            Cipher::Identity => "no cipher",
        };
        let bits = parameters.key_length * 8;
        Ok(Self {
            key: key.ok_or(if password.is_empty() {
                DocumentError::PasswordRequired
            } else {
                DocumentError::WrongPassword
            })?,
            strings,
            streams,
            permissions: parameters.permissions,
            description: format!("Standard V{version} R{revision} {bits}-bit {method}"),
            encrypt_metadata: parameters.encrypt_metadata,
            dictionary_object: None,
        })
    }

    /// New AES-256 protection (revision 6) for the given passwords, and
    /// the encryption dictionary that records it. An empty user password
    /// lets anyone open the document; the owner password, which should
    /// differ, lifts the permissions.
    pub fn protect(user: &str, owner: &str, permissions: Permissions) -> (Self, Dictionary) {
        // Passwords are UTF-8, at most 127 bytes; SASLprep is not applied.
        let user = &user.as_bytes()[..user.len().min(127)];
        let owner = &owner.as_bytes()[..owner.len().min(127)];
        let key = random_bytes(32);
        let salts = random_bytes(32);
        let (user_check, user_salt) = (&salts[0..8], &salts[8..16]);
        let (owner_check, owner_salt) = (&salts[16..24], &salts[24..32]);
        let wrap = |hash: [u8; 32]| cbc_encrypt(&hash, [0; 16], &key).unwrap_or_default();

        let mut user_entry = modern_hash(6, user, user_check, &[]).to_vec();
        user_entry.extend([user_check, user_salt].concat());
        let user_key = wrap(modern_hash(6, user, user_salt, &[]));
        let mut owner_entry = modern_hash(6, owner, owner_check, &user_entry).to_vec();
        owner_entry.extend([owner_check, owner_salt].concat());
        let owner_key = wrap(modern_hash(6, owner, owner_salt, &user_entry));

        let bits = permissions.bits();
        let mut perms = bits.to_le_bytes().to_vec();
        perms.extend([0xFF; 4]);
        perms.extend(b"Tadb");
        perms.extend(random_bytes(4));
        let perms = cbc_encrypt(&key, [0; 16], &perms).unwrap_or_default();

        let name = |text: &str| PdfObject::Name(text.as_bytes().to_vec());
        let mut filter = Dictionary::new();
        filter.insert(b"AuthEvent".to_vec(), name("DocOpen"));
        filter.insert(b"CFM".to_vec(), name("AESV3"));
        filter.insert(b"Length".to_vec(), PdfObject::Integer(32));
        let mut filters = Dictionary::new();
        filters.insert(b"StdCF".to_vec(), PdfObject::Dictionary(filter));
        let mut dictionary = Dictionary::new();
        let entries = [
            ("Filter", name("Standard")),
            ("V", PdfObject::Integer(5)),
            ("R", PdfObject::Integer(6)),
            ("Length", PdfObject::Integer(256)),
            ("CF", PdfObject::Dictionary(filters)),
            ("StmF", name("StdCF")),
            ("StrF", name("StdCF")),
            ("U", PdfObject::String(user_entry)),
            ("O", PdfObject::String(owner_entry)),
            ("UE", PdfObject::String(user_key)),
            ("OE", PdfObject::String(owner_key)),
            ("Perms", PdfObject::String(perms)),
            // The value is a signed 32-bit integer.
            ("P", PdfObject::Integer(i64::from(bits as i32))),
            ("EncryptMetadata", PdfObject::Boolean(true)),
        ];
        for (key, value) in entries {
            dictionary.insert(key.as_bytes().to_vec(), value);
        }
        let protection = Self {
            key,
            strings: Cipher::Aes256,
            streams: Cipher::Aes256,
            permissions: bits,
            description: "Standard V5 R6 256-bit AES".to_string(),
            encrypt_metadata: true,
            dictionary_object: None,
        };
        (protection, dictionary)
    }

    /// Decrypt a string that belongs to the given indirect object.
    pub fn decrypt_string(&self, number: u32, generation: u32, data: &[u8]) -> Vec<u8> {
        self.decrypt(self.strings, number, generation, data)
    }

    /// Decrypt the stored data of the stream with the given object number.
    pub fn decrypt_stream(&self, number: u32, generation: u32, data: &[u8]) -> Vec<u8> {
        self.decrypt(self.streams, number, generation, data)
    }

    /// Encrypt a string that belongs to the given indirect object.
    pub fn encrypt_string(&self, number: u32, generation: u32, data: &[u8]) -> Vec<u8> {
        self.encrypt(self.strings, number, generation, data)
    }

    /// Encrypt the stored data of the stream with the given object number.
    pub fn encrypt_stream(&self, number: u32, generation: u32, data: &[u8]) -> Vec<u8> {
        self.encrypt(self.streams, number, generation, data)
    }

    /// The key that encrypts one object's strings or stream.
    fn object_key(&self, cipher: Cipher, number: u32, generation: u32) -> Vec<u8> {
        match cipher {
            Cipher::Identity | Cipher::Aes256 => self.key.clone(),
            Cipher::Rc4 | Cipher::Aes128 => {
                // Each object has its own key: the file key extended with
                // the low bytes of its object and generation numbers.
                let mut material = self.key.clone();
                material.extend(&number.to_le_bytes()[..3]);
                material.extend(&generation.to_le_bytes()[..2]);
                if cipher == Cipher::Aes128 {
                    material.extend(b"sAlT");
                }
                md5(&material)[..(self.key.len() + 5).min(16)].to_vec()
            }
        }
    }

    fn encrypt(&self, cipher: Cipher, number: u32, generation: u32, data: &[u8]) -> Vec<u8> {
        let key = self.object_key(cipher, number, generation);
        match cipher {
            Cipher::Identity => data.to_vec(),
            Cipher::Rc4 => {
                let mut data = data.to_vec();
                rc4(&key, &mut data);
                data
            }
            Cipher::Aes128 | Cipher::Aes256 => {
                // A random initialization vector, then the data padded to
                // whole blocks with bytes that each hold the padding length.
                let mut iv = [0u8; 16];
                iv.copy_from_slice(&random_bytes(16));
                let padding = 16 - data.len() % 16;
                let mut padded = data.to_vec();
                padded.extend(std::iter::repeat_n(padding as u8, padding));
                let mut encrypted = iv.to_vec();
                encrypted.extend(cbc_encrypt(&key, iv, &padded).unwrap_or_default());
                encrypted
            }
        }
    }

    fn decrypt(&self, cipher: Cipher, number: u32, generation: u32, data: &[u8]) -> Vec<u8> {
        if cipher == Cipher::Identity {
            return data.to_vec();
        }
        let key = self.object_key(cipher, number, generation);
        if cipher == Cipher::Rc4 {
            let mut data = data.to_vec();
            rc4(&key, &mut data);
            return data;
        }
        // AES data is a 16-byte initialization vector, then whole blocks
        // that end with padding bytes each holding the padding length.
        let Some((iv, encrypted)) = data.split_first_chunk::<16>() else {
            return Vec::new();
        };
        let whole = encrypted.len() / 16 * 16;
        let mut plain = cbc_decrypt(&key, *iv, &encrypted[..whole]).unwrap_or_default();
        if let Some(&padding) = plain.last()
            && (1..=16).contains(&padding)
        {
            plain.truncate(plain.len() - usize::from(padding));
        }
        plain
    }
}

/// The cipher a named crypt filter selects.
fn crypt_filter(encrypt: &Dictionary, name: Option<&PdfObject>) -> Result<Cipher, DocumentError> {
    let Some(name) = name.and_then(PdfObject::as_name) else {
        return Ok(Cipher::Identity);
    };
    let method = entry(encrypt, "CF")
        .and_then(PdfObject::as_dictionary)
        .and_then(|filters| filters.get(name))
        .and_then(PdfObject::as_dictionary)
        .and_then(|filter| entry(filter, "CFM"))
        .and_then(PdfObject::as_name);
    match method {
        _ if name == b"Identity" => Ok(Cipher::Identity),
        None | Some(b"None") => Ok(Cipher::Identity),
        Some(b"V2") => Ok(Cipher::Rc4),
        Some(b"AESV2") => Ok(Cipher::Aes128),
        Some(b"AESV3") => Ok(Cipher::Aes256),
        Some(_) => Err(DocumentError::Unsupported("this encryption method")),
    }
}

/// The first 32 bytes of a password followed by the standard padding.
fn pad_password(password: &[u8]) -> [u8; 32] {
    let length = password.len().min(32);
    let mut padded = [0u8; 32];
    padded[..length].copy_from_slice(&password[..length]);
    padded[length..].copy_from_slice(&PASSWORD_PADDING[..32 - length]);
    padded
}

/// Revisions 2 through 4: the file key, if the password is right.
fn legacy_key(parameters: &Parameters<'_>, password: &[u8]) -> Option<Vec<u8>> {
    let from_user = |password: &[u8]| {
        let key = legacy_user_key(parameters, password);
        legacy_user_matches(parameters, &key).then_some(key)
    };
    from_user(password).or_else(|| {
        // The owner entry holds the user password, encrypted with a key
        // made from the owner password.
        let length = parameters.key_length;
        let mut digest = md5(&pad_password(password));
        if parameters.revision >= 3 {
            for _ in 0..50 {
                digest = md5(&digest);
            }
        }
        let owner_key = &digest[..length];
        let mut user_password = parameters.owner.get(..32)?.to_vec();
        if parameters.revision == 2 {
            rc4(owner_key, &mut user_password);
        } else {
            for round in (0..20u8).rev() {
                let key: Vec<u8> = owner_key.iter().map(|byte| byte ^ round).collect();
                rc4(&key, &mut user_password);
            }
        }
        from_user(&user_password)
    })
}

/// Revisions 2 through 4: the file key a user password would produce.
fn legacy_user_key(parameters: &Parameters<'_>, password: &[u8]) -> Vec<u8> {
    let mut material = pad_password(password).to_vec();
    material.extend(parameters.owner.iter().take(32));
    material.extend(parameters.permissions.to_le_bytes());
    material.extend(parameters.id);
    if parameters.revision >= 4 && !parameters.encrypt_metadata {
        material.extend([0xff; 4]);
    }
    let length = parameters.key_length;
    let mut digest = md5(&material);
    if parameters.revision >= 3 {
        for _ in 0..50 {
            digest = md5(&digest[..length]);
        }
    }
    digest[..length].to_vec()
}

/// Revisions 2 through 4: whether a file key reproduces the user entry.
fn legacy_user_matches(parameters: &Parameters<'_>, key: &[u8]) -> bool {
    if parameters.revision == 2 {
        let mut expected = PASSWORD_PADDING;
        rc4(key, &mut expected);
        return parameters.user.get(..32) == Some(&expected);
    }
    let mut material = PASSWORD_PADDING.to_vec();
    material.extend(parameters.id);
    let mut expected = md5(&material);
    for round in 0..20u8 {
        let key: Vec<u8> = key.iter().map(|byte| byte ^ round).collect();
        rc4(&key, &mut expected);
    }
    // Only the first 16 bytes are defined; the rest is arbitrary padding.
    parameters.user.get(..16) == Some(&expected)
}

/// Revisions 5 and 6: the file key, if the password is right.
///
/// The user and owner entries are a 32-byte hash, an 8-byte validation
/// salt, and an 8-byte key salt. The file key is stored encrypted under a
/// hash of the password and the key salt.
fn modern_key(parameters: &Parameters<'_>, password: &[u8]) -> Option<Vec<u8>> {
    let password = &password[..password.len().min(127)];
    let user = parameters.user.get(..48)?;
    let unlock = |entry: &[u8], encrypted_key: &[u8], user_data: &[u8]| {
        let hash = |salt: &[u8]| modern_hash(parameters.revision, password, salt, user_data);
        if hash(&entry[32..40])[..] != entry[..32] {
            return None;
        }
        cbc_decrypt(&hash(&entry[40..48]), [0; 16], encrypted_key.get(..32)?)
    };
    unlock(user, parameters.user_key, &[]).or_else(|| {
        let owner = parameters.owner.get(..48)?;
        unlock(owner, parameters.owner_key, user)
    })
}

/// The password hash of revision 5 (one SHA-256) or revision 6
/// (ISO 32000-2 algorithm 2.B).
fn modern_hash(revision: i64, password: &[u8], salt: &[u8], user_data: &[u8]) -> [u8; 32] {
    let mut digest = sha256(&[password, salt, user_data].concat()).to_vec();
    if revision >= 6 {
        let mut round = 0usize;
        loop {
            let block = [password, &digest, user_data].concat().repeat(64);
            let mut iv = [0u8; 16];
            iv.copy_from_slice(&digest[16..32]);
            let encrypted = cbc_encrypt(&digest[..16], iv, &block).unwrap_or_default();
            // The first 16 bytes, read as a number modulo 3, pick the hash.
            let selector: u32 = encrypted.iter().take(16).map(|&byte| u32::from(byte)).sum();
            digest = match selector % 3 {
                0 => sha256(&encrypted).to_vec(),
                1 => sha384(&encrypted).to_vec(),
                _ => sha512(&encrypted).to_vec(),
            };
            round += 1;
            let last = usize::from(encrypted.last().copied().unwrap_or(0));
            if round >= 64 && last + 32 <= round {
                break;
            }
        }
    }
    let mut result = [0u8; 32];
    result.copy_from_slice(&digest[..32]);
    result
}
