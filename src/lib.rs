//! Arcane game SDK core — offline ownership ticket verification.
//!
//! The desktop launcher issues/refreshes JWTs and writes them under the shared
//! Arcane app-data DRM directory. Games link this crate (or the C ABI) and call
//! [`arcane_init`] (or optionally [`check_ownership_offline`]) without talking
//! to the network.

mod device;
mod error;
mod paths;
mod ticket;

pub use error::{OwnershipStatus, SdkError};
pub use ticket::check_ownership_offline;

/// High-level init policy used by game engines.
///
/// - If cached `drm_enabled` is false → Ok (no ticket required).
/// - If true / unknown → require a valid offline ownership ticket.
pub fn arcane_init(game_id: &str) -> Result<OwnershipStatus, SdkError> {
    match paths::load_cached_drm_flag(game_id) {
        Some(false) => Ok(OwnershipStatus::DrmDisabled),
        _ => check_ownership_offline(game_id),
    }
}

pub mod ffi;
