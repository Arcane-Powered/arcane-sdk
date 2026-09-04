//! Arcane game SDK core.
//!
//! Games build one [`ArcaneClient`] at launch with their Arcane portal public
//! key. The client verifies ownership once and then holds the result in memory —
//! the signed-in `user_id`, the title's `game_id`, the ownership status and the
//! device fingerprint — so nothing downstream needs the public key again.
//!
//! ```no_run
//! use arcane_sdk::ArcaneClient;
//!
//! let client = ArcaneClient::init("pk_...")?;
//! println!("user={:?} owned={}", client.user_id(), client.is_owned());
//! # Ok::<(), arcane_sdk::SdkError>(())
//! ```
//!
//! Ownership is checked against a locally cached ticket when possible. If the
//! ticket is missing or expired, the SDK asks the Arcane desktop app (loopback
//! `127.0.0.1:39284`) to refresh online, opening the app via deep link when
//! needed.
//!
//! `init` also opens a play session: one background thread reports playtime to
//! the Arcane desktop app once a minute, and — while the player allows it —
//! samples the frame rate in short windows if the game calls
//! [`ArcaneClient::frame`]. It never blocks or fails `init`. See
//! [`SessionSnapshot`].
//!
//! Achievements are opt-in and cost nothing until used: one line unlocks one,
//! and it is idempotent, so a game can call it every time its condition holds.
//!
//! ```no_run
//! # let client = arcane_sdk::ArcaneClient::init("pk_...")?;
//! client.achievements().unlock("first_blood")?;
//! # Ok::<(), arcane_sdk::SdkError>(())
//! ```
//!
//! Friends are the same shape: one call gives the list with `online` and
//! `in_game` for this title, read straight from the Arcane desktop app.
//!
//! ```no_run
//! # let client = arcane_sdk::ArcaneClient::init("pk_...")?;
//! client.friends().list()?;
//! # Ok::<(), arcane_sdk::SdkError>(())
//! ```
//!
//! Failures are [`SdkError`], carrying a stable [`code`](SdkError::code), a
//! player-facing [`message`](SdkError::message), a developer-facing
//! [`hint`](SdkError::hint) and a [`context`](SdkError::context) key/value list.

mod achievements;
mod client;
mod desktop;
mod device;
mod error;
mod friends;
mod paths;
mod session;
mod ticket;

pub use achievements::{Achievement, Achievements, Unlock, MAX_ACHIEVEMENT_KEY_LEN};
pub use client::{ArcaneClient, MAX_PUBLIC_KEY_LEN};
pub use error::{ErrorCode, OwnershipStatus, SdkError};
pub use friends::{Friend, FriendList, Friends};
pub use session::{SessionSnapshot, TrackingState};

pub mod ffi;
