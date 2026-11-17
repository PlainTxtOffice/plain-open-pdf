//! RC4, the stream cipher of PDF encryption before AES.

/// Encrypt or decrypt in place; the operation is its own inverse.
///
/// An empty key leaves the data unchanged.
pub(crate) fn rc4(key: &[u8], data: &mut [u8]) {
    if key.is_empty() {
        return;
    }
    let mut state: [u8; 256] = std::array::from_fn(|index| index as u8);
    let mut j = 0u8;
    for i in 0..256 {
        j = j.wrapping_add(state[i]).wrapping_add(key[i % key.len()]);
        state.swap(i, usize::from(j));
    }
    let (mut i, mut j) = (0u8, 0u8);
    for byte in data {
        i = i.wrapping_add(1);
        j = j.wrapping_add(state[usize::from(i)]);
        state.swap(usize::from(i), usize::from(j));
        let index = state[usize::from(i)].wrapping_add(state[usize::from(j)]);
        *byte ^= state[usize::from(index)];
    }
}
