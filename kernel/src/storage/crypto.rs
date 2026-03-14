//! Device-level transparent encryption — AES-256-GCM.
//!
//! Every block is encrypted before reaching the VirtIO-blk driver and
//! decrypted on read. This is the lowest encryption layer, protecting
//! against physical access to the storage medium.
//!
//! Nonce: [random_prefix(4B) | counter(8B)] — global monotonic counter
//! persisted in superblock, advanced +1000 on crash recovery per §6.1.1.
//!
//! Per spaces.md §4.10 Device-Level Transparent Encryption.

use core::sync::atomic::{AtomicU64, Ordering};

use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::Aes256Gcm;
use sha2::{Digest, Sha256};
use shared::storage::{StorageError, ENCRYPTION_OVERHEAD};

/// Crash recovery nonce gap — advance counter by this much on init to guarantee
/// no nonce reuse even after unclean shutdown.
const CRASH_RECOVERY_GAP: u64 = 1000;

/// DeviceKeyManager — manages the AES-256-GCM device encryption key and nonce counter.
#[allow(dead_code)]
pub struct DeviceKeyManager {
    /// The AES-256-GCM cipher instance (holds the key).
    cipher: Aes256Gcm,
    /// Key epoch (for future key rotation; single key in Phase 4).
    pub epoch: u64,
    /// Global monotonic nonce counter.
    nonce_counter: AtomicU64,
    /// Random prefix for nonce (first 4 bytes).
    pub random_prefix: u32,
}

impl DeviceKeyManager {
    /// Create a DeviceKeyManager from a passphrase.
    ///
    /// Key derivation: SHA-256(passphrase + "aios-device-key-salt") → 32-byte key.
    /// This is a placeholder for Argon2id (Phase 24).
    pub fn from_passphrase(passphrase: &[u8], initial_counter: u64, random_prefix: u32) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(passphrase);
        hasher.update(b"aios-device-key-salt");
        let key_bytes = hasher.finalize();

        let key = GenericArray::from_slice(&key_bytes);
        let cipher = Aes256Gcm::new(key);

        // Advance counter by crash recovery gap to guarantee no nonce reuse.
        let safe_counter = initial_counter.saturating_add(CRASH_RECOVERY_GAP);

        Self {
            cipher,
            epoch: 1,
            nonce_counter: AtomicU64::new(safe_counter),
            random_prefix,
        }
    }

    /// Get the current nonce counter value (for superblock persistence).
    pub fn nonce_counter(&self) -> u64 {
        self.nonce_counter.load(Ordering::Relaxed)
    }

    /// Build a 12-byte nonce: [random_prefix(4B) | counter(8B)].
    fn next_nonce(&self) -> [u8; 12] {
        let counter = self.nonce_counter.fetch_add(1, Ordering::Relaxed);
        let mut nonce = [0u8; 12];
        nonce[..4].copy_from_slice(&self.random_prefix.to_le_bytes());
        nonce[4..].copy_from_slice(&counter.to_le_bytes());
        nonce
    }

    /// Encrypt a plaintext block in-place.
    ///
    /// On-disk format: `[nonce(12B) | ciphertext | tag(16B)]`
    ///
    /// `buf` must have room for plaintext_len + ENCRYPTION_OVERHEAD bytes.
    /// Returns the total encrypted size (plaintext_len + 28).
    pub fn encrypt(&self, plaintext: &[u8], buf: &mut [u8]) -> Result<usize, StorageError> {
        let total = plaintext.len() + ENCRYPTION_OVERHEAD;
        if buf.len() < total {
            return Err(StorageError::IoError);
        }

        let nonce_bytes = self.next_nonce();
        let nonce = GenericArray::from_slice(&nonce_bytes);

        // Copy nonce to output.
        buf[..12].copy_from_slice(&nonce_bytes);

        // Copy plaintext after nonce.
        buf[12..12 + plaintext.len()].copy_from_slice(plaintext);

        // Encrypt in-place (ciphertext replaces plaintext, tag appended).
        let tag = self
            .cipher
            .encrypt_in_place_detached(nonce, b"", &mut buf[12..12 + plaintext.len()])
            .map_err(|_| StorageError::IoError)?;

        // Append tag after ciphertext.
        buf[12 + plaintext.len()..total].copy_from_slice(&tag);

        Ok(total)
    }

    /// Decrypt an encrypted block.
    ///
    /// Input format: `[nonce(12B) | ciphertext | tag(16B)]`
    ///
    /// Decrypts ciphertext in-place within `encrypted` and returns the plaintext length.
    /// After return, `encrypted[12..12+plaintext_len]` contains the decrypted data.
    pub fn decrypt(&self, encrypted: &mut [u8]) -> Result<usize, StorageError> {
        if encrypted.len() < ENCRYPTION_OVERHEAD {
            return Err(StorageError::DecryptionFailed);
        }

        let plaintext_len = encrypted.len() - ENCRYPTION_OVERHEAD;
        // Copy nonce to local array to avoid conflicting borrows with mutable decrypt below.
        let mut nonce_bytes = [0u8; 12];
        nonce_bytes.copy_from_slice(&encrypted[..12]);
        let nonce = GenericArray::from_slice(&nonce_bytes);

        // Extract tag from end.
        let tag_start = 12 + plaintext_len;
        let mut tag = [0u8; 16];
        tag.copy_from_slice(&encrypted[tag_start..tag_start + 16]);
        let tag = GenericArray::from_slice(&tag);

        // Decrypt in-place.
        self.cipher
            .decrypt_in_place_detached(nonce, b"", &mut encrypted[12..12 + plaintext_len], tag)
            .map_err(|_| StorageError::DecryptionFailed)?;

        Ok(plaintext_len)
    }
}
