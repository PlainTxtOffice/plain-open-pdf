//! AES (FIPS 197) with 128- and 256-bit keys, in cipher block chaining mode.

const BLOCK: usize = 16;

/// Multiply by x in GF(2^8) modulo the AES polynomial.
fn double(value: u8) -> u8 {
    (value << 1) ^ if value & 0x80 != 0 { 0x1b } else { 0 }
}

/// The substitution box and its inverse, computed from their definition:
/// the multiplicative inverse in GF(2^8) followed by an affine transform.
fn substitution_boxes() -> ([u8; 256], [u8; 256]) {
    let mut forward = [0u8; 256];
    let mut inverse = [0u8; 256];
    // `p` walks every nonzero element as powers of 3 while `q` walks the
    // inverses as powers of 1/3, so `q` is always the inverse of `p`.
    let (mut p, mut q) = (1u8, 1u8);
    loop {
        p ^= double(p);
        q ^= q << 1;
        q ^= q << 2;
        q ^= q << 4;
        if q & 0x80 != 0 {
            q ^= 0x09;
        }
        let transformed =
            q ^ q.rotate_left(1) ^ q.rotate_left(2) ^ q.rotate_left(3) ^ q.rotate_left(4) ^ 0x63;
        forward[usize::from(p)] = transformed;
        if p == 1 {
            break;
        }
    }
    forward[0] = 0x63;
    for (index, &value) in forward.iter().enumerate() {
        inverse[usize::from(value)] = index as u8;
    }
    (forward, inverse)
}

/// An expanded key. The state is 16 bytes in column order, as in the standard.
struct Aes {
    round_keys: Vec<[u8; BLOCK]>,
    forward: [u8; 256],
    inverse: [u8; 256],
}

impl Aes {
    /// Expand a 16- or 32-byte key; other lengths are not used by PDF.
    fn new(key: &[u8]) -> Option<Self> {
        if key.len() != 16 && key.len() != 32 {
            return None;
        }
        let (forward, inverse) = substitution_boxes();
        let key_words = key.len() / 4;
        let rounds = key_words + 6;
        let mut words = key.as_chunks::<4>().0.to_vec();
        let mut round_constant = 1u8;
        for index in key_words..4 * (rounds + 1) {
            let mut word = words[index - 1];
            if index % key_words == 0 {
                word.rotate_left(1);
                word = word.map(|byte| forward[usize::from(byte)]);
                word[0] ^= round_constant;
                round_constant = double(round_constant);
            } else if key_words > 6 && index % key_words == 4 {
                word = word.map(|byte| forward[usize::from(byte)]);
            }
            let earlier = words[index - key_words];
            words.push(std::array::from_fn(|byte| earlier[byte] ^ word[byte]));
        }
        let round_keys = words
            .as_chunks::<4>()
            .0
            .iter()
            .map(|round| std::array::from_fn(|index| round[index / 4][index % 4]))
            .collect();
        Some(Self {
            round_keys,
            forward,
            inverse,
        })
    }

    fn add_round_key(&self, state: &mut [u8; BLOCK], round: usize) {
        for (byte, key) in state.iter_mut().zip(self.round_keys[round]) {
            *byte ^= key;
        }
    }

    fn encrypt_block(&self, state: &mut [u8; BLOCK]) {
        let last = self.round_keys.len() - 1;
        self.add_round_key(state, 0);
        for round in 1..=last {
            // Substitute bytes and shift row `r` left by `r` columns.
            let previous = *state;
            for column in 0..4 {
                for row in 0..4 {
                    let source = previous[(column + row) % 4 * 4 + row];
                    state[column * 4 + row] = self.forward[usize::from(source)];
                }
            }
            if round != last {
                mix_columns(state);
            }
            self.add_round_key(state, round);
        }
    }

    fn decrypt_block(&self, state: &mut [u8; BLOCK]) {
        let last = self.round_keys.len() - 1;
        self.add_round_key(state, last);
        for round in (0..last).rev() {
            // Shift row `r` right by `r` columns and undo the substitution.
            let previous = *state;
            for column in 0..4 {
                for row in 0..4 {
                    let source = previous[column * 4 + row];
                    state[(column + row) % 4 * 4 + row] = self.inverse[usize::from(source)];
                }
            }
            self.add_round_key(state, round);
            if round != 0 {
                unmix_columns(state);
            }
        }
    }
}

/// Multiply each column by the matrix whose rows rotate (2, 3, 1, 1).
///
/// With `t` the sum of the column, 2a + 3b + c + d equals a + t + 2(a + b).
fn mix_columns(state: &mut [u8; BLOCK]) {
    for column in state.as_chunks_mut::<4>().0 {
        let [a, b, c, d] = *column;
        let total = a ^ b ^ c ^ d;
        *column = [
            a ^ total ^ double(a ^ b),
            b ^ total ^ double(b ^ c),
            c ^ total ^ double(c ^ d),
            d ^ total ^ double(d ^ a),
        ];
    }
}

/// Undo [`mix_columns`]: the inverse matrix, rows rotating (14, 11, 13, 9),
/// is the forward matrix times one whose rows rotate (5, 0, 4, 0).
fn unmix_columns(state: &mut [u8; BLOCK]) {
    for column in state.as_chunks_mut::<4>().0 {
        let [a, b, c, d] = *column;
        let even = double(double(a ^ c));
        let odd = double(double(b ^ d));
        *column = [a ^ even, b ^ odd, c ^ even, d ^ odd];
    }
    mix_columns(state);
}

/// Decrypt whole blocks in CBC mode; padding is left for the caller.
///
/// Returns `None` for a key that is not 16 or 32 bytes or data that is not a
/// whole number of blocks.
pub(crate) fn cbc_decrypt(key: &[u8], iv: [u8; BLOCK], data: &[u8]) -> Option<Vec<u8>> {
    let cipher = Aes::new(key)?;
    if !data.len().is_multiple_of(BLOCK) {
        return None;
    }
    let mut output = Vec::with_capacity(data.len());
    let mut chain = iv;
    for encrypted in data.as_chunks::<BLOCK>().0 {
        let mut block = *encrypted;
        let next_chain = block;
        cipher.decrypt_block(&mut block);
        output.extend(block.iter().zip(chain).map(|(byte, mask)| byte ^ mask));
        chain = next_chain;
    }
    Some(output)
}

/// Encrypt whole blocks in CBC mode without padding.
///
/// Returns `None` for a key that is not 16 or 32 bytes or data that is not a
/// whole number of blocks.
pub(crate) fn cbc_encrypt(key: &[u8], iv: [u8; BLOCK], data: &[u8]) -> Option<Vec<u8>> {
    let cipher = Aes::new(key)?;
    if !data.len().is_multiple_of(BLOCK) {
        return None;
    }
    let mut output = Vec::with_capacity(data.len());
    let mut chain = iv;
    for plain in data.as_chunks::<BLOCK>().0 {
        for (byte, &value) in chain.iter_mut().zip(plain) {
            *byte ^= value;
        }
        cipher.encrypt_block(&mut chain);
        output.extend(chain);
    }
    Some(output)
}
