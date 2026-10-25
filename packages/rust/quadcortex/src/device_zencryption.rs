//! Cortex Control-compatible connection-scoped Zencryption primitives.
//!
//! These mechanics are deliberately private to the transport runtime. They do
//! not authorize or expose the Cloud/Login/Forward/Logs/License operations
//! which use encrypted framing in the reference client.

use aes::Aes128;
use ctr::cipher::{KeyIvInit, StreamCipher};
use sha1::{Digest, Sha1};
use zeroize::{Zeroize, Zeroizing};

const XXTEA_KEY: [u32; 4] = [0x83d2_0984, 0xb360_21be, 0x9c22_63bc, 0x4bb2_2d16];
const XXTEA_DELTA: u32 = 0x9e37_79b9;

type Aes128Ctr = ctr::Ctr128BE<Aes128>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ZencryptionError {
    MissingSecret,
}

/// Per-connection secret accumulation. `Vec<u8>::zeroize` overwrites capacity,
/// not merely the initialized length, before releasing the allocation.
#[derive(Default)]
pub(crate) struct ZencryptionState {
    secret: Vec<u8>,
}

impl std::fmt::Debug for ZencryptionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZencryptionState")
            .field("secret_installed", &!self.secret.is_empty())
            .finish_non_exhaustive()
    }
}

impl Drop for ZencryptionState {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

impl ZencryptionState {
    pub(crate) fn clear(&mut self) {
        self.secret.zeroize();
        self.secret.clear();
    }

    pub(crate) fn install_wrapped_fragment(&mut self, wrapped: &[u8]) -> bool {
        let Some(fragment) = unwrap_secret(wrapped) else {
            // A bad terminal length is an exact Cortex no-op. Inputs outside
            // the decoder's statically proven structural preconditions also
            // fail closed as a host safety policy: never mutate existing key
            // material from an unparseable fragment.
            return false;
        };
        self.secret.extend_from_slice(&fragment);
        true
    }

    pub(crate) fn is_installed(&self) -> bool {
        !self.secret.is_empty()
    }

    pub(crate) fn transform(&self, bytes: &[u8]) -> Result<Vec<u8>, ZencryptionError> {
        if self.secret.is_empty() {
            return Err(ZencryptionError::MissingSecret);
        }
        let (key, iv) = evp_bytes_to_key_sha1_count_10(&self.secret);
        let mut output = bytes.to_vec();
        let mut cipher = Aes128Ctr::new((&*key).into(), (&*iv).into());
        cipher.apply_keystream(&mut output);
        Ok(output)
    }

    #[cfg(test)]
    fn secret(&self) -> &[u8] {
        &self.secret
    }
}

/// The six message families initialized into Cortex Control's global
/// Zencryption applicability set.
pub(crate) fn applies_to(message_type: u16) -> bool {
    matches!(message_type, 18 | 29 | 30 | 31 | 41 | 58)
}

fn unwrap_secret(wrapped: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if wrapped.len() < 8 || !wrapped.len().is_multiple_of(4) {
        return None;
    }
    let mut words = Zeroizing::new(
        wrapped
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect::<Vec<_>>(),
    );
    let n = words.len();
    let rounds = 6_u32 + 52_u32 / n as u32;
    let mut sum = rounds.wrapping_mul(XXTEA_DELTA);
    let mut y = words[0];
    while sum != 0 {
        let e = (sum >> 2) & 3;
        for p in (1..n).rev() {
            let z = words[p - 1];
            words[p] = words[p].wrapping_sub(mx(sum, y, z, p, e));
            y = words[p];
        }
        let z = words[n - 1];
        words[0] = words[0].wrapping_sub(mx(sum, y, z, 0, e));
        y = words[0];
        sum = sum.wrapping_sub(XXTEA_DELTA);
    }

    let word_bytes = n * 4;
    let plaintext_bytes = words[n - 1] as usize;
    if !(word_bytes.saturating_sub(7)..=word_bytes - 4).contains(&plaintext_bytes) {
        return None;
    }
    let mut decoded = Zeroizing::new(Vec::with_capacity(word_bytes));
    for word in words.iter() {
        decoded.extend_from_slice(&word.to_le_bytes());
    }
    decoded.truncate(plaintext_bytes);
    Some(decoded)
}

#[inline]
fn mx(sum: u32, y: u32, z: u32, p: usize, e: u32) -> u32 {
    ((z >> 5 ^ y << 2).wrapping_add(y >> 3 ^ z << 4))
        ^ ((sum ^ y).wrapping_add(XXTEA_KEY[((p as u32 & 3) ^ e) as usize] ^ z))
}

fn evp_bytes_to_key_sha1_count_10(secret: &[u8]) -> (Zeroizing<[u8; 16]>, Zeroizing<[u8; 16]>) {
    let mut derived = Zeroizing::new([0_u8; 40]);
    let mut previous = Zeroizing::new(Vec::<u8>::new());
    let mut offset = 0;
    while offset < 32 {
        let mut digest = Sha1::new();
        digest.update(previous.as_slice());
        digest.update(secret);
        let mut block = Zeroizing::new(digest.finalize().to_vec());
        for _ in 1..10 {
            block = Zeroizing::new(Sha1::digest(block.as_slice()).to_vec());
        }
        let count = block.len().min(derived.len() - offset);
        derived[offset..offset + count].copy_from_slice(&block[..count]);
        offset += count;
        previous = block;
    }
    let mut key = Zeroizing::new([0_u8; 16]);
    let mut iv = Zeroizing::new([0_u8; 16]);
    key.copy_from_slice(&derived[..16]);
    iv.copy_from_slice(&derived[16..32]);
    (key, iv)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    #[test]
    fn fixed_xxtea_vector_unwraps_and_fragments_append() {
        let mut state = ZencryptionState::default();
        assert!(state.install_wrapped_fragment(&hex("637aa22b4bc21e69435ef11d01d129b837f56105")));
        assert_eq!(state.secret(), b"cortex-secret");
    }

    #[test]
    fn malformed_fragment_preserves_existing_secret() {
        let mut state = ZencryptionState::default();
        state.secret.extend_from_slice(b"already-installed");
        assert!(!state.install_wrapped_fragment(&[1, 2, 3]));
        assert_eq!(state.secret(), b"already-installed");
    }

    #[test]
    fn kdf_and_aes_ctr_match_fixed_reference_vector() {
        let mut state = ZencryptionState::default();
        state.secret.extend_from_slice(b"cortex-secret");
        assert_eq!(
            state.transform(b"Quad Cortex encrypted payload").unwrap(),
            hex("a175e56df2744eebbc1e93ef4c2ee481d2cc15a6d93c00a9cc502f43e8")
        );
    }

    #[test]
    fn ctr_transform_is_its_own_inverse_across_bounded_lengths() {
        let mut value = 0x43_u8;
        for length in 0..=4096_usize {
            let bytes = (0..length)
                .map(|_| {
                    value = value.wrapping_mul(17).wrapping_add(31);
                    value
                })
                .collect::<Vec<_>>();
            let mut state = ZencryptionState::default();
            state.secret.extend_from_slice(b"property-secret");
            let encrypted = state.transform(&bytes).unwrap();
            assert_eq!(state.transform(&encrypted).unwrap(), bytes);
        }
    }

    #[test]
    fn applicability_is_exact_and_closed() {
        let observed = (0_u16..=64)
            .filter(|value| applies_to(*value))
            .collect::<Vec<_>>();
        assert_eq!(observed, [18, 29, 30, 31, 41, 58]);
    }
}
