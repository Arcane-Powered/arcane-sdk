//! Arcane game SDK core — offline ownership ticket verification.
//!
//! The desktop launcher issues/refreshes JWTs and writes them under the shared
//! Arcane app-data DRM directory. Games link this crate (or the C ABI) and call
//! [`check_ownership_offline`] / [`arcane_init`] without talking to the network.

mod device;
mod error;
mod paths;
mod ticket;

pub use device::{device_hash, machine_id};
pub use error::{OwnershipStatus, SdkError};
pub use paths::{drm_data_root, load_cached_drm_flag, load_ticket_file, ticket_path};
pub use ticket::{check_ownership_offline, verify_ticket, OwnershipTicketClaims};

/// High-level init policy used by game engines.
///
/// - If cached `drm_enabled` is false → Ok (no ticket required).
/// - If true / unknown → require a valid offline ownership ticket.
pub fn arcane_init(game_id: &str) -> Result<OwnershipStatus, SdkError> {
    match load_cached_drm_flag(game_id) {
        Some(false) => Ok(OwnershipStatus::DrmDisabled),
        _ => check_ownership_offline(game_id),
    }
}

pub mod ffi;
