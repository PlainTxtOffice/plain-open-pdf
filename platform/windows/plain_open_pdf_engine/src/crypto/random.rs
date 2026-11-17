//! Unpredictable bytes for encryption keys, salts, and initialization
//! vectors.
//!
//! The standard library has no random number interface, but it keys the
//! hasher of each `RandomState` with bytes from the operating system's
//! random source. SipHash under such a secret key is a pseudorandom
//! function, so hashing a counter with it gives bytes an attacker cannot
//! predict; SHA-256 then mixes several of those outputs into each block.

use super::sha256;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Counts the blocks made, so that no two hash the same input.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// `count` unpredictable bytes.
pub(crate) fn random_bytes(count: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(count + 32);
    while bytes.len() < count {
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let mut seed = Vec::with_capacity(64);
        for _ in 0..8 {
            let mut hasher = RandomState::new().build_hasher();
            hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
            hasher.write_u128(time);
            seed.extend(hasher.finish().to_le_bytes());
        }
        bytes.extend(sha256(&seed));
    }
    bytes.truncate(count);
    bytes
}
