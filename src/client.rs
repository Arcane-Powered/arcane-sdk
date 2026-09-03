//! The SDK client: initialise once at launch, then read state from memory.

use std::sync::Arc;

use crate::achievements::{AchievementCache, Achievements};
use crate::desktop::{offline_only, refresh_ownership_via_desktop, OFFLINE_ONLY_ENV};
use crate::device::{device_hash, now_unix};
use crate::error::{OwnershipStatus, SdkError};
use crate::friends::Friends;
use crate::p2p::{P2p, P2pState};
use crate::paths::{load_cached_drm_flag, load_session, SessionState};
use crate::session::{Session, SessionSnapshot, TrackingState};
use crate::ticket::{check_ownership_offline, OwnershipCheck};

/// Longest public key the SDK will accept, in bytes.
pub const MAX_PUBLIC_KEY_LEN: usize = 256;

/// Reject a malformed public key before any filesystem or network work happens,
/// so a typo surfaces as `invalid_public_key` instead of a confusing
/// `ticket_missing` several layers down.
///
/// The charset is also what makes it safe to interpolate the key straight into
/// the loopback URL and the `arcane-powered://` deep link.
pub(crate) fn validate_public_key(public_key: &str) -> Result<(), SdkError> {
    if public_key.is_empty() {
        return Err(SdkError::invalid_public_key("The public key is empty.")
            .with_hint("Pass the public key generated for this title in the Arcane portal."));
    }
    if public_key.len() > MAX_PUBLIC_KEY_LEN {
        return Err(SdkError::invalid_public_key("The public key is too long.")
            .with_hint("Pass the public key from the Arcane portal, not a file path or a token.")
            .with_context("length", public_key.len())
            .with_context("max_length", MAX_PUBLIC_KEY_LEN));
    }
    if let Some((index, bad)) = public_key
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
    {
        return Err(SdkError::invalid_public_key(
            "The public key contains a character that is not allowed.",
        )
        .with_hint(
            "Allowed characters are ASCII letters, digits, and `_`, `-`, `.`. \
             Check for stray whitespace or quotes around the value.",
        )
        .with_context("index", index)
        .with_context("character", format!("{bad:?}")));
    }
    Ok(())
}

/// The handle a game holds for its whole session.
///
/// Create it once at launch with [`ArcaneClient::init`]. It caches the ownership
/// result, the signed-in `user_id`, the title's `game_id` and the device
/// fingerprint, so nothing downstream has to pass the public key around again.
///
/// Ownership is never revalidated on its own: it reflects the state as of the
/// last [`init`](ArcaneClient::init) or [`refresh`](ArcaneClient::refresh). The
/// client does run one background thread for the play session — playtime and
/// FPS sampling — described in [`SessionSnapshot`].
///
/// Cloning shares that session, the achievement cache filled by
/// [`Achievements::list`], and the lobby state behind [`P2p`]. The session ends
/// when the last clone is dropped, or on [`shutdown`](ArcaneClient::shutdown).
#[derive(Debug, Clone)]
pub struct ArcaneClient {
    public_key: String,
    game_id: Option<String>,
    user_id: Option<String>,
    device_hash: String,
    ownership: OwnershipStatus,
    ticket_expires_at: Option<i64>,
    checked_at: i64,
    session: Arc<Session>,
    achievements: Arc<AchievementCache>,
    p2p: Arc<P2pState>,
}

impl ArcaneClient {
    /// Verify ownership and build the client. Call once, at launch.
    ///
    /// 1. Validates `public_key`.
    /// 2. If the cached `drm_enabled` flag is `false`, returns immediately with
    ///    [`OwnershipStatus::DrmDisabled`] — no ticket required.
    /// 3. Otherwise verifies the cached ownership ticket offline.
    /// 4. If the ticket is missing or expired, asks Arcane desktop to refresh
    ///    (opening the app via deep link when needed), then re-verifies.
    /// 5. Opens the play session and starts the `arcane-session` thread.
    ///
    /// The session never blocks or fails init: if the Arcane desktop app cannot
    /// be reached, tracking stays [`TrackingState::Pending`] and the thread
    /// retries every 60 seconds. It never opens the deep link.
    ///
    /// # Errors
    ///
    /// `invalid_public_key`, `ticket_missing`, `ticket_expired`, `ticket_invalid`,
    /// `device_mismatch`, `clock_rollback`, `not_owned`, `network_required`,
    /// `not_authenticated`, `arcane_unavailable`, `ambiguous_session`, `internal`.
    pub fn init(public_key: &str) -> Result<Self, SdkError> {
        validate_public_key(public_key)?;
        let client = Self::resolve_ownership(public_key)?;
        client.session.begin(client.tracking_state());
        Ok(client)
    }

    fn resolve_ownership(public_key: &str) -> Result<Self, SdkError> {
        if let Some(false) = load_cached_drm_flag(public_key) {
            return Self::drm_disabled(public_key);
        }

        match check_ownership_offline(public_key) {
            Ok(check) => Ok(Self::from_check(public_key, check)),
            Err(err) if err.should_refresh_via_desktop() && !offline_only() => {
                let outcome = refresh_ownership_via_desktop(public_key)?;
                match check_ownership_offline(public_key) {
                    Ok(check) => {
                        let mut client = Self::from_check(public_key, check);
                        client.user_id = client.user_id.or(outcome.user_id);
                        client.game_id = client.game_id.or(outcome.game_id);
                        Ok(client)
                    }
                    // The desktop confirmed DRM is off for this title, so there is
                    // no ticket to find and the missing file is not a failure.
                    Err(retry_err)
                        if !outcome.drm_enabled && retry_err.should_refresh_via_desktop() =>
                    {
                        let mut client = Self::drm_disabled(public_key)?;
                        client.user_id = client.user_id.or(outcome.user_id);
                        client.game_id = client.game_id.or(outcome.game_id);
                        Ok(client)
                    }
                    Err(retry_err) => Err(retry_err),
                }
            }
            Err(err) => Err(err),
        }
    }

    fn tracking_state(&self) -> TrackingState {
        if offline_only() {
            return TrackingState::Disabled;
        }
        if self.ownership == OwnershipStatus::DrmDisabled && self.user_id.is_none() {
            return TrackingState::Disabled;
        }
        TrackingState::Pending
    }

    /// Re-run the ownership check, contacting Arcane desktop, and update the
    /// cached state in place.
    ///
    /// Use this when a long-running session needs to re-confirm ownership — the
    /// client never does it on its own. Returns the same error codes as
    /// [`ArcaneClient::init`]. On failure the client keeps its previous state.
    pub fn refresh(&mut self) -> Result<OwnershipStatus, SdkError> {
        if offline_only() {
            return Err(SdkError::network_required(
                "Ownership refresh is disabled because the SDK is running in offline-only mode.",
            )
            .with_hint(format!(
                "Unset {OFFLINE_ONLY_ENV} to let the SDK contact the Arcane desktop app."
            ))
            .with_context("env", OFFLINE_ONLY_ENV));
        }

        let outcome = refresh_ownership_via_desktop(&self.public_key)?;

        let mut next = match check_ownership_offline(&self.public_key) {
            Ok(check) => Self::from_check(&self.public_key, check),
            Err(err) if !outcome.drm_enabled && err.should_refresh_via_desktop() => {
                Self::drm_disabled(&self.public_key)?
            }
            Err(err) => return Err(err),
        };
        next.user_id = next.user_id.or(outcome.user_id).or(self.user_id.clone());
        next.game_id = next.game_id.or(outcome.game_id).or(self.game_id.clone());
        next.session = Arc::clone(&self.session);
        next.achievements = Arc::clone(&self.achievements);
        next.p2p = Arc::clone(&self.p2p);

        let status = next.ownership.clone();
        *self = next;
        Ok(status)
    }

    /// The public key this client was initialised with.
    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    /// Canonical title id, when Arcane has reported one.
    pub fn game_id(&self) -> Option<&str> {
        self.game_id.as_deref()
    }

    /// The signed-in Arcane account, when one is known.
    pub fn user_id(&self) -> Option<&str> {
        self.user_id.as_deref()
    }

    /// This machine's fingerprint — the value tickets are bound to.
    pub fn device_hash(&self) -> &str {
        &self.device_hash
    }

    /// Ownership as of the last check.
    pub fn ownership(&self) -> OwnershipStatus {
        self.ownership.clone()
    }

    /// Whether a valid ownership ticket backed the last check. `false` when DRM
    /// is disabled for the title — the game should still run in that case.
    pub fn is_owned(&self) -> bool {
        self.ownership == OwnershipStatus::Owned
    }

    /// Unix timestamp of the ticket's `exp` claim, when a ticket was verified.
    pub fn ticket_expires_at(&self) -> Option<i64> {
        self.ticket_expires_at
    }

    /// Unix timestamp of the last successful check.
    pub fn checked_at(&self) -> i64 {
        self.checked_at
    }

    /// Count one rendered frame. Call it once per frame, from the render loop.
    ///
    /// Outside an FPS sampling window this is a single relaxed atomic load;
    /// inside one it adds a relaxed increment. No lock, no allocation, no clock
    /// read — safe to call thousands of times a second. A game that never calls
    /// it simply reports no FPS samples.
    pub fn frame(&self) {
        self.session.frame();
    }

    /// Record the current display settings, attached to the FPS samples that
    /// follow. Call it at startup and whenever the player changes them — never
    /// from the render loop, it takes a short lock.
    ///
    /// Both values are free-form and only travel to Arcane, for example
    /// `"2560x1440"` and `"high"`. Empty strings clear them.
    pub fn set_graphics(&self, resolution: &str, preset: &str) {
        self.session.set_graphics(resolution, preset);
    }

    /// Achievements for this title: list them, unlock one, read the cache.
    ///
    /// [`Achievements::list`] and [`Achievements::unlock`] each make one
    /// synchronous loopback call, so call them off the render thread — never per
    /// frame. [`Achievements::is_unlocked`] only reads memory.
    ///
    /// ```no_run
    /// # let client = arcane_sdk::ArcaneClient::init("pk_...")?;
    /// client.achievements().unlock("first_blood")?;
    /// # Ok::<(), arcane_sdk::SdkError>(())
    /// ```
    pub fn achievements(&self) -> Achievements<'_> {
        Achievements::new(&self.public_key, &self.achievements)
    }

    /// This player's friends on Arcane, with `online` and `in_game` for this
    /// title.
    ///
    /// [`Friends::list`] makes one synchronous loopback call, so call it when a
    /// menu opens or on a timer of your own — never per frame. The SDK holds no
    /// list of its own: the Arcane desktop app caches it and flags a stale
    /// answer.
    ///
    /// `in_game` comes from this client's [`game_id`](ArcaneClient::game_id),
    /// which is copied into a clone when the clone is made: a clone taken
    /// before a [`refresh`](ArcaneClient::refresh) that discovered a `game_id`
    /// still reports every friend as not in game.
    ///
    /// ```no_run
    /// # let client = arcane_sdk::ArcaneClient::init("pk_...")?;
    /// client.friends().list()?;
    /// # Ok::<(), arcane_sdk::SdkError>(())
    /// ```
    pub fn friends(&self) -> Friends<'_> {
        Friends::new(self.game_id())
    }

    /// P2P lobbies for this title: create one, join by code or by id, invite a
    /// friend, and read what happened.
    ///
    /// Arcane hosts the lobby and carries each member's opaque connection
    /// blob; your game keeps its own netcode. Every call but
    /// [`P2p::poll_events`] and [`P2p::launch_join_code`] makes one synchronous
    /// loopback round trip — call them off the render thread.
    ///
    /// **Calling this arms lobby event polling** on the `arcane-session`
    /// thread: from here on it asks the Arcane desktop app for events on every
    /// tick, and every 5 seconds while this client is in an open lobby. A game
    /// that never calls `p2p()` pays nothing for any of it.
    ///
    /// ```no_run
    /// # use arcane_sdk::Visibility;
    /// # let client = arcane_sdk::ArcaneClient::init("pk_...")?;
    /// # let my_endpoint = b"udp://203.0.113.7:7777";
    /// let lobby = client.p2p().create_lobby(4, Visibility::FriendsAndCode, my_endpoint)?;
    /// # Ok::<(), arcane_sdk::SdkError>(())
    /// ```
    pub fn p2p(&self) -> P2p<'_> {
        P2p::new(&self.public_key, &self.p2p)
    }

    /// A copy of the current play session state: tracking, playtime, FPS
    /// samples. Reads memory only.
    pub fn session(&self) -> SessionSnapshot {
        self.session.snapshot()
    }

    /// End the play session now, reporting the final playtime, and drop the
    /// client.
    ///
    /// This is the one blocking call of the lifecycle: it posts to the Arcane
    /// desktop app with a 2-second timeout. Dropping the last clone of a client
    /// does the same thing, best-effort, so calling this is optional — it just
    /// makes the moment explicit.
    pub fn shutdown(self) {
        self.session.end();
    }

    fn from_check(public_key: &str, check: OwnershipCheck) -> Self {
        let p2p = Arc::new(P2pState::new());
        Self {
            public_key: public_key.to_string(),
            game_id: check.game_id,
            user_id: check.user_id,
            device_hash: check.device_hash,
            ownership: check.status,
            ticket_expires_at: check.ticket_expires_at,
            checked_at: now_unix(),
            session: Arc::new(Session::dormant(public_key, Arc::clone(&p2p))),
            achievements: Arc::new(AchievementCache::new()),
            p2p,
        }
    }

    fn drm_disabled(public_key: &str) -> Result<Self, SdkError> {
        let p2p = Arc::new(P2pState::new());
        Ok(Self {
            public_key: public_key.to_string(),
            game_id: None,
            user_id: match load_session() {
                SessionState::SignedIn(user_id) => Some(user_id),
                _ => None,
            },
            device_hash: device_hash()?,
            ownership: OwnershipStatus::DrmDisabled,
            ticket_expires_at: None,
            checked_at: now_unix(),
            session: Arc::new(Session::dormant(public_key, Arc::clone(&p2p))),
            achievements: Arc::new(AchievementCache::new()),
            p2p,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_portal_shaped_keys() {
        for key in ["pk_live_abc123", "pk-test.01", "ABC", "0"] {
            assert!(validate_public_key(key).is_ok(), "rejected {key}");
        }
    }

    #[test]
    fn rejects_empty_key() {
        let err = validate_public_key("").unwrap_err();
        assert_eq!(err.code(), "invalid_public_key");
        assert!(err.hint().is_some());
    }

    #[test]
    fn rejects_oversized_key() {
        let err = validate_public_key(&"a".repeat(MAX_PUBLIC_KEY_LEN + 1)).unwrap_err();
        assert_eq!(err.code(), "invalid_public_key");
        assert!(err
            .context()
            .iter()
            .any(|(k, v)| k == "length" && v == &(MAX_PUBLIC_KEY_LEN + 1).to_string()));
    }

    #[test]
    fn rejects_characters_that_would_escape_a_url_or_path() {
        for key in [
            "pk_abc/../evil",
            "pk abc",
            "pk_abc?x=1",
            "pk_abc#frag",
            "pk_abc\n",
            " pk_abc",
            "pk_abc ",
            "pk_é",
        ] {
            let err = validate_public_key(key).unwrap_err();
            assert_eq!(err.code(), "invalid_public_key", "accepted {key:?}");
        }
    }

    #[test]
    fn reports_where_the_bad_character_is() {
        let err = validate_public_key("pk_ab/cd").unwrap_err();
        let context: Vec<_> = err
            .context()
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert!(context.contains(&("index", "5")));
        assert!(context.contains(&("character", "'/'")));
    }
}
