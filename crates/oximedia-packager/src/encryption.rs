//! Encryption support for adaptive streaming.

use crate::config::EncryptionMethod;
use crate::error::{PackagerError, PackagerResult};
use bytes::{BufMut, BytesMut};
use rand::TryRng;

#[cfg(feature = "encryption")]
use aes::cipher::{
    BlockCipherDecrypt, BlockCipherEncrypt, BlockModeDecrypt, BlockModeEncrypt, KeyInit, KeyIvInit,
};
#[cfg(feature = "encryption")]
use aes::{Aes128, Aes128Dec};
#[cfg(feature = "encryption")]
use cbc::{Decryptor, Encryptor};

/// Encryption key information.
#[derive(Debug, Clone)]
pub struct KeyInfo {
    /// Encryption key (16 bytes for AES-128).
    pub key: Vec<u8>,
    /// Initialization vector.
    pub iv: Vec<u8>,
    /// Key URI (for HLS).
    pub uri: Option<String>,
    /// Key format (for HLS).
    pub format: Option<String>,
    /// Key format versions.
    pub format_versions: Option<String>,
}

impl KeyInfo {
    /// Create new key info.
    #[must_use]
    pub fn new(key: Vec<u8>, iv: Vec<u8>) -> Self {
        Self {
            key,
            iv,
            uri: None,
            format: None,
            format_versions: None,
        }
    }

    /// Set the key URI.
    #[must_use]
    pub fn with_uri(mut self, uri: String) -> Self {
        self.uri = Some(uri);
        self
    }

    /// Set the key format.
    #[must_use]
    pub fn with_format(mut self, format: String) -> Self {
        self.format = Some(format);
        self
    }

    /// Validate key info.
    pub fn validate(&self) -> PackagerResult<()> {
        if self.key.len() != 16 {
            return Err(PackagerError::EncryptionError(
                "Key must be 16 bytes for AES-128".to_string(),
            ));
        }

        if self.iv.len() != 16 {
            return Err(PackagerError::EncryptionError(
                "IV must be 16 bytes".to_string(),
            ));
        }

        Ok(())
    }
}

/// Encryption handler.
pub struct EncryptionHandler {
    method: EncryptionMethod,
    key_info: Option<KeyInfo>,
}

impl EncryptionHandler {
    /// Create a new encryption handler.
    #[must_use]
    pub fn new(method: EncryptionMethod) -> Self {
        Self {
            method,
            key_info: None,
        }
    }

    /// Set key information.
    pub fn set_key_info(&mut self, key_info: KeyInfo) -> PackagerResult<()> {
        key_info.validate()?;
        self.key_info = Some(key_info);
        Ok(())
    }

    /// Check if encryption is enabled.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.method != EncryptionMethod::None
    }

    /// Get encryption method.
    #[must_use]
    pub fn method(&self) -> EncryptionMethod {
        self.method
    }

    /// Encrypt data.
    pub fn encrypt(&self, data: &[u8]) -> PackagerResult<Vec<u8>> {
        if !self.is_enabled() {
            return Ok(data.to_vec());
        }

        match self.method {
            EncryptionMethod::None => Ok(data.to_vec()),
            EncryptionMethod::Aes128 => self.encrypt_aes128(data),
            EncryptionMethod::SampleAes => self.encrypt_sample_aes(data),
            EncryptionMethod::Cenc => self.encrypt_cenc(data),
        }
    }

    /// Decrypt data.
    pub fn decrypt(&self, data: &[u8]) -> PackagerResult<Vec<u8>> {
        if !self.is_enabled() {
            return Ok(data.to_vec());
        }

        match self.method {
            EncryptionMethod::None => Ok(data.to_vec()),
            EncryptionMethod::Aes128 => self.decrypt_aes128(data),
            EncryptionMethod::SampleAes => self.decrypt_sample_aes(data),
            EncryptionMethod::Cenc => self.decrypt_cenc(data),
        }
    }

    /// Encrypt with AES-128 CBC.
    #[cfg(feature = "encryption")]
    fn encrypt_aes128(&self, data: &[u8]) -> PackagerResult<Vec<u8>> {
        let key_info = self
            .key_info
            .as_ref()
            .ok_or_else(|| PackagerError::EncryptionError("Key info not set".to_string()))?;

        type Aes128CbcEnc = Encryptor<Aes128>;

        let cipher = Aes128CbcEnc::new_from_slices(&key_info.key, &key_info.iv)
            .map_err(|e| PackagerError::EncryptionError(format!("Failed to create cipher: {e}")))?;

        // Allocate buffer with space for PKCS7 padding (at most one extra block)
        let msg_len = data.len();
        let mut buf = vec![0u8; msg_len + 16];
        buf[..msg_len].copy_from_slice(data);

        let encrypted = cipher
            .encrypt_padded::<block_padding::Pkcs7>(&mut buf, msg_len)
            .map_err(|e| PackagerError::EncryptionError(format!("Encryption failed: {e}")))?;

        Ok(encrypted.to_vec())
    }

    /// Encrypt with AES-128 CBC (when encryption feature is disabled).
    #[cfg(not(feature = "encryption"))]
    fn encrypt_aes128(&self, _data: &[u8]) -> PackagerResult<Vec<u8>> {
        Err(PackagerError::EncryptionError(
            "Encryption feature not enabled".to_string(),
        ))
    }

    /// Decrypt with AES-128 CBC.
    #[cfg(feature = "encryption")]
    fn decrypt_aes128(&self, data: &[u8]) -> PackagerResult<Vec<u8>> {
        let key_info = self
            .key_info
            .as_ref()
            .ok_or_else(|| PackagerError::EncryptionError("Key info not set".to_string()))?;

        type Aes128CbcDec = Decryptor<Aes128>;

        let cipher = Aes128CbcDec::new_from_slices(&key_info.key, &key_info.iv)
            .map_err(|e| PackagerError::EncryptionError(format!("Failed to create cipher: {e}")))?;

        let mut buf = data.to_vec();
        let decrypted = cipher
            .decrypt_padded::<block_padding::Pkcs7>(&mut buf)
            .map_err(|e| PackagerError::EncryptionError(format!("Decryption failed: {e}")))?;

        Ok(decrypted.to_vec())
    }

    /// Decrypt with AES-128 CBC (when encryption feature is disabled).
    #[cfg(not(feature = "encryption"))]
    fn decrypt_aes128(&self, _data: &[u8]) -> PackagerResult<Vec<u8>> {
        Err(PackagerError::EncryptionError(
            "Encryption feature not enabled".to_string(),
        ))
    }

    /// Encrypt with SAMPLE-AES (HLS), i.e. the CENC `cbcs` pattern-encryption
    /// scheme (ISO/IEC 23001-7 §9.6): AES-128-CBC applied over a repeating
    /// 1-crypt / 9-skip 16-byte block pattern. Only every 10th block is
    /// encrypted; the intervening blocks and any trailing partial (< 16-byte)
    /// block are left in the clear. The CBC chain spans only the encrypted
    /// blocks (the per-sample IV seeds the first, each ciphertext block seeds
    /// the next). This is what FairPlay / Shaka / hls.js expect; full-buffer
    /// CBC (the previous behaviour) could not be decrypted by any of them.
    ///
    /// TODO(0.2.x): NAL-unit-aware subsample mapping (a clear leader per NAL)
    /// needs a bitstream parser; this applies the pattern over the whole
    /// sample buffer, which is correct for already-elementary media payloads.
    #[cfg(feature = "encryption")]
    fn encrypt_sample_aes(&self, data: &[u8]) -> PackagerResult<Vec<u8>> {
        let key_info = self
            .key_info
            .as_ref()
            .ok_or_else(|| PackagerError::EncryptionError("Key info not set".to_string()))?;
        sample_aes_cbcs_encrypt(&key_info.key, &key_info.iv, data)
    }

    /// SAMPLE-AES encryption stub when the `encryption` feature is disabled.
    #[cfg(not(feature = "encryption"))]
    fn encrypt_sample_aes(&self, _data: &[u8]) -> PackagerResult<Vec<u8>> {
        Err(PackagerError::EncryptionError(
            "Encryption feature not enabled".to_string(),
        ))
    }

    /// Decrypt SAMPLE-AES (`cbcs` pattern) — exact inverse of
    /// [`Self::encrypt_sample_aes`].
    #[cfg(feature = "encryption")]
    fn decrypt_sample_aes(&self, data: &[u8]) -> PackagerResult<Vec<u8>> {
        let key_info = self
            .key_info
            .as_ref()
            .ok_or_else(|| PackagerError::EncryptionError("Key info not set".to_string()))?;
        sample_aes_cbcs_decrypt(&key_info.key, &key_info.iv, data)
    }

    /// SAMPLE-AES decryption stub when the `encryption` feature is disabled.
    #[cfg(not(feature = "encryption"))]
    fn decrypt_sample_aes(&self, _data: &[u8]) -> PackagerResult<Vec<u8>> {
        Err(PackagerError::EncryptionError(
            "Encryption feature not enabled".to_string(),
        ))
    }

    /// Encrypts `data`, routing pattern-based schemes (`SampleAes`, i.e. the
    /// CENC `cbcs` scheme) through NAL-unit-aware subsample mapping when
    /// `codec` is NAL-structured (see [`codec_structure_for`]); every other
    /// method/codec combination behaves exactly like [`Self::encrypt`], which
    /// remains available directly for callers that don't have a codec name at
    /// hand.
    ///
    /// # Errors
    /// See [`Self::encrypt`]. For `SampleAes` with a NAL-structured `codec`,
    /// additionally returns [`PackagerError::EncryptionError`] if `data` is
    /// not validly length-prefixed (see [`nal_subsamples`]).
    pub fn encrypt_for_codec(&self, data: &[u8], codec: &str) -> PackagerResult<Vec<u8>> {
        if !self.is_enabled() || self.method != EncryptionMethod::SampleAes {
            return self.encrypt(data);
        }
        match codec_structure_for(codec) {
            CodecStructure::Elementary => self.encrypt(data),
            CodecStructure::NalLengthPrefixed {
                length_size,
                header_style,
            } => self.encrypt_sample_aes_nal(data, length_size, header_style),
        }
    }

    /// Inverse of [`Self::encrypt_for_codec`].
    ///
    /// # Errors
    /// See [`Self::encrypt_for_codec`].
    pub fn decrypt_for_codec(&self, data: &[u8], codec: &str) -> PackagerResult<Vec<u8>> {
        if !self.is_enabled() || self.method != EncryptionMethod::SampleAes {
            return self.decrypt(data);
        }
        match codec_structure_for(codec) {
            CodecStructure::Elementary => self.decrypt(data),
            CodecStructure::NalLengthPrefixed {
                length_size,
                header_style,
            } => self.decrypt_sample_aes_nal(data, length_size, header_style),
        }
    }

    /// NAL-unit-aware `cbcs` encryption — see [`nal_subsamples`] and
    /// [`sample_aes_cbcs_encrypt_subsamples`].
    #[cfg(feature = "encryption")]
    fn encrypt_sample_aes_nal(
        &self,
        data: &[u8],
        length_size: u8,
        header_style: NalHeaderStyle,
    ) -> PackagerResult<Vec<u8>> {
        let key_info = self
            .key_info
            .as_ref()
            .ok_or_else(|| PackagerError::EncryptionError("Key info not set".to_string()))?;
        let subsamples = nal_subsamples(data, length_size, header_style)?;
        ensure_nal_sample_has_protected_data(data, &subsamples)?;
        sample_aes_cbcs_encrypt_subsamples(&key_info.key, &key_info.iv, data, &subsamples)
    }

    /// NAL-aware SAMPLE-AES encryption stub when the `encryption` feature is
    /// disabled.
    #[cfg(not(feature = "encryption"))]
    fn encrypt_sample_aes_nal(
        &self,
        _data: &[u8],
        _length_size: u8,
        _header_style: NalHeaderStyle,
    ) -> PackagerResult<Vec<u8>> {
        Err(PackagerError::EncryptionError(
            "Encryption feature not enabled".to_string(),
        ))
    }

    /// Inverse of [`Self::encrypt_sample_aes_nal`].
    #[cfg(feature = "encryption")]
    fn decrypt_sample_aes_nal(
        &self,
        data: &[u8],
        length_size: u8,
        header_style: NalHeaderStyle,
    ) -> PackagerResult<Vec<u8>> {
        let key_info = self
            .key_info
            .as_ref()
            .ok_or_else(|| PackagerError::EncryptionError("Key info not set".to_string()))?;
        let subsamples = nal_subsamples(data, length_size, header_style)?;
        ensure_nal_sample_has_protected_data(data, &subsamples)?;
        sample_aes_cbcs_decrypt_subsamples(&key_info.key, &key_info.iv, data, &subsamples)
    }

    /// NAL-aware SAMPLE-AES decryption stub when the `encryption` feature is
    /// disabled.
    #[cfg(not(feature = "encryption"))]
    fn decrypt_sample_aes_nal(
        &self,
        _data: &[u8],
        _length_size: u8,
        _header_style: NalHeaderStyle,
    ) -> PackagerResult<Vec<u8>> {
        Err(PackagerError::EncryptionError(
            "Encryption feature not enabled".to_string(),
        ))
    }

    /// Encrypt with Common Encryption `cenc` (ISO/IEC 23001-7): full-sample
    /// AES-128 in CTR mode. The 16-byte IV is used as the initial 128-bit
    /// big-endian counter block, incremented once per 16-byte block. CTR is the
    /// spec-correct cipher for full-sample `cenc` and is what Widevine /
    /// PlayReady / dash.js / Shaka expect; the previous full-buffer CBC could
    /// not be decrypted by any CENC client.
    #[cfg(feature = "encryption")]
    fn encrypt_cenc(&self, data: &[u8]) -> PackagerResult<Vec<u8>> {
        let key_info = self
            .key_info
            .as_ref()
            .ok_or_else(|| PackagerError::EncryptionError("Key info not set".to_string()))?;
        aes128_ctr_apply(&key_info.key, &key_info.iv, data)
    }

    /// CENC encryption stub when the `encryption` feature is disabled.
    #[cfg(not(feature = "encryption"))]
    fn encrypt_cenc(&self, _data: &[u8]) -> PackagerResult<Vec<u8>> {
        Err(PackagerError::EncryptionError(
            "Encryption feature not enabled".to_string(),
        ))
    }

    /// Decrypt `cenc` (AES-128-CTR). CTR is symmetric, so this applies the same
    /// keystream as [`Self::encrypt_cenc`].
    #[cfg(feature = "encryption")]
    fn decrypt_cenc(&self, data: &[u8]) -> PackagerResult<Vec<u8>> {
        let key_info = self
            .key_info
            .as_ref()
            .ok_or_else(|| PackagerError::EncryptionError("Key info not set".to_string()))?;
        aes128_ctr_apply(&key_info.key, &key_info.iv, data)
    }

    /// CENC decryption stub when the `encryption` feature is disabled.
    #[cfg(not(feature = "encryption"))]
    fn decrypt_cenc(&self, _data: &[u8]) -> PackagerResult<Vec<u8>> {
        Err(PackagerError::EncryptionError(
            "Encryption feature not enabled".to_string(),
        ))
    }

    /// Generate HLS EXT-X-KEY tag.
    pub fn generate_hls_key_tag(&self) -> PackagerResult<String> {
        if !self.is_enabled() {
            return Ok(String::new());
        }

        let key_info = self
            .key_info
            .as_ref()
            .ok_or_else(|| PackagerError::EncryptionError("Key info not set".to_string()))?;

        let method = match self.method {
            EncryptionMethod::Aes128 => "AES-128",
            EncryptionMethod::SampleAes => "SAMPLE-AES",
            _ => {
                return Err(PackagerError::EncryptionError(
                    "Unsupported method for HLS".to_string(),
                ))
            }
        };

        let uri = key_info
            .uri
            .as_ref()
            .ok_or_else(|| PackagerError::EncryptionError("Key URI not set".to_string()))?;

        let iv_hex = hex::encode(&key_info.iv);

        let mut tag = format!("#EXT-X-KEY:METHOD={method},URI=\"{uri}\",IV=0x{iv_hex}");

        if let Some(format) = &key_info.format {
            tag.push_str(&format!(",KEYFORMAT=\"{format}\""));
        }

        if let Some(versions) = &key_info.format_versions {
            tag.push_str(&format!(",KEYFORMATVERSIONS=\"{versions}\""));
        }

        Ok(tag)
    }

    /// Get key info.
    #[must_use]
    pub fn key_info(&self) -> Option<&KeyInfo> {
        self.key_info.as_ref()
    }
}

// ---------------------------------------------------------------------------
// Low-level AES primitives backing the CENC `cenc` (CTR) and `cbcs`
// (pattern-CBC) paths. These operate on raw 16-byte AES blocks (via the `aes`
// crate's block cipher, which selects AES-NI at runtime) so the CTR keystream
// and the crypt/skip pattern can be expressed exactly per ISO/IEC 23001-7,
// without the fixed full-buffer padding of a high-level CBC block mode.
// ---------------------------------------------------------------------------

/// AES block size in bytes.
#[cfg(feature = "encryption")]
const AES_BLOCK: usize = 16;

/// CENC `cbcs` default pattern: encrypt 1 block, skip 9 (ISO/IEC 23001-7 §9.6 —
/// the pattern mandated by Apple FairPlay and used for HLS SAMPLE-AES video).
#[cfg(feature = "encryption")]
const CBCS_CRYPT_BLOCKS: usize = 1;
#[cfg(feature = "encryption")]
const CBCS_SKIP_BLOCKS: usize = 9;

/// Encrypt a single 16-byte AES-128 block.
#[cfg(feature = "encryption")]
fn aes128_encrypt_block(cipher: &Aes128, block: &[u8; AES_BLOCK]) -> [u8; AES_BLOCK] {
    let mut b = aes::Block::from(*block);
    cipher.encrypt_block(&mut b);
    let mut out = [0u8; AES_BLOCK];
    out.copy_from_slice(b.as_slice());
    out
}

/// Decrypt a single 16-byte AES-128 block.
#[cfg(feature = "encryption")]
fn aes128_decrypt_block(cipher: &Aes128Dec, block: &[u8; AES_BLOCK]) -> [u8; AES_BLOCK] {
    let mut b = aes::Block::from(*block);
    cipher.decrypt_block(&mut b);
    let mut out = [0u8; AES_BLOCK];
    out.copy_from_slice(b.as_slice());
    out
}

/// Increment a 16-byte big-endian counter block in place (CTR mode).
#[cfg(feature = "encryption")]
fn increment_be_counter(counter: &mut [u8; AES_BLOCK]) {
    for byte in counter.iter_mut().rev() {
        *byte = byte.wrapping_add(1);
        if *byte != 0 {
            break;
        }
    }
}

/// Apply the AES-128-CTR keystream to `data`.
///
/// CTR encryption and decryption are the same operation (XOR with the
/// keystream), so this backs both directions of the `cenc` scheme. `iv` is the
/// 16-byte initial counter block; it is incremented as a 128-bit big-endian
/// integer once per 16-byte block.
#[cfg(feature = "encryption")]
fn aes128_ctr_apply(key: &[u8], iv: &[u8], data: &[u8]) -> PackagerResult<Vec<u8>> {
    if iv.len() != AES_BLOCK {
        return Err(PackagerError::EncryptionError(format!(
            "CENC (CTR) IV must be 16 bytes, got {}",
            iv.len()
        )));
    }
    let cipher = Aes128::new_from_slice(key).map_err(|e| {
        PackagerError::EncryptionError(format!("Failed to create AES-128 cipher: {e}"))
    })?;

    let mut counter = [0u8; AES_BLOCK];
    counter.copy_from_slice(iv);

    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks(AES_BLOCK) {
        let keystream = aes128_encrypt_block(&cipher, &counter);
        for (&byte, &ks) in chunk.iter().zip(keystream.iter()) {
            out.push(byte ^ ks);
        }
        increment_be_counter(&mut counter);
    }
    Ok(out)
}

/// Encrypt `data` with the CENC `cbcs` pattern (AES-128-CBC, 1:9 crypt/skip).
///
/// Every `CBCS_CRYPT_BLOCKS`-of-`(CBCS_CRYPT_BLOCKS + CBCS_SKIP_BLOCKS)` whole
/// 16-byte block is CBC-encrypted; the rest, plus any trailing partial block,
/// are left clear. The CBC chaining register is seeded once with the per-sample
/// IV and advanced only by encrypted blocks.
#[cfg(feature = "encryption")]
fn sample_aes_cbcs_encrypt(key: &[u8], iv: &[u8], data: &[u8]) -> PackagerResult<Vec<u8>> {
    if iv.len() != AES_BLOCK {
        return Err(PackagerError::EncryptionError(format!(
            "SAMPLE-AES (cbcs) IV must be 16 bytes, got {}",
            iv.len()
        )));
    }
    let cipher = Aes128::new_from_slice(key).map_err(|e| {
        PackagerError::EncryptionError(format!("Failed to create AES-128 cipher: {e}"))
    })?;

    let cycle = CBCS_CRYPT_BLOCKS + CBCS_SKIP_BLOCKS;
    let full_blocks = data.len() / AES_BLOCK;

    let mut out = Vec::with_capacity(data.len());
    let mut prev = [0u8; AES_BLOCK];
    prev.copy_from_slice(iv);

    for block_idx in 0..full_blocks {
        let start = block_idx * AES_BLOCK;
        let mut block = [0u8; AES_BLOCK];
        block.copy_from_slice(&data[start..start + AES_BLOCK]);

        if block_idx % cycle < CBCS_CRYPT_BLOCKS {
            let mut xored = [0u8; AES_BLOCK];
            for ((dst, &b), &p) in xored.iter_mut().zip(block.iter()).zip(prev.iter()) {
                *dst = b ^ p;
            }
            let ct = aes128_encrypt_block(&cipher, &xored);
            out.extend_from_slice(&ct);
            prev = ct;
        } else {
            out.extend_from_slice(&block);
        }
    }

    // Trailing partial block (< 16 bytes) is always left clear per spec.
    out.extend_from_slice(&data[full_blocks * AES_BLOCK..]);
    Ok(out)
}

/// Decrypt `data` produced by [`sample_aes_cbcs_encrypt`] (exact inverse).
#[cfg(feature = "encryption")]
fn sample_aes_cbcs_decrypt(key: &[u8], iv: &[u8], data: &[u8]) -> PackagerResult<Vec<u8>> {
    if iv.len() != AES_BLOCK {
        return Err(PackagerError::EncryptionError(format!(
            "SAMPLE-AES (cbcs) IV must be 16 bytes, got {}",
            iv.len()
        )));
    }
    let cipher = Aes128Dec::new_from_slice(key).map_err(|e| {
        PackagerError::EncryptionError(format!("Failed to create AES-128 cipher: {e}"))
    })?;

    let cycle = CBCS_CRYPT_BLOCKS + CBCS_SKIP_BLOCKS;
    let full_blocks = data.len() / AES_BLOCK;

    let mut out = Vec::with_capacity(data.len());
    let mut prev = [0u8; AES_BLOCK];
    prev.copy_from_slice(iv);

    for block_idx in 0..full_blocks {
        let start = block_idx * AES_BLOCK;
        let mut block = [0u8; AES_BLOCK];
        block.copy_from_slice(&data[start..start + AES_BLOCK]);

        if block_idx % cycle < CBCS_CRYPT_BLOCKS {
            let dec = aes128_decrypt_block(&cipher, &block);
            for (&d, &p) in dec.iter().zip(prev.iter()) {
                out.push(d ^ p);
            }
            prev = block;
        } else {
            out.extend_from_slice(&block);
        }
    }

    out.extend_from_slice(&data[full_blocks * AES_BLOCK..]);
    Ok(out)
}

// ---------------------------------------------------------------------------
// NAL-unit-aware subsample mapping for `cbcs` pattern encryption.
//
// The functions above (`sample_aes_cbcs_encrypt`/`_decrypt`) apply the
// crypt/skip pattern over a whole buffer, which is spec-correct for
// elementary payloads (AV1/VP9/VP8/Opus/Vorbis/FLAC — see
// `CodecStructure::Elementary`) but not for NAL-structured codecs (AVC/HEVC),
// where CENC requires clear NAL headers/slice headers and pattern encryption
// scoped to each NAL's protected data (ISO/IEC 23001-7 §10.4). This section
// parses length-prefixed NAL units (ISO/IEC 14496-15) into CENC subsamples
// and applies pattern encryption per subsample.
// ---------------------------------------------------------------------------

/// NAL unit header format, used to classify a NAL unit as VCL (slice data) or
/// non-VCL (parameter sets, SEI, delimiters, …) for CENC `cbcs` subsample
/// mapping. Different NAL-structured codec families place `nal_unit_type` at
/// a different bit offset and give it a different VCL range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NalHeaderStyle {
    /// ITU-T H.264 / AVC: a 1-byte NAL header (`forbidden_zero_bit(1) +
    /// nal_ref_idc(2) + nal_unit_type(5)`). VCL (slice) types are 1-5
    /// (non-IDR slice, slice data partitions A/B/C, IDR slice); every other
    /// type (6=SEI, 7=SPS, 8=PPS, 9=AUD, …) is non-VCL.
    Avc,
    /// ITU-T H.265 / HEVC: a 2-byte NAL header (`forbidden_zero_bit(1) +
    /// nal_unit_type(6) + nuh_layer_id(6) + nuh_temporal_id_plus1(3)`,
    /// `nal_unit_type` entirely within the first byte). VCL (slice) types are
    /// 0-31; types 32 and above (VPS/SPS/PPS/SEI/AUD/…) are non-VCL.
    Hevc,
}

/// How a codec's sample payload is structured, for CENC `cbcs` pattern
/// encryption routing.
///
/// [`codec_structure_for`] maps a codec name (as used in
/// [`crate::config::BitrateEntry::codec`]) to this; [`EncryptionHandler`]'s
/// `*_for_codec` methods use it to choose between the whole-buffer pattern
/// path (spec-correct for elementary payloads) and NAL-unit-aware subsample
/// mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecStructure {
    /// One opaque elementary block per sample, with no internal unit framing
    /// this crate parses: AV1, VP9, VP8, Opus, Vorbis, FLAC, PCM, and any
    /// codec name not recognized by [`codec_structure_for`].
    ///
    /// `cbcs` pattern encryption is applied over the whole sample buffer —
    /// the ISO/IEC 23001-7 spec-correct behavior for elementary payloads.
    ///
    /// AV1 is intentionally kept on this path rather than faked: the AV1
    /// Codec ISOBMFF Binding Specification defines its own OBU-based
    /// subsample rules (each OBU is its own subsample, with large frames
    /// split further at tile-group boundaries), a materially different — and
    /// currently unimplemented — mapping from the NAL model below. Encrypting
    /// AV1 with the whole-buffer path is honest (it matches this crate's
    /// actual capability); claiming OBU-aware subsampling without
    /// implementing it would not be.
    Elementary,
    /// A sequence of length-prefixed NAL units per ISO/IEC 14496-15 (the
    /// AVC/HEVC ISOBMFF "sample format"): each unit is `[length: length_size
    /// bytes, big-endian][NAL unit bytes]`.
    ///
    /// None of the codecs this tree currently emits (AV1/VP9/VP8 video,
    /// Opus/Vorbis/FLAC audio — see
    /// [`crate::variant_stream::StreamCodec`]) use this framing;
    /// [`codec_structure_for`] only returns this variant for H.264/H.265
    /// codec names. It exists so `cbcs` subsample mapping is ready for those
    /// codecs, and is directly exercised by [`nal_subsamples`] and the NAL
    /// subsample tests in this module.
    NalLengthPrefixed {
        /// Length-field size in bytes: 1, 2, or 4 (`lengthSizeMinusOne + 1`
        /// from the `avcC`/`hvcC` box when known; 4 is by far the most
        /// common value in the wild and is what [`codec_structure_for`]
        /// assumes).
        length_size: u8,
        /// NAL header layout used to tell VCL (slice) NAL units apart from
        /// non-VCL ones.
        header_style: NalHeaderStyle,
    },
}

/// Maps a short codec name (as used in [`crate::config::BitrateEntry::codec`]
/// / [`crate::ladder::SourceInfo::codec`]) to its [`CodecStructure`] for
/// `cbcs` subsample routing. Matching is case-insensitive.
///
/// | Codec name(s) | Structure |
/// |---|---|
/// | `av1`, `vp9`, `vp8` (this tree's video codecs) | [`CodecStructure::Elementary`] |
/// | `opus`, `vorbis`, `flac` (this tree's audio codecs) | [`CodecStructure::Elementary`] |
/// | `h264`, `avc`, `avc1`, `avc3` | [`CodecStructure::NalLengthPrefixed`] with [`NalHeaderStyle::Avc`] |
/// | `h265`, `hevc`, `hvc1`, `hev1` | [`CodecStructure::NalLengthPrefixed`] with [`NalHeaderStyle::Hevc`] |
/// | anything else | [`CodecStructure::Elementary`] (safe default — whole-buffer pattern encryption can't corrupt stream structure the way a wrong NAL parse could) |
#[must_use]
pub fn codec_structure_for(codec: &str) -> CodecStructure {
    match codec.to_ascii_lowercase().as_str() {
        "h264" | "avc" | "avc1" | "avc3" => CodecStructure::NalLengthPrefixed {
            length_size: 4,
            header_style: NalHeaderStyle::Avc,
        },
        "h265" | "hevc" | "hvc1" | "hev1" => CodecStructure::NalLengthPrefixed {
            length_size: 4,
            header_style: NalHeaderStyle::Hevc,
        },
        _ => CodecStructure::Elementary,
    }
}

/// One CENC subsample split (ISO/IEC 23001-7 `SubSampleEntry`): within a
/// sample, `clear_bytes` leading bytes are left unencrypted, followed by
/// `protected_bytes` bytes subject to the active encryption scheme (for
/// `cbcs`, the crypt/skip pattern — see [`nal_subsamples`]).
///
/// These are logical, in-memory subsamples consumed directly by
/// [`EncryptionHandler::encrypt_for_codec`] / `decrypt_for_codec`; this crate
/// does not currently serialize a `senc`/`saiz`/`saio` box. Note for any
/// future writer: the real `SubSampleEntry.bytes_of_clear_data` field is
/// `unsigned int(16)`, narrower than `clear_bytes` here — a clear run over
/// 65535 bytes (e.g. an unusually large non-VCL NAL) would need splitting
/// into multiple zero-protected entries to serialize into that box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubsampleEntry {
    /// Number of leading clear (unencrypted) bytes.
    pub clear_bytes: u32,
    /// Number of following bytes subject to the active encryption pattern.
    pub protected_bytes: u32,
}

/// VCL (slice data) vs non-VCL classification of a single NAL unit, used to
/// decide whether [`nal_subsamples`] gives it a clear leader plus pattern
/// encryption, or leaves the whole unit clear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NalKind {
    /// Slice data: carries a slice header followed by coded picture data.
    Vcl,
    /// Parameter sets, SEI, access unit delimiters, and everything else a
    /// decoder must be able to read even without a license.
    NonVcl,
}

/// Classifies a single NAL unit's payload (the bytes *after* its length
/// prefix) as VCL or non-VCL, per `header_style`.
///
/// Matches Apple's HLS SAMPLE-AES specification for AVC (NAL types 1 and 5
/// are the slice types it requires to be encrypted; this generalizes to the
/// full VCL range 1-5 per ITU-T H.264, and to 0-31 for HEVC per ITU-T H.265)
/// and real-world CENC packagers, which likewise leave parameter sets and SEI
/// fully clear so a decoder can always read stream structure without a key.
fn nal_kind(nal_payload: &[u8], header_style: NalHeaderStyle) -> NalKind {
    let Some(&first) = nal_payload.first() else {
        return NalKind::NonVcl;
    };
    let is_vcl = match header_style {
        NalHeaderStyle::Avc => (1..=5).contains(&(first & 0x1F)),
        NalHeaderStyle::Hevc => ((first >> 1) & 0x3F) <= 31,
    };
    if is_vcl {
        NalKind::Vcl
    } else {
        NalKind::NonVcl
    }
}

/// Clear leader length, in bytes of NAL *payload* (counted from the NAL
/// header byte, not including the length prefix), that CENC `cbcs` leaves
/// unencrypted at the start of a VCL NAL unit: the NAL header plus enough of
/// the slice header to be a safe, decoder-independent boundary. Matches
/// Apple's HLS SAMPLE-AES specification ("the byte containing nal_unit_type,
/// plus the 31 bytes that follow, are unencrypted") and is the value used by
/// mainstream CENC `cbcs` packagers for AVC/HEVC.
const NAL_CLEAR_LEAD_BYTES: usize = 32;

/// Parses `data` as a sequence of length-prefixed NAL units (ISO/IEC
/// 14496-15) and computes the CENC subsample split that `cbcs` pattern
/// encryption must honor for each one (ISO/IEC 23001-7 §10.4 — pattern
/// encryption applies to the protected portion of each subsample, video NAL
/// payloads only):
///
/// - Non-VCL NAL units (parameter sets, SEI, delimiters — see `nal_kind`)
///   are emitted as fully clear subsamples (`protected_bytes = 0`), since a
///   decoder must be able to read them without a license.
/// - VCL (slice) NAL units get a `NAL_CLEAR_LEAD_BYTES`-byte clear leader
///   (or the whole payload, if shorter) covering the NAL header and slice
///   header; the remainder is `protected` for the caller to pattern-encrypt.
///
/// In both cases the length-prefix field itself is always counted as clear —
/// a decryptor/demuxer must be able to walk NAL boundaries in the encrypted
/// bitstream, which is the entire reason `cbcs` leaves framing visible.
///
/// # Errors
/// Returns [`PackagerError::EncryptionError`] if `data` is truncated relative
/// to its own length prefixes (a malformed or non-NAL-structured buffer), if
/// a declared NAL length would overflow, or if `length_size` is not `1..=4`.
pub fn nal_subsamples(
    data: &[u8],
    length_size: u8,
    header_style: NalHeaderStyle,
) -> PackagerResult<Vec<SubsampleEntry>> {
    let length_size = length_size as usize;
    if !(1..=4).contains(&length_size) {
        return Err(PackagerError::EncryptionError(format!(
            "NAL length size must be 1-4 bytes, got {length_size}"
        )));
    }

    let mut subsamples = Vec::new();
    let mut offset = 0usize;
    while offset < data.len() {
        if offset + length_size > data.len() {
            return Err(PackagerError::EncryptionError(format!(
                "truncated NAL length prefix at offset {offset} ({} bytes remain, need {length_size})",
                data.len() - offset
            )));
        }
        let nal_len = data[offset..offset + length_size]
            .iter()
            .fold(0usize, |acc, &b| (acc << 8) | b as usize);
        let payload_start = offset + length_size;
        let Some(payload_end) = payload_start.checked_add(nal_len) else {
            return Err(PackagerError::EncryptionError(format!(
                "NAL unit at offset {offset} declares an overflowing length {nal_len}"
            )));
        };
        if payload_end > data.len() {
            return Err(PackagerError::EncryptionError(format!(
                "NAL unit at offset {offset} declares length {nal_len} but only {} bytes remain",
                data.len() - payload_start
            )));
        }

        let payload = &data[payload_start..payload_end];
        let entry = match nal_kind(payload, header_style) {
            NalKind::NonVcl => SubsampleEntry {
                clear_bytes: (length_size + nal_len) as u32,
                protected_bytes: 0,
            },
            NalKind::Vcl => {
                let clear_payload = nal_len.min(NAL_CLEAR_LEAD_BYTES);
                SubsampleEntry {
                    clear_bytes: (length_size + clear_payload) as u32,
                    protected_bytes: (nal_len - clear_payload) as u32,
                }
            }
        };
        subsamples.push(entry);

        offset = payload_end;
    }

    Ok(subsamples)
}

/// Sanity-checks that `subsamples` actually reflects NAL-structured video
/// data before pattern encryption/decryption proceeds.
///
/// [`nal_subsamples`] is a purely structural parser: fed a buffer that isn't
/// really length-prefixed AVC/HEVC, it can still return `Ok` with a split
/// that "parses" cleanly but is wrong — most notably, Annex-B byte-stream
/// framing (`00 00 00 01` start codes) misreads as a sequence of near-zero
/// declared NAL lengths, every one short enough to be classified fully clear.
/// Encrypting such a misparse would return the buffer completely unmodified
/// while still reporting success, which is exactly the "`Ok` for protection
/// that didn't happen" failure this crate's honesty policy forbids — so a
/// non-empty sample whose subsamples are *all* fully clear
/// (`protected_bytes == 0` everywhere) is treated as a hard error instead of
/// silently returned as if it had been protected.
///
/// A real coded video access unit always contains at least one VCL (slice)
/// NAL carrying actual picture data, so this cannot reject genuine input
/// unless every slice in the sample is implausibly small (under
/// [`NAL_CLEAR_LEAD_BYTES`] bytes) — samples with at least one normally-sized
/// slice are unaffected.
///
/// # Errors
/// Returns [`PackagerError::EncryptionError`] when `data` is non-empty and no
/// subsample has any protected bytes.
#[cfg(feature = "encryption")]
fn ensure_nal_sample_has_protected_data(
    data: &[u8],
    subsamples: &[SubsampleEntry],
) -> PackagerResult<()> {
    if !data.is_empty() && subsamples.iter().all(|s| s.protected_bytes == 0) {
        return Err(PackagerError::EncryptionError(
            "NAL-structured cbcs encryption found no protected (VCL slice) data anywhere in \
             this sample; the buffer is likely not really length-prefixed AVC/HEVC (ISO/IEC \
             14496-15) -- e.g. Annex-B start-code framing would misparse this way -- refusing \
             rather than silently returning it unencrypted"
                .to_string(),
        ));
    }
    Ok(())
}

/// Encrypts `data` (a sample already split into `subsamples` — see
/// [`nal_subsamples`]) with the CENC `cbcs` pattern, subsample by subsample:
/// each subsample's clear bytes pass through unchanged, and its protected
/// bytes are independently pattern-encrypted via [`sample_aes_cbcs_encrypt`].
///
/// Per ISO/IEC 23001-7 and matching real-world implementations — verified
/// against FFmpeg's `cbcs_scheme_decrypt` (`libavformat/mov.c`, which
/// `memcpy`s the *sample* IV back into its working IV at the start of every
/// subsample) and Shaka Packager's `kUseConstantIv` pattern cryptor — both the
/// crypt/skip pattern cycle *and* the AES-CBC chaining value restart at the
/// beginning of every subsample's protected range, reseeded from the sample
/// IV each time; neither carries over from the previous subsample. That is
/// exactly what the single-buffer [`sample_aes_cbcs_encrypt`] already does, so
/// each subsample can reuse it directly with the unmodified sample IV.
#[cfg(feature = "encryption")]
fn sample_aes_cbcs_encrypt_subsamples(
    key: &[u8],
    iv: &[u8],
    data: &[u8],
    subsamples: &[SubsampleEntry],
) -> PackagerResult<Vec<u8>> {
    let total: usize = subsamples
        .iter()
        .map(|s| s.clear_bytes as usize + s.protected_bytes as usize)
        .sum();
    if total != data.len() {
        return Err(PackagerError::EncryptionError(format!(
            "subsample byte total {total} does not match sample length {}",
            data.len()
        )));
    }

    let mut out = Vec::with_capacity(data.len());
    let mut offset = 0usize;
    for sub in subsamples {
        let clear_len = sub.clear_bytes as usize;
        out.extend_from_slice(&data[offset..offset + clear_len]);
        offset += clear_len;

        let protected_len = sub.protected_bytes as usize;
        let encrypted = sample_aes_cbcs_encrypt(key, iv, &data[offset..offset + protected_len])?;
        out.extend_from_slice(&encrypted);
        offset += protected_len;
    }

    Ok(out)
}

/// Decrypts data produced by [`sample_aes_cbcs_encrypt_subsamples`] (exact
/// inverse — `subsamples` must be the same split, which is safe to recompute
/// from the ciphertext via [`nal_subsamples`] since clear bytes, including
/// every NAL length prefix and header, are never modified by encryption).
#[cfg(feature = "encryption")]
fn sample_aes_cbcs_decrypt_subsamples(
    key: &[u8],
    iv: &[u8],
    data: &[u8],
    subsamples: &[SubsampleEntry],
) -> PackagerResult<Vec<u8>> {
    let total: usize = subsamples
        .iter()
        .map(|s| s.clear_bytes as usize + s.protected_bytes as usize)
        .sum();
    if total != data.len() {
        return Err(PackagerError::EncryptionError(format!(
            "subsample byte total {total} does not match sample length {}",
            data.len()
        )));
    }

    let mut out = Vec::with_capacity(data.len());
    let mut offset = 0usize;
    for sub in subsamples {
        let clear_len = sub.clear_bytes as usize;
        out.extend_from_slice(&data[offset..offset + clear_len]);
        offset += clear_len;

        let protected_len = sub.protected_bytes as usize;
        let decrypted = sample_aes_cbcs_decrypt(key, iv, &data[offset..offset + protected_len])?;
        out.extend_from_slice(&decrypted);
        offset += protected_len;
    }

    Ok(out)
}

/// Number of PBKDF2-HMAC-SHA256 rounds used by [`KeyGenerator::from_passphrase`].
///
/// `100_000` rounds meets the current OWASP minimum recommendation for
/// PBKDF2-HMAC-SHA256 password-based key derivation.
pub const PBKDF2_ITERATIONS: u32 = 100_000;

/// Key generator for creating encryption keys.
///
/// All keys and IVs are generated from the operating system's cryptographically
/// secure random number source (via [`rand::rngs::SysRng`], backed by `getrandom`).
pub struct KeyGenerator;

impl KeyGenerator {
    /// Generate a cryptographically secure random AES-128 key.
    ///
    /// Uses the OS CSPRNG ([`rand::rngs::SysRng`]) to fill 16 bytes of key
    /// material. Each call returns an independent, unpredictable key.
    ///
    /// # Errors
    /// Returns [`PackagerError::EncryptionError`] if the operating system's
    /// random number source is unavailable.
    pub fn generate_aes128_key() -> PackagerResult<Vec<u8>> {
        let mut key = vec![0u8; 16];
        rand::rngs::SysRng.try_fill_bytes(&mut key).map_err(|e| {
            PackagerError::EncryptionError(format!(
                "Failed to generate secure random AES-128 key: {e}"
            ))
        })?;
        Ok(key)
    }

    /// Generate a cryptographically secure random initialization vector.
    ///
    /// # Errors
    /// Returns [`PackagerError::EncryptionError`] if the operating system's
    /// random number source is unavailable.
    pub fn generate_iv() -> PackagerResult<Vec<u8>> {
        let mut iv = vec![0u8; 16];
        rand::rngs::SysRng.try_fill_bytes(&mut iv).map_err(|e| {
            PackagerError::EncryptionError(format!("Failed to generate secure random IV: {e}"))
        })?;
        Ok(iv)
    }

    /// Derive a 16-byte AES-128 key from a passphrase using PBKDF2-HMAC-SHA256.
    ///
    /// `salt` should be a unique, random value (at least 16 bytes recommended)
    /// generated once per key and stored/transmitted alongside the derived key
    /// (a salt is not secret, but it must not be reused across unrelated keys).
    /// Callers can generate one via [`Self::generate_iv`] or any other CSPRNG
    /// source. Uses [`PBKDF2_ITERATIONS`] rounds.
    ///
    /// This function is deterministic: the same `passphrase` and `salt` always
    /// produce the same key, while different salts (or passphrases) produce
    /// different keys.
    ///
    /// # Errors
    /// Returns [`PackagerError::EncryptionError`] if the underlying HMAC
    /// cannot be initialized (this only happens for pathological key sizes
    /// and should not occur in practice for `str` passphrases).
    pub fn from_passphrase(passphrase: &str, salt: &[u8]) -> PackagerResult<Vec<u8>> {
        let mut key = [0u8; 16];
        pbkdf2::pbkdf2::<pbkdf2::hmac::Hmac<sha2::Sha256>>(
            passphrase.as_bytes(),
            salt,
            PBKDF2_ITERATIONS,
            &mut key,
        )
        .map_err(|e| {
            PackagerError::EncryptionError(format!("Failed to derive key from passphrase: {e}"))
        })?;

        Ok(key.to_vec())
    }
}

/// DRM preparation hooks.
pub trait DrmProvider {
    /// Get DRM system ID.
    fn system_id(&self) -> &str;

    /// Generate PSSH box data.
    fn generate_pssh(&self, key_id: &[u8]) -> PackagerResult<Vec<u8>>;

    /// Get license server URL.
    fn license_url(&self) -> Option<String>;
}

/// Widevine DRM provider (placeholder).
pub struct WidevineDrmProvider {
    license_url: String,
}

impl WidevineDrmProvider {
    /// Create a new Widevine DRM provider.
    #[must_use]
    pub fn new(license_url: String) -> Self {
        Self { license_url }
    }
}

impl DrmProvider for WidevineDrmProvider {
    fn system_id(&self) -> &'static str {
        "edef8ba9-79d6-4ace-a3c8-27dcd51d21ed" // Widevine system ID
    }

    fn generate_pssh(&self, key_id: &[u8]) -> PackagerResult<Vec<u8>> {
        let mut pssh = BytesMut::new();

        // PSSH box header
        pssh.put_u32(0); // Size placeholder
        pssh.put_slice(b"pssh");
        pssh.put_u32(0); // Version and flags

        // System ID (Widevine)
        let system_id = hex::decode(self.system_id().replace('-', ""))
            .map_err(|_| PackagerError::DrmFailed("Invalid system ID".to_string()))?;
        pssh.put_slice(&system_id);

        // Key ID count and IDs
        pssh.put_u32(1);
        pssh.put_slice(key_id);

        // Data size and data (empty for now)
        pssh.put_u32(0);

        // Update size
        let size = pssh.len();
        pssh[0..4].copy_from_slice(&(size as u32).to_be_bytes());

        Ok(pssh.to_vec())
    }

    fn license_url(&self) -> Option<String> {
        Some(self.license_url.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_generation() {
        let key = KeyGenerator::generate_aes128_key().expect("RNG should succeed in test");
        assert_eq!(key.len(), 16);
    }

    #[test]
    fn test_key_generation_is_random_not_derived_from_timestamp() {
        // Two successive calls must produce different keys (probabilistic
        // uniqueness check for a CSPRNG). With the old timestamp-derived
        // implementation, calls in quick succession could collide or be
        // trivially brute-forced; a CSPRNG must not.
        let key1 = KeyGenerator::generate_aes128_key().expect("RNG should succeed in test");
        let key2 = KeyGenerator::generate_aes128_key().expect("RNG should succeed in test");
        assert_eq!(key1.len(), 16);
        assert_eq!(key2.len(), 16);
        assert_ne!(key1, key2, "successive CSPRNG keys must not collide");
    }

    #[test]
    fn test_iv_generation_is_random() {
        let iv1 = KeyGenerator::generate_iv().expect("RNG should succeed in test");
        let iv2 = KeyGenerator::generate_iv().expect("RNG should succeed in test");
        assert_eq!(iv1.len(), 16);
        assert_eq!(iv2.len(), 16);
        assert_ne!(iv1, iv2, "successive CSPRNG IVs must not collide");
    }

    #[test]
    fn test_from_passphrase_is_deterministic_for_same_salt() {
        let salt = b"a fixed test salt of 16B";
        let key1 = KeyGenerator::from_passphrase("correct horse battery staple", salt)
            .expect("KDF should succeed in test");
        let key2 = KeyGenerator::from_passphrase("correct horse battery staple", salt)
            .expect("KDF should succeed in test");
        assert_eq!(key1.len(), 16);
        assert_eq!(
            key1, key2,
            "PBKDF2 must be deterministic for same passphrase+salt"
        );
    }

    #[test]
    fn test_from_passphrase_differs_for_different_salt() {
        let key1 = KeyGenerator::from_passphrase("correct horse battery staple", b"salt-one")
            .expect("KDF should succeed in test");
        let key2 = KeyGenerator::from_passphrase("correct horse battery staple", b"salt-two")
            .expect("KDF should succeed in test");
        assert_ne!(key1, key2, "different salts must derive different keys");
    }

    #[test]
    fn test_from_passphrase_differs_for_different_passphrase() {
        let salt = b"a fixed test salt of 16B";
        let key1 = KeyGenerator::from_passphrase("password one", salt).expect("KDF should succeed");
        let key2 = KeyGenerator::from_passphrase("password two", salt).expect("KDF should succeed");
        assert_ne!(
            key1, key2,
            "different passphrases must derive different keys"
        );
    }

    #[test]
    fn test_key_info_validation() {
        let key = vec![0u8; 16];
        let iv = vec![0u8; 16];
        let key_info = KeyInfo::new(key, iv);

        assert!(key_info.validate().is_ok());
    }

    #[test]
    fn test_key_info_invalid_key_size() {
        let key = vec![0u8; 8]; // Wrong size
        let iv = vec![0u8; 16];
        let key_info = KeyInfo::new(key, iv);

        assert!(key_info.validate().is_err());
    }

    #[test]
    fn test_encryption_handler_creation() {
        let handler = EncryptionHandler::new(EncryptionMethod::Aes128);
        assert!(handler.is_enabled());
    }

    #[test]
    fn test_hls_key_tag_generation() {
        let key = vec![0u8; 16];
        let iv = vec![0u8; 16];
        let key_info = KeyInfo::new(key, iv).with_uri("https://example.com/key".to_string());

        let mut handler = EncryptionHandler::new(EncryptionMethod::Aes128);
        handler
            .set_key_info(key_info)
            .expect("should succeed in test");

        let tag = handler
            .generate_hls_key_tag()
            .expect("should succeed in test");
        assert!(tag.contains("AES-128"));
        assert!(tag.contains("https://example.com/key"));
    }

    #[test]
    #[cfg(feature = "encryption")]
    fn test_aes128_encrypt_decrypt_roundtrip() {
        let key = vec![
            0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf,
            0x4f, 0x3c,
        ];
        let iv = vec![
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f,
        ];
        // exactly 32 bytes of plaintext
        let plaintext = b"Hello OxiMedia AES-128 test data";

        let key_info = KeyInfo::new(key, iv);
        let mut handler = EncryptionHandler::new(EncryptionMethod::Aes128);
        handler
            .set_key_info(key_info)
            .expect("set_key_info should succeed in test");

        let ciphertext = handler
            .encrypt(plaintext)
            .expect("encrypt should succeed in test");
        assert_ne!(
            &ciphertext[..plaintext.len()],
            plaintext.as_ref(),
            "ciphertext must differ from plaintext"
        );
        assert_eq!(ciphertext.len() % 16, 0, "ciphertext must be block-aligned");

        let decrypted = handler
            .decrypt(&ciphertext)
            .expect("decrypt should succeed in test");
        assert_eq!(
            &decrypted[..plaintext.len()],
            plaintext.as_ref(),
            "AES-128 round-trip must recover original plaintext"
        );
    }

    #[test]
    #[cfg(feature = "encryption")]
    fn test_cenc_aes128_ctr_nist_known_answer() {
        // NIST SP 800-38A, F.5.1 CTR-AES128.Encrypt. The 16-byte IV is the
        // initial counter block, incremented as a full 128-bit big-endian
        // integer across the byte boundary (...feff -> ...ff00).
        let key = vec![
            0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf,
            0x4f, 0x3c,
        ];
        let iv = vec![
            0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa, 0xfb, 0xfc, 0xfd,
            0xfe, 0xff,
        ];
        let plaintext = vec![
            0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93,
            0x17, 0x2a, 0xae, 0x2d, 0x8a, 0x57, 0x1e, 0x03, 0xac, 0x9c, 0x9e, 0xb7, 0x6f, 0xac,
            0x45, 0xaf, 0x8e, 0x51, 0x30, 0xc8, 0x1c, 0x46, 0xa3, 0x5c, 0xe4, 0x11, 0xe5, 0xfb,
            0xc1, 0x19, 0x1a, 0x0a, 0x52, 0xef, 0xf6, 0x9f, 0x24, 0x45, 0xdf, 0x4f, 0x9b, 0x17,
            0xad, 0x2b, 0x41, 0x7b, 0xe6, 0x6c, 0x37, 0x10,
        ];
        let expected = vec![
            0x87, 0x4d, 0x61, 0x91, 0xb6, 0x20, 0xe3, 0x26, 0x1b, 0xef, 0x68, 0x64, 0x99, 0x0d,
            0xb6, 0xce, 0x98, 0x06, 0xf6, 0x6b, 0x79, 0x70, 0xfd, 0xff, 0x86, 0x17, 0x18, 0x7b,
            0xb9, 0xff, 0xfd, 0xff, 0x5a, 0xe4, 0xdf, 0x3e, 0xdb, 0xd5, 0xd3, 0x5e, 0x5b, 0x4f,
            0x09, 0x02, 0x0d, 0xb0, 0x3e, 0xab, 0x1e, 0x03, 0x1d, 0xda, 0x2f, 0xbe, 0x03, 0xd1,
            0x79, 0x21, 0x70, 0xa0, 0xf3, 0x00, 0x9c, 0xee,
        ];

        let key_info = KeyInfo::new(key, iv);
        let mut handler = EncryptionHandler::new(EncryptionMethod::Cenc);
        handler
            .set_key_info(key_info)
            .expect("set_key_info should succeed in test");

        let ciphertext = handler.encrypt(&plaintext).expect("cenc encrypt");
        assert_eq!(
            ciphertext, expected,
            "cenc must be real AES-128-CTR (NIST SP 800-38A F.5.1), not CBC"
        );

        let decrypted = handler.decrypt(&ciphertext).expect("cenc decrypt");
        assert_eq!(
            decrypted, plaintext,
            "CTR round-trip must recover plaintext"
        );
    }

    #[test]
    #[cfg(feature = "encryption")]
    fn test_cenc_ctr_preserves_length_unlike_cbc() {
        // CTR is a stream cipher: ciphertext length == plaintext length, even
        // for non-block-aligned input. Full-buffer CBC (the old behaviour)
        // would pad up to the next 16-byte boundary.
        let key = vec![0x24u8; 16];
        let iv = vec![0x68u8; 16];
        let plaintext = b"cenc CTR keeps exact length"; // 27 bytes, not a multiple of 16

        let key_info = KeyInfo::new(key, iv);
        let mut handler = EncryptionHandler::new(EncryptionMethod::Cenc);
        handler
            .set_key_info(key_info)
            .expect("set_key_info should succeed in test");

        let ciphertext = handler.encrypt(plaintext).expect("cenc encrypt");
        assert_eq!(
            ciphertext.len(),
            plaintext.len(),
            "CTR must not change length"
        );
        assert_ne!(&ciphertext[..], plaintext.as_ref());

        let decrypted = handler.decrypt(&ciphertext).expect("cenc decrypt");
        assert_eq!(decrypted, plaintext.to_vec());
    }

    #[test]
    #[cfg(feature = "encryption")]
    fn test_sample_aes_cbcs_pattern_and_roundtrip() {
        // 25 whole 16-byte blocks + a 7-byte tail. With the 1:9 pattern only
        // blocks 0, 10, 20 are encrypted; every other whole block and the tail
        // stay clear. This is the defining property the old full-buffer CBC
        // violated (it encrypted everything).
        let key = vec![0x11u8; 16];
        let iv = vec![0x22u8; 16];
        let total = 25 * 16 + 7;
        let plaintext: Vec<u8> = (0..total).map(|i| (i as u8).wrapping_mul(7)).collect();

        let key_info = KeyInfo::new(key, iv);
        let mut handler = EncryptionHandler::new(EncryptionMethod::SampleAes);
        handler
            .set_key_info(key_info)
            .expect("set_key_info should succeed in test");

        let ciphertext = handler.encrypt(&plaintext).expect("cbcs encrypt");
        assert_eq!(
            ciphertext.len(),
            plaintext.len(),
            "cbcs pattern encryption must preserve length (no padding)"
        );

        for block_idx in 0..25 {
            let range = block_idx * 16..block_idx * 16 + 16;
            if block_idx % 10 == 0 {
                assert_ne!(
                    ciphertext[range.clone()],
                    plaintext[range],
                    "crypt block {block_idx} must be encrypted"
                );
            } else {
                assert_eq!(
                    ciphertext[range.clone()],
                    plaintext[range],
                    "skip block {block_idx} must remain clear"
                );
            }
        }
        assert_eq!(
            ciphertext[25 * 16..],
            plaintext[25 * 16..],
            "trailing partial block must remain clear"
        );

        let decrypted = handler.decrypt(&ciphertext).expect("cbcs decrypt");
        assert_eq!(
            decrypted, plaintext,
            "cbcs pattern round-trip must recover plaintext"
        );
    }

    #[test]
    #[cfg(feature = "encryption")]
    fn test_sample_aes_differs_from_full_cbc() {
        // Guard against a regression to full-buffer CBC: with input longer than
        // one pattern cycle, at least one whole block must remain identical to
        // the plaintext (a skipped block), which full-buffer CBC never produces.
        let key = vec![0x33u8; 16];
        let iv = vec![0x44u8; 16];
        let plaintext = vec![0xA5u8; 16 * 12]; // 12 whole blocks > 1 cycle (10)

        let key_info = KeyInfo::new(key, iv);
        let mut handler = EncryptionHandler::new(EncryptionMethod::SampleAes);
        handler
            .set_key_info(key_info)
            .expect("set_key_info should succeed in test");

        let ciphertext = handler.encrypt(&plaintext).expect("cbcs encrypt");
        let has_clear_block =
            (0..12).any(|b| ciphertext[b * 16..b * 16 + 16] == plaintext[b * 16..b * 16 + 16]);
        assert!(
            has_clear_block,
            "cbcs must leave skip blocks in the clear (not full-buffer CBC)"
        );
    }

    // --- NAL-unit-aware subsample mapping ----------------------------------

    /// Builds a synthetic NAL unit payload: `header` byte followed by
    /// `extra_len` bytes of deterministic filler content.
    fn build_test_nal(header: u8, extra_len: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(1 + extra_len);
        v.push(header);
        v.extend((0..extra_len).map(|i| (i as u8).wrapping_mul(7).wrapping_add(3)));
        v
    }

    /// Concatenates NAL unit payloads into a length-prefixed sample buffer
    /// (4-byte big-endian length per unit, per ISO/IEC 14496-15).
    fn build_nal_sample(nals: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for nal in nals {
            out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            out.extend_from_slice(nal);
        }
        out
    }

    #[test]
    fn test_nal_subsamples_boundaries_and_byte_counts() {
        // SPS (type 7, non-VCL, 10-byte payload) + PPS (type 8, non-VCL,
        // 6-byte payload) + an IDR slice (type 5, VCL, 82-byte payload).
        let sps = build_test_nal(0x67, 9);
        let pps = build_test_nal(0x68, 5);
        let idr = build_test_nal(0x65, 81);
        let sample = build_nal_sample(&[&sps, &pps, &idr]);
        assert_eq!(
            sample.len(),
            14 + 10 + 86,
            "sanity-check the hand-built sample length"
        );

        let subsamples = nal_subsamples(&sample, 4, NalHeaderStyle::Avc)
            .expect("a well-formed length-prefixed NAL sample must parse");

        assert_eq!(
            subsamples,
            vec![
                SubsampleEntry {
                    clear_bytes: 14,
                    protected_bytes: 0
                },
                SubsampleEntry {
                    clear_bytes: 10,
                    protected_bytes: 0
                },
                SubsampleEntry {
                    clear_bytes: 36,
                    protected_bytes: 50
                },
            ],
            "SPS/PPS (non-VCL) must be fully clear subsamples; the IDR slice (VCL) \
             must get a 4-byte length + 32-byte clear lead (36 clear bytes) with \
             the remaining 82 - 32 = 50 bytes protected"
        );
    }

    #[test]
    fn test_nal_subsamples_hevc_header_style_classifies_vcl_vs_non_vcl() {
        // VPS (type 32, non-VCL): 2-byte header 0x40 0x01 + 8 bytes of body.
        let mut vps = vec![0x40u8, 0x01u8];
        vps.extend((0..8).map(|i| i as u8));
        // IDR_W_RADL slice (type 19, VCL): 2-byte header 0x26 0x01 + 60 bytes
        // of body (comfortably over the 32-byte clear lead).
        let mut idr = vec![0x26u8, 0x01u8];
        idr.extend((0..60).map(|i| (i as u8).wrapping_mul(5)));

        let sample = build_nal_sample(&[&vps, &idr]);
        let subsamples = nal_subsamples(&sample, 4, NalHeaderStyle::Hevc)
            .expect("a well-formed HEVC NAL sample must parse");

        assert_eq!(
            subsamples,
            vec![
                SubsampleEntry {
                    clear_bytes: (4 + vps.len()) as u32,
                    protected_bytes: 0
                },
                SubsampleEntry {
                    clear_bytes: 4 + 32,
                    protected_bytes: (idr.len() - 32) as u32
                },
            ],
            "HEVC VPS (type 32) must be fully clear; the IDR slice (type 19, VCL) \
             must get the 32-byte clear lead"
        );
    }

    #[test]
    fn test_nal_subsamples_rejects_truncated_length_prefix() {
        let sample = vec![0x00, 0x00, 0x00]; // 3 bytes: not enough for a 4-byte length field
        let result = nal_subsamples(&sample, 4, NalHeaderStyle::Avc);
        assert!(
            result.is_err(),
            "a truncated length prefix must be a real error, not silently ignored"
        );
    }

    #[test]
    fn test_nal_subsamples_rejects_declared_length_exceeding_buffer() {
        // Declares a NAL of length 100 but the buffer has only 5 bytes of payload.
        let mut sample = vec![0x00, 0x00, 0x00, 100u8];
        sample.extend(vec![0u8; 5]);
        let result = nal_subsamples(&sample, 4, NalHeaderStyle::Avc);
        assert!(
            result.is_err(),
            "a NAL length declared past the end of the buffer must be a real error"
        );
    }

    #[test]
    fn test_nal_subsamples_rejects_invalid_length_size() {
        let sample = vec![0u8; 8];
        assert!(nal_subsamples(&sample, 0, NalHeaderStyle::Avc).is_err());
        assert!(nal_subsamples(&sample, 5, NalHeaderStyle::Avc).is_err());
    }

    #[test]
    fn test_nal_subsamples_empty_buffer_yields_no_subsamples() {
        let result = nal_subsamples(&[], 4, NalHeaderStyle::Avc)
            .expect("an empty buffer is trivially valid (zero NAL units)");
        assert!(result.is_empty());
    }

    #[test]
    fn test_codec_structure_for_routes_known_codecs() {
        for codec in [
            "av1",
            "vp9",
            "vp8",
            "opus",
            "vorbis",
            "flac",
            "totally_unknown",
        ] {
            assert_eq!(
                codec_structure_for(codec),
                CodecStructure::Elementary,
                "'{codec}' must route to the whole-buffer path"
            );
        }
        for codec in ["h264", "avc", "avc1", "avc3", "H264"] {
            assert_eq!(
                codec_structure_for(codec),
                CodecStructure::NalLengthPrefixed {
                    length_size: 4,
                    header_style: NalHeaderStyle::Avc
                },
                "'{codec}' must route to the AVC NAL path"
            );
        }
        for codec in ["h265", "hevc", "hvc1", "hev1"] {
            assert_eq!(
                codec_structure_for(codec),
                CodecStructure::NalLengthPrefixed {
                    length_size: 4,
                    header_style: NalHeaderStyle::Hevc
                },
                "'{codec}' must route to the HEVC NAL path"
            );
        }
    }

    #[test]
    #[cfg(feature = "encryption")]
    fn test_nal_aware_cbcs_rejects_data_that_is_not_really_nal_structured() {
        // Regression for a real silent-cleartext bug: an all-zero buffer (what
        // `DashPackager`/`HlsPackager`'s placeholder segment generation
        // actually feeds today) parses "successfully" as a long run of
        // zero-length non-VCL NAL units -- every subsample fully clear -- so
        // encrypting it must refuse rather than return Ok(unmodified cleartext)
        // while claiming SAMPLE-AES protection was applied.
        let all_zero = vec![0u8; 1000];

        let key = vec![0x99u8; 16];
        let iv = vec![0xAAu8; 16];
        let key_info = KeyInfo::new(key, iv);
        let mut handler = EncryptionHandler::new(EncryptionMethod::SampleAes);
        handler
            .set_key_info(key_info)
            .expect("set_key_info should succeed in test");

        let result = handler.encrypt_for_codec(&all_zero, "h264");
        assert!(
            result.is_err(),
            "a buffer with no protected data anywhere must be a hard error, not a \
             silently-unencrypted Ok(..)"
        );

        // A genuinely mixed sample (SPS/PPS clear + a real IDR slice) must NOT
        // be rejected by the same guard.
        let sps = build_test_nal(0x67, 9);
        let pps = build_test_nal(0x68, 5);
        let idr = build_test_nal(0x65, 81);
        let real_sample = build_nal_sample(&[&sps, &pps, &idr]);
        assert!(
            handler.encrypt_for_codec(&real_sample, "h264").is_ok(),
            "a sample with real protected (VCL) data must not be rejected by the guard"
        );
    }

    #[test]
    #[cfg(feature = "encryption")]
    fn test_nal_aware_cbcs_pattern_block_alignment_and_roundtrip() {
        let sps = build_test_nal(0x67, 9); // 10 bytes, non-VCL -> fully clear
        let pps = build_test_nal(0x68, 5); // 6 bytes, non-VCL -> fully clear
        let idr = build_test_nal(0x65, 81); // 82 bytes, VCL -> 32-byte lead + 50 protected
        let sample = build_nal_sample(&[&sps, &pps, &idr]);
        assert_eq!(sample.len(), 110);

        let key = vec![0x77u8; 16];
        let iv = vec![0x88u8; 16];
        let key_info = KeyInfo::new(key, iv);
        let mut handler = EncryptionHandler::new(EncryptionMethod::SampleAes);
        handler
            .set_key_info(key_info)
            .expect("set_key_info should succeed in test");

        let ciphertext = handler
            .encrypt_for_codec(&sample, "h264")
            .expect("NAL-aware cbcs encrypt should succeed");
        assert_eq!(
            ciphertext.len(),
            sample.len(),
            "cbcs pattern encryption must preserve length"
        );

        // SPS + PPS (fully clear, non-VCL): bytes [0, 24) unchanged.
        assert_eq!(
            &ciphertext[0..24],
            &sample[0..24],
            "non-VCL NAL units must stay fully clear"
        );

        // IDR's length prefix + 32-byte clear lead: bytes [24, 60) unchanged.
        assert_eq!(
            &ciphertext[24..60],
            &sample[24..60],
            "IDR NAL header + slice header clear lead must stay clear"
        );

        // First 16-byte block of the protected region [60, 76): must be encrypted
        // (the pattern always resets to the crypt state at a subsample's start).
        assert_ne!(
            &ciphertext[60..76],
            &sample[60..76],
            "first protected block must be encrypted"
        );

        // Skip blocks 1-2 of the protected region [76, 108): stay clear (1:9 pattern).
        assert_eq!(
            &ciphertext[76..108],
            &sample[76..108],
            "skip blocks within the pattern must stay clear"
        );

        // Trailing partial block [108, 110) (< 16 bytes): stays clear.
        assert_eq!(
            &ciphertext[108..110],
            &sample[108..110],
            "trailing partial block must stay clear"
        );

        let decrypted = handler
            .decrypt_for_codec(&ciphertext, "h264")
            .expect("NAL-aware cbcs decrypt should succeed");
        assert_eq!(
            decrypted, sample,
            "NAL-aware cbcs round-trip must recover the original sample exactly"
        );
    }

    #[test]
    #[cfg(feature = "encryption")]
    fn test_encrypt_for_codec_elementary_codec_matches_whole_buffer_path() {
        // For this tree's actual codecs (av1/vp9/vp8/opus/vorbis/flac),
        // encrypt_for_codec must be byte-for-byte identical to the pre-existing
        // encrypt() path: adding NAL-awareness must not change behavior for any
        // codec this crate actually emits.
        let key = vec![0x55u8; 16];
        let iv = vec![0x66u8; 16];
        let plaintext: Vec<u8> = (0..300).map(|i| (i as u8).wrapping_mul(11)).collect();

        let key_info = KeyInfo::new(key, iv);
        let mut handler = EncryptionHandler::new(EncryptionMethod::SampleAes);
        handler
            .set_key_info(key_info)
            .expect("set_key_info should succeed in test");

        let via_generic = handler.encrypt(&plaintext).expect("encrypt");
        for codec in ["av1", "vp9", "vp8", "opus", "vorbis", "flac"] {
            let via_codec = handler
                .encrypt_for_codec(&plaintext, codec)
                .expect("encrypt_for_codec");
            assert_eq!(
                via_codec, via_generic,
                "'{codec}' must match the whole-buffer path exactly"
            );
        }
    }
}
