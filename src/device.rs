use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::SdkError;
use crate::paths::{drm_data_root, machine_id_path};

pub(crate) fn machine_id() -> Result<String, SdkError> {
    let path = machine_id_path().map_err(SdkError::Io)?;
    if let Ok(existing) = fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| SdkError::Io(e.to_string()))?;
    }
    let id = uuid::Uuid::new_v4().to_string();
    write_private_file(&path, id.as_bytes())?;
    Ok(id)
}

pub(crate) fn device_hash() -> Result<String, SdkError> {
    let mid = machine_id()?;
    let mut hasher = Sha256::new();
    hasher.update(mid.as_bytes());
    let digest = hasher.finalize();
    Ok(hex::encode(&digest[..16]))
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), SdkError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| SdkError::Io(e.to_string()))?;
    }
    let mut file = fs::File::create(path).map_err(|e| SdkError::Io(e.to_string()))?;
    file.write_all(bytes).map_err(|e| SdkError::Io(e.to_string()))?;
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
    pub cached_at: String,
    pub expires_at: String,
    pub game_id: String,
    pub user_id: String,
    pub device_hash: String,
    pub drm_enabled: bool,
    #[serde(default)]
    pub last_seen_wall_time: Option<i64>,
}

#[allow(dead_code)]
pub(crate) fn drm_root() -> Result<PathBuf, SdkError> {
    drm_data_root().map_err(SdkError::Io)
}
