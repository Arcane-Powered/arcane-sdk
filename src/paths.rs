//! DRM data root layout, session lookup, and ticket resolution.
//!
//! ```text
//! {app_data}/Arcane Powered/drm/
//! ├── machine_id
//! ├── jwks.json
//! ├── session.json                     written by Arcane desktop on sign-in/out
//! ├── flags/{game_id}.json
//! └── tickets/{user_id}/{game_id}.ticket
//! ```
//!
//! `session.json` is what lets the SDK pick *this* account's ticket instead of
//! whichever one happens to be on disk first. See [`resolve_ticket`].

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::device::CachedTicketFile;
use crate::error::SdkError;

/// Overrides the DRM data root. Intended for tests and QA — a game should never
/// set it.
pub(crate) const DRM_ROOT_ENV: &str = "ARCANE_DRM_ROOT";

pub(crate) fn drm_data_root() -> Result<PathBuf, SdkError> {
    if let Some(raw) = env::var_os(DRM_ROOT_ENV) {
        if !raw.is_empty() {
            return Ok(PathBuf::from(raw));
        }
    }
    let base = dirs::data_dir().ok_or_else(|| {
        SdkError::internal("Could not resolve the OS application data directory.")
            .with_hint(format!(
                "Set {DRM_ROOT_ENV} to an explicit directory if this platform has no standard app-data path."
            ))
    })?;
    Ok(base.join("Arcane Powered").join("drm"))
}

pub(crate) fn machine_id_path() -> Result<PathBuf, SdkError> {
    Ok(drm_data_root()?.join("machine_id"))
}

pub(crate) fn jwks_path() -> Result<PathBuf, SdkError> {
    Ok(drm_data_root()?.join("jwks.json"))
}

pub(crate) fn session_path() -> Result<PathBuf, SdkError> {
    Ok(drm_data_root()?.join("session.json"))
}

pub(crate) fn tickets_root() -> Result<PathBuf, SdkError> {
    Ok(drm_data_root()?.join("tickets"))
}

pub(crate) fn ticket_path(user_id: &str, game_id: &str) -> Result<PathBuf, SdkError> {
    Ok(tickets_root()?
        .join(user_id)
        .join(format!("{game_id}.ticket")))
}

#[derive(Debug, Deserialize)]
struct SessionFile {
    #[serde(default)]
    user_id: Option<String>,
}

/// Who Arcane desktop last recorded as signed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionState {
    SignedIn(String),
    /// `session.json` exists and explicitly records no account.
    SignedOut,
    /// No `session.json` — an Arcane build that predates it, or a fresh install.
    Unknown,
}

pub(crate) fn load_session() -> SessionState {
    let Ok(path) = session_path() else {
        return SessionState::Unknown;
    };
    let Ok(raw) = fs::read_to_string(&path) else {
        return SessionState::Unknown;
    };
    let Ok(session) = serde_json::from_str::<SessionFile>(&raw) else {
        return SessionState::Unknown;
    };
    match session.user_id {
        Some(user_id) if !user_id.trim().is_empty() => {
            SessionState::SignedIn(user_id.trim().to_string())
        }
        _ => SessionState::SignedOut,
    }
}

pub(crate) fn load_cached_drm_flag(game_id: &str) -> Option<bool> {
    let path = drm_data_root()
        .ok()?
        .join("flags")
        .join(format!("{game_id}.json"));
    let raw = fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value.get("drm_enabled")?.as_bool()
}

/// A ticket file plus where it came from — the path feeds error context.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedTicket {
    pub file: CachedTicketFile,
    pub path: PathBuf,
}

fn read_ticket_at(path: &Path) -> Result<ResolvedTicket, SdkError> {
    let raw = fs::read_to_string(path).map_err(|e| {
        SdkError::internal(format!("Could not read the cached ownership ticket: {e}"))
            .with_hint("Check read permissions on the Arcane DRM directory.")
            .with_context("path", path.display())
    })?;
    let file: CachedTicketFile = serde_json::from_str(&raw).map_err(|e| {
        SdkError::ticket_invalid(format!("The cached ownership ticket file is corrupt: {e}"))
            .with_hint("Delete the file and let Arcane desktop mint a fresh ticket.")
            .with_context("path", path.display())
    })?;
    Ok(ResolvedTicket {
        file,
        path: path.to_path_buf(),
    })
}

/// Find the ticket belonging to the **currently signed-in** account.
///
/// Resolution order:
/// 1. `session.json` names a user → read exactly `tickets/{user_id}/{game_id}.ticket`.
///    No fallback: another account's ticket must never satisfy this account.
/// 2. `session.json` records a signed-out state → `not_authenticated`.
/// 3. No `session.json` (older Arcane desktop) → scan `tickets/*/`. Exactly one
///    match is used; several matches are `ambiguous_session` rather than a guess.
pub(crate) fn resolve_ticket(game_id: &str) -> Result<ResolvedTicket, SdkError> {
    match load_session() {
        SessionState::SignedIn(user_id) => {
            let path = ticket_path(&user_id, game_id)?;
            if !path.exists() {
                return Err(SdkError::ticket_missing(
                    "No ownership ticket is cached for this title on the signed-in account.",
                )
                .with_hint(
                    "Open the Arcane desktop app once while online so it can mint a ticket \
                     for this account.",
                )
                .with_context("game_id", game_id)
                .with_context("user_id", &user_id)
                .with_context("expected_path", path.display()));
            }
            read_ticket_at(&path)
        }
        SessionState::SignedOut => Err(SdkError::not_authenticated(
            "Nobody is signed in to Arcane on this machine.",
        )
        .with_hint("Sign in to the Arcane desktop app, then retry.")
        .with_context("game_id", game_id)
        .with_context(
            "session_path",
            session_path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "<unresolved>".into()),
        )),
        SessionState::Unknown => resolve_ticket_by_scan(game_id),
    }
}

/// Compatibility path for Arcane desktop builds that do not write `session.json`.
fn resolve_ticket_by_scan(game_id: &str) -> Result<ResolvedTicket, SdkError> {
    let root = tickets_root()?;
    if !root.exists() {
        return Err(
            SdkError::ticket_missing("No ownership tickets are cached on this machine.")
                .with_hint("Sign in to the Arcane desktop app once while online, then retry.")
                .with_context("game_id", game_id)
                .with_context("tickets_root", root.display()),
        );
    }

    let entries = fs::read_dir(&root).map_err(|e| {
        SdkError::internal(format!("Could not list cached ownership tickets: {e}"))
            .with_hint("Check read permissions on the Arcane DRM directory.")
            .with_context("tickets_root", root.display())
    })?;

    let mut matches: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let candidate = entry.path().join(format!("{game_id}.ticket"));
        if candidate.exists() {
            matches.push(candidate);
        }
    }

    match matches.len() {
        0 => Err(SdkError::ticket_missing(format!(
            "No ownership ticket is cached for `{game_id}`."
        ))
        .with_hint("Open the Arcane desktop app once while online, then retry.")
        .with_context("game_id", game_id)
        .with_context("tickets_root", root.display())),
        1 => read_ticket_at(&matches[0]),
        n => Err(SdkError::ambiguous_session(format!(
            "{n} accounts hold a ticket for this title on this machine, and Arcane has not \
             recorded which one is signed in."
        ))
        .with_hint(
            "Open the Arcane desktop app once so it records the active session, then retry. \
             This needs an Arcane build that writes session.json.",
        )
        .with_context("game_id", game_id)
        .with_context("tickets_root", root.display())
        .with_context("candidates", n)),
    }
}
