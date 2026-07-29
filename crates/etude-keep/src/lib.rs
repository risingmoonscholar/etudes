//! Journal encryption (M7).
//!
//! Isolated in its own crate so that `etude-core` — the classification engine —
//! keeps **zero dependencies**. The no-network claim is about the engine, and
//! keeping the engine dependency-free is what makes that claim cheap to check.
//!
//! # Design
//!
//! - XChaCha20-Poly1305 from RustCrypto. Not hand-rolled; a hand-rolled cipher
//!   in a privacy tool would be a worse bug than no cipher at all.
//! - A 256-bit key lives in the **login keychain**, never on disk.
//! - The key reaches `security` over **stdin**, never `argv`, because process
//!   arguments are readable with `ps`.
//! - A fresh random 192-bit nonce per write. XChaCha's nonce is large enough
//!   that random generation is safe without a counter.
//! - The plaintext is padded to a 4 KiB boundary before sealing, so ciphertext
//!   length does not reveal how many files were organised.

use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::{Aead, KeyInit, OsRng};
use chacha20poly1305::{AeadCore, Key, XChaCha20Poly1305, XNonce};
use std::io::Write;
use std::process::{Command, Stdio};

const SERVICE: &str = "sweep-journal-key";
const ACCOUNT: &str = "sweep";
const MAGIC: &[u8] = b"SWEEPJ1\0";
const PAD_TO: usize = 4096;

#[derive(Debug)]
pub enum KeepError {
    Keychain(String),
    Crypto(&'static str),
    Malformed(&'static str),
}

impl std::fmt::Display for KeepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeepError::Keychain(m) => write!(f, "keychain: {m}"),
            // Never distinguish "wrong key" from "tampered" to a caller that
            // might print it; both mean the same thing operationally.
            KeepError::Crypto(m) => write!(f, "cannot decrypt journal: {m}"),
            KeepError::Malformed(m) => write!(f, "journal malformed: {m}"),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

/// Read the key from the keychain, creating one on first use.
pub fn key() -> Result<[u8; 32], KeepError> {
    if let Some(k) = read_key()? {
        return Ok(k);
    }
    let mut fresh = [0u8; 32];
    getrandom_fill(&mut fresh)?;
    store_key(&fresh)?;
    // Read back rather than trusting the write: a key we cannot retrieve would
    // produce a journal nobody can ever decrypt, including the owner.
    match read_key()? {
        Some(k) if k == fresh => Ok(k),
        _ => Err(KeepError::Keychain("key did not survive a write/read round trip".into())),
    }
}

fn getrandom_fill(buf: &mut [u8]) -> Result<(), KeepError> {
    // OsRng draws from the OS CSPRNG. `try_fill_bytes` rather than
    // `fill_bytes` so an entropy failure surfaces instead of panicking.
    OsRng.try_fill_bytes(buf).map_err(|_| KeepError::Crypto("no secure randomness available"))
}

fn read_key() -> Result<Option<[u8; 32]>, KeepError> {
    let out = Command::new("security")
        .args(["find-generic-password", "-a", ACCOUNT, "-s", SERVICE, "-w"])
        .output()
        .map_err(|e| KeepError::Keychain(e.kind().to_string()))?;
    if !out.status.success() {
        return Ok(None); // not found is not an error; it means first run
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let bytes = unhex(&text).ok_or(KeepError::Keychain("stored key is not hex".into()))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| KeepError::Keychain("stored key is the wrong length".into()))?;
    Ok(Some(arr))
}

fn store_key(k: &[u8; 32]) -> Result<(), KeepError> {
    // The secret goes over stdin. `security` prompts for the value and then for
    // a confirmation, so it is written twice. It must never be an argument:
    // arguments are visible to `ps`.
    let mut child = Command::new("security")
        .args(["add-generic-password", "-a", ACCOUNT, "-s", SERVICE, "-U", "-w"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| KeepError::Keychain(e.kind().to_string()))?;

    let encoded = hex(k);
    {
        let stdin = child.stdin.as_mut().ok_or(KeepError::Keychain("no stdin".into()))?;
        writeln!(stdin, "{encoded}").map_err(|e| KeepError::Keychain(e.kind().to_string()))?;
        writeln!(stdin, "{encoded}").map_err(|e| KeepError::Keychain(e.kind().to_string()))?;
    }
    let status = child.wait().map_err(|e| KeepError::Keychain(e.kind().to_string()))?;
    if !status.success() {
        return Err(KeepError::Keychain("could not store the key".into()));
    }
    Ok(())
}

/// Remove the key. After this, existing journals are unreadable by anyone.
pub fn destroy_key() {
    let _ = Command::new("security")
        .args(["delete-generic-password", "-a", ACCOUNT, "-s", SERVICE])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Seal plaintext. Layout: `MAGIC | nonce(24) | ciphertext`.
pub fn seal(key_bytes: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, KeepError> {
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key_bytes));
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);

    // Pad before sealing so ciphertext length reveals nothing about how much
    // work was done. Length is stored in the first 4 bytes of the plaintext.
    let mut padded = Vec::with_capacity(plaintext.len() + 4 + PAD_TO);
    padded.extend_from_slice(&(plaintext.len() as u32).to_le_bytes());
    padded.extend_from_slice(plaintext);
    let target = padded.len().div_ceil(PAD_TO) * PAD_TO;
    padded.resize(target, 0);

    let ct =
        cipher.encrypt(&nonce, padded.as_ref()).map_err(|_| KeepError::Crypto("seal failed"))?;

    let mut out = Vec::with_capacity(MAGIC.len() + nonce.len() + ct.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Open a sealed journal. Fails closed on any tampering.
pub fn open(key_bytes: &[u8; 32], sealed: &[u8]) -> Result<Vec<u8>, KeepError> {
    if sealed.len() < MAGIC.len() + 24 || &sealed[..MAGIC.len()] != MAGIC {
        return Err(KeepError::Malformed("bad header"));
    }
    let nonce = XNonce::from_slice(&sealed[MAGIC.len()..MAGIC.len() + 24]);
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key_bytes));
    let padded = cipher
        .decrypt(nonce, &sealed[MAGIC.len() + 24..])
        .map_err(|_| KeepError::Crypto("wrong key or the file was altered"))?;

    if padded.len() < 4 {
        return Err(KeepError::Malformed("truncated"));
    }
    let len = u32::from_le_bytes([padded[0], padded[1], padded[2], padded[3]]) as usize;
    if 4 + len > padded.len() {
        return Err(KeepError::Malformed("length prefix exceeds payload"));
    }
    Ok(padded[4..4 + len].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    const K: [u8; 32] = [7u8; 32];

    #[test]
    fn round_trips() {
        let msg = b"mv\t/Users/x/Desktop/W2_2024.pdf\t/Users/x/Desktop/tax/W2_2024.pdf\n";
        let sealed = seal(&K, msg).expect("seal");
        assert_eq!(open(&K, &sealed).expect("open"), msg);
    }

    #[test]
    fn the_plaintext_never_appears_in_the_ciphertext() {
        // The whole point: a curious application reading the file learns nothing.
        let msg = b"/Users/x/Desktop/SSN_card_scan.jpg";
        let sealed = seal(&K, msg).expect("seal");
        let hay = String::from_utf8_lossy(&sealed);
        assert!(!hay.contains("SSN_card_scan"), "filename survived into the ciphertext");
    }

    #[test]
    fn length_is_padded_so_size_does_not_leak_volume() {
        let small = seal(&K, b"x").expect("seal");
        let bigger = seal(&K, &vec![b'x'; 2000]).expect("seal");
        assert_eq!(small.len(), bigger.len(), "ciphertext size leaks payload size");
    }

    #[test]
    fn tampering_fails_closed() {
        let mut sealed = seal(&K, b"hello").expect("seal");
        let last = sealed.len() - 1;
        sealed[last] ^= 0xff;
        assert!(open(&K, &sealed).is_err(), "tampered ciphertext was accepted");
    }

    #[test]
    fn a_wrong_key_cannot_open_it() {
        let sealed = seal(&K, b"hello").expect("seal");
        assert!(open(&[9u8; 32], &sealed).is_err(), "wrong key opened the journal");
    }

    #[test]
    fn nonces_differ_between_writes() {
        let a = seal(&K, b"same").expect("seal");
        let b = seal(&K, b"same").expect("seal");
        assert_ne!(a, b, "identical ciphertext for identical input — nonce reuse");
    }
}
