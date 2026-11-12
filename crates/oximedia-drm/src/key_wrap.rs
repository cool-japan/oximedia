// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! RFC 3394 AES-128 Key Wrap and Unwrap.
//!
//! This module implements the AES Key Wrap algorithm defined in
//! [RFC 3394](https://www.rfc-editor.org/rfc/rfc3394) and
//! NIST SP 800-38F.  The algorithm provides authenticated encryption
//! of a Content Encryption Key (`cek`) using a Key Encryption Key
//! (`kek`), producing a wrapped key that is 8 bytes longer than the
//! original.
//!
//! For AES-128 key wrap:
//! - Input: 16-byte `kek` + 16-byte `cek`
//! - Output: 24-byte wrapped key (2 × 8-byte ciphertext blocks + 8-byte
//!   integrity check value)
//!
//! Unwrapping verifies the integrity check value (`0xA6A6A6A6A6A6A6A6`)
//! and returns `None` if authentication fails (wrong KEK or tampered data).
//!
//! # Security
//!
//! This implementation is backed by the `aes` crate from the
//! [RustCrypto](https://github.com/RustCrypto) project, which provides
//! a constant-time, pure-Rust AES-128 block cipher with optional hardware
//! acceleration (AES-NI on x86/x86_64, ARMv8 Crypto Extensions).
//!
//! # Example
//!
//! ```rust
//! use oximedia_drm::key_wrap::KeyWrapper;
//!
//! let kek: [u8; 16] = [0xAB; 16];
//! let cek: [u8; 16] = [0x12; 16];
//!
//! let wrapped = KeyWrapper::wrap_aes128(&kek, &cek);
//! let unwrapped = KeyWrapper::unwrap_aes128(&kek, &wrapped);
//! assert_eq!(unwrapped, Some(cek));
//! ```

use aes::cipher::{BlockCipherDecrypt, BlockCipherEncrypt, KeyInit};
use aes::Aes128;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// RFC 3394 default initial value (Alternative Initial Value: 0xA6A6A6A6A6A6A6A6).
const RFC3394_IV: u64 = 0xA6A6A6A6A6A6A6A6;

/// Number of 64-bit plaintext blocks for a 128-bit CEK.
const N: usize = 2;

/// Number of wrap iterations (RFC 3394: always 6).
const WRAP_ROUNDS: u64 = 6;

// ---------------------------------------------------------------------------
// KeyWrapper
// ---------------------------------------------------------------------------

/// Stateless AES-128 key wrap / unwrap operations (RFC 3394).
pub struct KeyWrapper;

impl KeyWrapper {
    /// Wrap a 128-bit Content Encryption Key (`cek`) with a 128-bit
    /// Key Encryption Key (`kek`) using the RFC 3394 AES Key Wrap algorithm.
    ///
    /// # Arguments
    ///
    /// * `kek` — 16-byte Key Encryption Key (the master/wrapping key).
    /// * `cek` — 16-byte Content Encryption Key (the key to protect).
    ///
    /// # Returns
    ///
    /// A 24-byte wrapped key: 8-byte integrity check value followed by
    /// two 8-byte ciphertext blocks.
    #[must_use]
    pub fn wrap_aes128(kek: &[u8; 16], cek: &[u8; 16]) -> [u8; 24] {
        let cipher = Aes128::new_from_slice(kek).expect("KEK is exactly 16 bytes; infallible");

        // Split CEK into two 8-byte blocks: R[1] and R[2].
        let mut r: [[u8; 8]; N] = [[0u8; 8]; N];
        for (block_idx, chunk) in cek.chunks(8).enumerate() {
            r[block_idx].copy_from_slice(chunk);
        }

        // Integrity check register A, initialised to the RFC 3394 IV.
        let mut a: u64 = RFC3394_IV;

        // RFC 3394 wrap loop: j = 0..5, i = 1..n  (n=2)
        for j in 0..WRAP_ROUNDS {
            for i in 0..N {
                // B = AES_K(A || R[i])
                let b = aes_ecb_encrypt_block(&cipher, a, &r[i]);

                // A = MSB(64, B) XOR t   where t = n*j + (i+1) as u64
                let t: u64 = (N as u64) * j + (i as u64 + 1);
                a = u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) ^ t;

                // R[i] = LSB(64, B)
                r[i].copy_from_slice(&b[8..16]);
            }
        }

        // Output = A || R[1] || R[2]  (24 bytes total)
        let mut out = [0u8; 24];
        out[0..8].copy_from_slice(&a.to_be_bytes());
        out[8..16].copy_from_slice(&r[0]);
        out[16..24].copy_from_slice(&r[1]);
        out
    }

    /// Unwrap a previously wrapped 128-bit key using RFC 3394.
    ///
    /// Verifies the integrity check value; returns `None` if the KEK is
    /// wrong or the wrapped data has been tampered with.
    ///
    /// # Arguments
    ///
    /// * `kek`     — 16-byte Key Encryption Key (must match wrapping KEK).
    /// * `wrapped` — 24-byte wrapped key produced by [`wrap_aes128`](Self::wrap_aes128).
    ///
    /// # Returns
    ///
    /// `Some(cek)` if authentication succeeds, `None` otherwise.
    #[must_use]
    pub fn unwrap_aes128(kek: &[u8; 16], wrapped: &[u8; 24]) -> Option<[u8; 16]> {
        let cipher = Aes128::new_from_slice(kek).expect("KEK is exactly 16 bytes; infallible");

        // Decompose wrapped = C[0] || C[1] || C[2]  (each 8 bytes)
        let mut a: u64 = u64::from_be_bytes(wrapped[0..8].try_into().ok()?);
        let mut r: [[u8; 8]; N] = [[0u8; 8]; N];
        r[0].copy_from_slice(&wrapped[8..16]);
        r[1].copy_from_slice(&wrapped[16..24]);

        // RFC 3394 unwrap loop: j = 5 down to 0, i = n down to 1
        for j in (0..WRAP_ROUNDS).rev() {
            for i in (0..N).rev() {
                // t = n*j + (i+1) as u64
                let t: u64 = (N as u64) * j + (i as u64 + 1);

                // B = AES_K^-1((A XOR t) || R[i])
                let b = aes_ecb_decrypt_block(&cipher, a ^ t, &r[i]);

                // A = MSB(64, B)
                a = u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);

                // R[i] = LSB(64, B)
                r[i].copy_from_slice(&b[8..16]);
            }
        }

        // Verify integrity: A must equal the RFC 3394 default IV.
        if a != RFC3394_IV {
            return None;
        }

        // Reconstruct CEK = R[1] || R[2]
        let mut cek = [0u8; 16];
        cek[0..8].copy_from_slice(&r[0]);
        cek[8..16].copy_from_slice(&r[1]);
        Some(cek)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// AES-128 ECB encrypt a single 16-byte block formed by `(a_val || r_block)`.
///
/// `a_val`   — the current 64-bit integrity register A (big-endian).
/// `r_block` — the current 8-byte R[i] value.
///
/// Returns the 16-byte ciphertext.
#[inline]
fn aes_ecb_encrypt_block(cipher: &Aes128, a_val: u64, r_block: &[u8; 8]) -> [u8; 16] {
    let mut block_arr = [0u8; 16];
    block_arr[0..8].copy_from_slice(&a_val.to_be_bytes());
    block_arr[8..16].copy_from_slice(r_block);

    let mut block = aes::Block::from(block_arr);
    cipher.encrypt_block(&mut block);

    let mut out = [0u8; 16];
    out.copy_from_slice(block.as_slice());
    out
}

/// AES-128 ECB decrypt a single 16-byte block formed by `((a_val XOR t) || r_block)`.
///
/// `a_xor_t` — the XOR of the current A register and counter t (already computed).
/// `r_block`  — the current 8-byte R[i] value.
///
/// Returns the 16-byte plaintext.
#[inline]
fn aes_ecb_decrypt_block(cipher: &Aes128, a_xor_t: u64, r_block: &[u8; 8]) -> [u8; 16] {
    let mut block_arr = [0u8; 16];
    block_arr[0..8].copy_from_slice(&a_xor_t.to_be_bytes());
    block_arr[8..16].copy_from_slice(r_block);

    let mut block = aes::Block::from(block_arr);
    cipher.decrypt_block(&mut block);

    let mut out = [0u8; 16];
    out.copy_from_slice(block.as_slice());
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Basic correctness tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_wrap_unwrap_roundtrip() {
        let kek: [u8; 16] = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        let cek: [u8; 16] = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        let wrapped = KeyWrapper::wrap_aes128(&kek, &cek);
        let recovered = KeyWrapper::unwrap_aes128(&kek, &wrapped);
        assert_eq!(
            recovered,
            Some(cek),
            "unwrapped key must equal original cek"
        );
    }

    #[test]
    fn test_wrapped_differs_from_cek() {
        let kek: [u8; 16] = [0xAA; 16];
        let cek: [u8; 16] = [0x55; 16];
        let wrapped = KeyWrapper::wrap_aes128(&kek, &cek);
        // wrapped is 24 bytes; first 16 bytes must not equal the plaintext cek
        let wrapped_prefix: [u8; 16] = wrapped[0..16].try_into().expect("slice len 16");
        assert_ne!(
            wrapped_prefix, cek,
            "wrapped key must differ from plaintext cek"
        );
        // Also check the full wrapped value doesn't encode cek naively
        assert_ne!(&wrapped[0..16], &cek[..]);
    }

    #[test]
    fn test_different_keks_produce_different_wraps() {
        let kek_a: [u8; 16] = [0x11; 16];
        let kek_b: [u8; 16] = [0x22; 16];
        let cek: [u8; 16] = [0xFF; 16];
        let wrapped_a = KeyWrapper::wrap_aes128(&kek_a, &cek);
        let wrapped_b = KeyWrapper::wrap_aes128(&kek_b, &cek);
        assert_ne!(
            wrapped_a, wrapped_b,
            "different KEKs must produce different wraps"
        );
    }

    // -----------------------------------------------------------------------
    // RFC 3394 Test Vectors
    // -----------------------------------------------------------------------

    /// RFC 3394 §2.2.3 Test Vector 1 — Wrap 128 bits of Key Data with a 128-bit KEK.
    ///
    /// KEK      : 000102030405060708090A0B0C0D0E0F
    /// Key Data : 00112233445566778899AABBCCDDEEFF
    /// Ciphertext: 1FA68B0A8112B447AEF34BD8FB5A7B829D3E862371D2CFE5
    #[test]
    fn test_rfc3394_test_vector_1() {
        let kek: [u8; 16] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D,
            0x0E, 0x0F,
        ];
        let cek: [u8; 16] = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD,
            0xEE, 0xFF,
        ];
        let expected: [u8; 24] = [
            0x1F, 0xA6, 0x8B, 0x0A, 0x81, 0x12, 0xB4, 0x47, 0xAE, 0xF3, 0x4B, 0xD8, 0xFB, 0x5A,
            0x7B, 0x82, 0x9D, 0x3E, 0x86, 0x23, 0x71, 0xD2, 0xCF, 0xE5,
        ];

        let wrapped = KeyWrapper::wrap_aes128(&kek, &cek);
        assert_eq!(
            wrapped, expected,
            "wrap output must match RFC 3394 test vector 1"
        );

        // Verify round-trip via unwrap.
        let unwrapped = KeyWrapper::unwrap_aes128(&kek, &wrapped);
        assert_eq!(
            unwrapped,
            Some(cek),
            "unwrap of test-vector ciphertext must recover plaintext CEK"
        );
    }

    /// Unwrapping with a wrong KEK must return None (authentication failure).
    #[test]
    fn test_unwrap_wrong_kek_returns_none() {
        let kek_correct: [u8; 16] = [0xDE; 16];
        let kek_wrong: [u8; 16] = [0xAD; 16];
        let cek: [u8; 16] = [0xBE; 16];

        let wrapped = KeyWrapper::wrap_aes128(&kek_correct, &cek);
        let result = KeyWrapper::unwrap_aes128(&kek_wrong, &wrapped);
        assert_eq!(result, None, "wrong KEK must cause unwrap to return None");
    }

    /// Unwrapping a single-bit-flipped wrapped value must return None.
    #[test]
    fn test_unwrap_tampered_data_returns_none() {
        let kek: [u8; 16] = [0xCA; 16];
        let cek: [u8; 16] = [0xFE; 16];

        let mut wrapped = KeyWrapper::wrap_aes128(&kek, &cek);
        // Flip one bit in the ciphertext portion.
        wrapped[10] ^= 0x01;

        let result = KeyWrapper::unwrap_aes128(&kek, &wrapped);
        assert_eq!(
            result, None,
            "tampered data must cause unwrap to return None"
        );
    }
}
