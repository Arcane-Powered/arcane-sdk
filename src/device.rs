use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::SdkError;
use crate::paths::machine_id_path;

pub(crate) fn machine_id() -> Result<String, SdkError> {
    let path = machine_id_path()?;
    if let Ok(existing) = fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    write_private_file(&path, id.as_bytes())?;
    Ok(id)
}

/// Stable per-machine fingerprint: first 16 bytes of SHA-256(`machine_id`), hex.
pub(crate) fn device_hash() -> Result<String, SdkError> {
    let mid = machine_id()?;
    let mut hasher = Sha256::new();
    hasher.update(mid.as_bytes());
    let digest = hasher.finalize();
    Ok(hex::encode(&digest[..16]))
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

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), SdkError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            SdkError::internal(format!("Could not create the Arcane DRM directory: {e}"))
                .with_hint("Check write permissions on the application data directory.")
                .with_context("path", parent.display())
        })?;
    }
    let mut file = fs::File::create(path).map_err(|e| {
        SdkError::internal(format!("Could not create the device identity file: {e}"))
            .with_hint("Check write permissions on the Arcane DRM directory.")
            .with_context("path", path.display())
    })?;
    file.write_all(bytes).map_err(|e| {
        SdkError::internal(format!("Could not write the device identity file: {e}"))
            .with_context("path", path.display())
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
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
