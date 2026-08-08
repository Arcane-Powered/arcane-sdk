use std::fs;
use std::path::PathBuf;

use crate::device::CachedTicketFile;
use crate::error::SdkError;

pub(crate) fn drm_data_root() -> Result<PathBuf, String> {
    let base = dirs::data_dir().ok_or_else(|| "No application data directory".to_string())?;
    Ok(base.join("Arcane Powered").join("drm"))
}

pub(crate) fn machine_id_path() -> Result<PathBuf, String> {
    Ok(drm_data_root()?.join("machine_id"))
}

pub(crate) fn jwks_path() -> Result<PathBuf, String> {
    Ok(drm_data_root()?.join("jwks.json"))
}

#[allow(dead_code)] // documents on-disk layout; load_ticket_file scans by game id
pub(crate) fn ticket_path(user_id: &str, game_id: &str) -> Result<PathBuf, String> {
    Ok(drm_data_root()?
        .join("tickets")
        .join(user_id)
        .join(format!("{game_id}.ticket")))
}

pub(crate) fn load_cached_drm_flag(game_id: &str) -> Option<bool> {
    let path = drm_data_root().ok()?.join("flags").join(format!("{game_id}.json"));
    let raw = fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("drm_enabled")?.as_bool()
}

/// Find a ticket file for `game_id` by scanning ticket dirs (user id may be cloud UUID).
pub(crate) fn load_ticket_file(game_id: &str) -> Result<CachedTicketFile, SdkError> {
    let root = drm_data_root()
        .map_err(SdkError::Io)?
        .join("tickets");
    if !root.exists() {
        return Err(SdkError::TicketMissing(
            "No local ownership tickets. Connect online once via the Arcane app.".into(),
        ));
    }
    let users = fs::read_dir(&root).map_err(|e| SdkError::Io(e.to_string()))?;
    for user in users.flatten() {
        let path = user.path().join(format!("{game_id}.ticket"));
        if !path.exists() {
            continue;
        }
        let raw = fs::read_to_string(&path).map_err(|e| SdkError::Io(e.to_string()))?;
        let file: CachedTicketFile = serde_json::from_str(&raw)
            .map_err(|e| SdkError::TicketInvalid(format!("corrupt ticket file: {e}")))?;
        return Ok(file);
    }
    Err(SdkError::TicketMissing(format!(
        "No ownership ticket cached for game `{game_id}`."
    )))
}
