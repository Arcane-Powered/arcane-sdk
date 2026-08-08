//! Arcane game SDK core — ownership ticket verification.
//!
//! Games call [`arcane_init`] at launch. The SDK verifies a locally cached
//! ownership ticket when possible. If the ticket is missing or expired, it
//! asks the Arcane desktop app (loopback `127.0.0.1:39284`) to refresh online,
//! opening the app via deep link when needed.

mod desktop;
mod device;
mod error;
mod paths;
mod ticket;

pub use error::{OwnershipStatus, SdkError};
pub use ticket::check_ownership_offline;

/// High-level init policy used by game engines.
///
/// - If cached `drm_enabled` is false → Ok (no ticket required).
/// - If true / unknown → require a valid ownership ticket.
/// - On `ticket_missing` / `ticket_expired`, contact Arcane desktop to refresh
///   (may launch the app), then re-verify offline.
pub fn arcane_init(game_id: &str) -> Result<OwnershipStatus, SdkError> {
    match paths::load_cached_drm_flag(game_id) {
        Some(false) => return Ok(OwnershipStatus::DrmDisabled),
        _ => {}
    }

    match check_ownership_offline(game_id) {
        Ok(status) => Ok(status),
        Err(err) if err.should_refresh_via_desktop() => {
            desktop::refresh_ownership_via_desktop(game_id)?;
            check_ownership_offline(game_id)
        }
        Err(err) => Err(err),
    }
}

pub mod ffi;
