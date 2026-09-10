//! The DRM device fingerprint this machine presents for an account.
//!
//! The hash is **not** a property of the hardware: it is
//! `sha256(account_key.public_key_fpr)` truncated to 16 bytes, exactly what the
//! Arcane desktop app sends the backend when it mints a ticket, so the `dev`
//! claim and the local value are the same thing by construction.
//!
//! The desktop derives it from a key sealed in the OS keyring, which a game
//! process has no business reading. So it publishes the **pre-images** — public
//! values, no secret — in `device/{user_id}.json`, and the SDK rehashes them
//! itself rather than trusting a hash handed to it. See `DESKTOP_CONTRACT.md` §12.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::SdkError;
use crate::paths::device_file_path;

/// A 32-byte SPKI fingerprint, hex. Hashed over the decoded bytes.
const KIND_ACCOUNT_KEY_FPR: &str = "account_key_fpr";
/// The legacy keyring device id, for accounts with no account key. Hashed over
/// the string as written.
const KIND_DEVICE_ID: &str = "device_id";

#[derive(Debug, Clone, Deserialize)]
struct Fingerprint {
    kind: String,
    value: String,
}

#[derive(Debug, Clone, Deserialize)]
struct DeviceFile {
    #[serde(default)]
    fingerprints: Vec<Fingerprint>,
}

/// Every device hash this machine can present for `user_id`, most current first.
///
/// More than one is normal: an account that rotated its key keeps the older
/// fingerprint listed so a ticket minted before the rotation still verifies
/// offline. The list only says what this machine *can* present — the ticket's
/// signed `dev` claim is what decides.
pub(crate) fn device_hashes(user_id: &str) -> Result<Vec<String>, SdkError> {
    let path = device_file_path(user_id)?;
    let raw = fs::read_to_string(&path).map_err(|e| {
        SdkError::device_mismatch(
            "This machine has no Arcane device fingerprint for the signed-in account.",
        )
        .with_hint(
            "Open the Arcane desktop app once while online and signed in to this account — \
             it writes the fingerprint alongside the ticket.",
        )
        .with_context("user_id", user_id)
        .with_context("path", path.display())
        .with_context("detail", e)
    })?;
    let file: DeviceFile = serde_json::from_str(&raw).map_err(|e| {
        SdkError::device_mismatch(format!(
            "The local device fingerprint file is not valid JSON: {e}"
        ))
        .with_hint("Delete the file and let Arcane desktop rewrite it.")
        .with_context("path", path.display())
    })?;

    let mut hashes = Vec::new();
    for fingerprint in &file.fingerprints {
        // An unknown `kind` is a newer desktop describing a fingerprint this SDK
        // cannot rehash. Skipping it is right: guessing would invent a hash.
        if let Some(hash) = hash_fingerprint(fingerprint) {
            if !hashes.contains(&hash) {
                hashes.push(hash);
            }
        }
    }

    if hashes.is_empty() {
        return Err(SdkError::device_mismatch(
            "The local device fingerprint file lists nothing this SDK can verify.",
        )
        .with_hint("Update the SDK, or sign in again in Arcane desktop so it rewrites the file.")
        .with_context("user_id", user_id)
        .with_context("path", path.display())
        .with_context("entries", file.fingerprints.len()));
    }
    Ok(hashes)
}

/// The hash this client reports when nothing was verified against a ticket —
/// the DRM-disabled path. Best-effort by design: a title that does not enforce
/// DRM must not fail to start because a fingerprint file is missing.
pub(crate) fn primary_device_hash(user_id: &str) -> Option<String> {
    device_hashes(user_id).ok()?.into_iter().next()
}

/// Mirrors the desktop's own derivation. Any drift here is a device mismatch on
/// every machine, so the two must be read together.
fn hash_fingerprint(fingerprint: &Fingerprint) -> Option<String> {
    let value = fingerprint.value.trim();
    let digest = match fingerprint.kind.as_str() {
        KIND_ACCOUNT_KEY_FPR => {
            let bytes = hex::decode(value).ok()?;
            if bytes.len() != 32 {
                return None;
            }
            Sha256::digest(bytes)
        }
        KIND_DEVICE_ID => Sha256::digest(value.as_bytes()),
        _ => return None,
    };
    Some(hex::encode(&digest[..16]))
}

/// Shorten a device hash for error context — enough to compare, not the whole value.
pub(crate) fn short_hash(hash: &str) -> String {
    let head: String = hash.chars().take(12).collect();
    if hash.chars().count() > 12 {
        format!("{head}…")
    } else {
        head
    }
}

/// Render a candidate list for error context, so a mismatch names both sides.
pub(crate) fn short_hashes(hashes: &[String]) -> String {
    hashes
        .iter()
        .map(|h| short_hash(h))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CachedTicketFile {
    pub ticket: String,
    #[allow(dead_code)]
    pub cached_at: String,
    #[allow(dead_code)]
    pub expires_at: String,
    #[allow(dead_code)]
    pub game_id: String,
    pub user_id: String,
    pub device_hash: String,
    pub drm_enabled: bool,
    #[serde(default)]
    pub last_seen_wall_time: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_key_fpr_hashes_the_decoded_bytes() {
        let fpr = "11".repeat(32);
        let expected = hex::encode(&Sha256::digest(vec![0x11u8; 32])[..16]);
        let hash = hash_fingerprint(&Fingerprint {
            kind: KIND_ACCOUNT_KEY_FPR.into(),
            value: fpr,
        });
        assert_eq!(hash.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn device_id_hashes_the_string() {
        let expected = hex::encode(&Sha256::digest(b"0xabc")[..16]);
        let hash = hash_fingerprint(&Fingerprint {
            kind: KIND_DEVICE_ID.into(),
            value: "0xabc".into(),
        });
        assert_eq!(hash.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn rejects_a_fingerprint_that_is_not_32_bytes() {
        for value in ["", "abcd", &"22".repeat(31), "zz"] {
            assert_eq!(
                hash_fingerprint(&Fingerprint {
                    kind: KIND_ACCOUNT_KEY_FPR.into(),
                    value: value.into(),
                }),
                None
            );
        }
    }

    #[test]
    fn skips_a_kind_it_cannot_rehash() {
        assert_eq!(
            hash_fingerprint(&Fingerprint {
                kind: "tpm_attestation".into(),
                value: "33".repeat(32),
            }),
            None
        );
    }
}
