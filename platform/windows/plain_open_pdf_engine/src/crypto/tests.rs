//! Published test vectors, plus digests of a multi-block message computed
//! with an independent implementation.

use super::{cbc_decrypt, cbc_encrypt, md5, rc4, sha256, sha384, sha512};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

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

/// Bytes 0 through 199: longer than one block of every hash here.
fn long_message() -> Vec<u8> {
    (0..200).collect()
}

#[test]
fn md5_matches_rfc_1321() {
    assert_eq!(hex(&md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
    assert_eq!(hex(&md5(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(
        hex(&md5(&long_message())),
        "fb7001d34b8e82c9b579be5005d5b0a5"
    );
}

#[test]
fn sha2_matches_fips_180() {
    assert_eq!(
        hex(&sha256(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        hex(&sha256(&long_message())),
        "1901da1c9f699b48f6b2636e65cbf73abf99d0441ef67f5c540a42f7051dec6f"
    );
    assert_eq!(
        hex(&sha384(b"abc")),
        "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed\
         8086072ba1e7cc2358baeca134c825a7"
    );
    assert_eq!(
        hex(&sha384(&long_message())),
        "7ea4bb2534c67036f49de7beb5fe8a2478df04ff3fef40a9cd4923999a590e99\
         12df1297217ce1a021aa2fb1013498b8"
    );
    assert_eq!(
        hex(&sha512(b"abc")),
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
         2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
    );
    assert_eq!(
        hex(&sha512(&long_message())),
        "986058e9895e2c2ab8f9e8cbdf801db12a44842a56a91d5a4e87b1fc98b29372\
         2c4664142e42c3c551ff898646268cd92b84ed230b8c94bed7798d4f27cd7465"
    );
}

#[test]
fn rc4_matches_known_keystream_and_inverts_itself() {
    let mut data = b"Plaintext".to_vec();
    rc4(b"Key", &mut data);
    assert_eq!(hex(&data), "bbf316e8d940af0ad3");
    rc4(b"Key", &mut data);
    assert_eq!(data, b"Plaintext");
}

#[test]
fn aes_matches_fips_197_examples() {
    let plain = unhex("00112233445566778899aabbccddeeff");
    let zero = [0u8; 16];
    for (key, expected) in [
        (
            "000102030405060708090a0b0c0d0e0f",
            "69c4e0d86a7b0430d8cdb78070b4c55a",
        ),
        (
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
            "8ea2b7ca516745bfeafc49904b496089",
        ),
    ] {
        let key = unhex(key);
        let encrypted = cbc_encrypt(&key, zero, &plain).unwrap();
        assert_eq!(hex(&encrypted), expected);
        assert_eq!(cbc_decrypt(&key, zero, &encrypted).unwrap(), plain);
    }
}

#[test]
fn cbc_chains_blocks_and_rejects_bad_sizes() {
    // NIST SP 800-38A, CBC-AES128, first two blocks.
    let key = unhex("2b7e151628aed2a6abf7158809cf4f3c");
    let iv: [u8; 16] = std::array::from_fn(|index| index as u8);
    let plain = unhex("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51");
    let encrypted = cbc_encrypt(&key, iv, &plain).unwrap();
    assert_eq!(
        hex(&encrypted),
        "7649abac8119b246cee98e9b12e9197d5086cb9b507219ee95db113a917678b2"
    );
    assert_eq!(cbc_decrypt(&key, iv, &encrypted).unwrap(), plain);

    assert_eq!(cbc_decrypt(&key, iv, &encrypted[..17]), None);
    assert_eq!(cbc_encrypt(&key[..15], iv, &plain), None);
    assert_eq!(cbc_decrypt(&[0; 24], iv, &encrypted), None);
}
