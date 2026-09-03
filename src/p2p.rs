//! P2P lobbies: the meeting point, not the transport.
//!
//! Arcane hosts the **lobby** — a host, its members, a capacity, a join code —
//! and carries an opaque `payload` for each member. Your game keeps its own
//! netcode: there is no relay here, no NAT traversal, no host migration and no
//! public matchmaking. What Arcane gives you is the part that is annoying to
//! build yourself: a place for players to find each other, invitations between
//! friends, and "Join" from the launcher.
//!
//! ```no_run
//! # use arcane_sdk::Visibility;
//! # let client = arcane_sdk::ArcaneClient::init("pk_...")?;
//! # let my_endpoint = b"udp://203.0.113.7:7777";
//! let lobby = client.p2p().create_lobby(4, Visibility::FriendsAndCode, my_endpoint)?;
//! println!("join code: {:?}", lobby.join_code);
//! # Ok::<(), arcane_sdk::SdkError>(())
//! ```
//!
//! The `payload` is yours: a public address, a ticket from your own netcode,
//! anything up to 4 KiB. Arcane transports it base64-encoded and never reads it.
//!
//! [`P2p::poll_events`] drains a queue the `arcane-session` thread fills. That
//! polling is **armed by the first call to [`crate::ArcaneClient::p2p`]** and
//! costs a game that never touches lobbies exactly nothing. While the client is
//! in an open lobby the thread polls every 5 seconds instead of every 60;
//! heartbeats keep their own schedule either way.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, Weak};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::desktop::{
    delete_json, get_json, offline_only, post_json, DesktopCall, GAMES_PATH_PREFIX,
    OFFLINE_ONLY_ENV,
};
use crate::error::SdkError;
use crate::session::SessionInner;

const CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// How fast the session thread polls lobby events while the client is in an
/// open lobby, instead of the usual 60-second tick.
pub(crate) const IN_LOBBY_POLL: Duration = Duration::from_secs(5);

/// Longest `payload` the SDK will carry for one member, in bytes.
pub const MAX_LOBBY_PAYLOAD_LEN: usize = 4096;

/// Number of characters in a join code.
pub const JOIN_CODE_LEN: usize = 6;

const MAX_ID_LEN: usize = 64;
const MAX_QUEUED_EVENTS: usize = 256;

/// Who can join a lobby.
///
/// Arcane never lists lobbies publicly: a lobby is reachable through the host's
/// friends, through its join code, or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    /// The host's friends can see it in the launcher and join. No join code is
    /// issued.
    Friends,
    /// Only players who have the join code can join.
    Code,
    /// Both: friends see it, and the code works for anyone who has it.
    FriendsAndCode,
}

impl Visibility {
    /// Stable wire string: `"friends"`, `"code"`, `"friends_and_code"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Friends => "friends",
            Self::Code => "code",
            Self::FriendsAndCode => "friends_and_code",
        }
    }
}

impl std::fmt::Display for Visibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A lobby as Arcane knows it, right after a create or a join.
///
/// It is a snapshot, not a live view: members who arrive later come through
/// [`P2p::poll_events`] as [`LobbyEvent::MemberJoined`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lobby {
    /// Arcane's id for the lobby — what [`P2p::invite`], [`P2p::leave`] and
    /// [`P2p::close`] take.
    pub lobby_id: String,
    /// The six-character code to show the player, when the lobby's
    /// [`Visibility`] issues one. `None` for a friends-only lobby, and for a
    /// member who is not the host.
    pub join_code: Option<String>,
    /// The Arcane account hosting the lobby.
    pub host_user_id: String,
    /// The host's connection blob, decoded. This is what a joining player
    /// connects to.
    pub host_payload: Vec<u8>,
    /// Everyone in the lobby right now, host included.
    pub members: Vec<LobbyMember>,
    /// Capacity the host asked for.
    pub max_players: u8,
}

/// One player in a lobby, with the blob they published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LobbyMember {
    /// Their Arcane account id.
    pub user_id: String,
    /// Their display name on Arcane.
    pub pseudo: String,
    /// Their connection blob, decoded — whatever their copy of the game passed
    /// to [`P2p::create_lobby`] or [`P2p::join`].
    pub payload: Vec<u8>,
}

/// Something that happened in a lobby this player is in, or an invitation.
///
/// Delivered by [`P2p::poll_events`], oldest first, exactly once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LobbyEvent {
    /// A friend invited this player to their lobby. Join it with
    /// [`P2p::join`], or with [`P2p::join_by_code`] when a code came with it.
    Invite {
        lobby_id: String,
        join_code: Option<String>,
        from_user_id: String,
        pseudo: String,
    },
    /// Somebody joined a lobby this player is in. `payload` is their
    /// connection blob — connect to it.
    MemberJoined {
        lobby_id: String,
        user_id: String,
        pseudo: String,
        payload: Vec<u8>,
    },
    /// Somebody left.
    MemberLeft { lobby_id: String, user_id: String },
    /// The lobby is over: the host closed it or their play session expired.
    /// There is no host migration — open a new lobby.
    LobbyClosed { lobby_id: String },
}

impl LobbyEvent {
    /// The lobby this event is about.
    pub fn lobby_id(&self) -> &str {
        match self {
            Self::Invite { lobby_id, .. }
            | Self::MemberJoined { lobby_id, .. }
            | Self::MemberLeft { lobby_id, .. }
            | Self::LobbyClosed { lobby_id } => lobby_id,
        }
    }
}

/// Whether the session thread is polling Arcane for lobby events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LobbyPollingState {
    /// The game has never called `p2p()`, so nothing is polled.
    Off,
    /// Armed: the session thread asks for events on every tick.
    Active,
    /// The Arcane desktop app predates the lobby routes. Polling stopped and
    /// will not restart for this client.
    Unavailable,
}

impl LobbyPollingState {
    /// Stable wire string: `"off"`, `"active"`, `"unavailable"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Active => "active",
            Self::Unavailable => "unavailable",
        }
    }
}

impl std::fmt::Display for LobbyPollingState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug)]
struct Shared {
    polling: LobbyPollingState,
    cursor: Option<String>,
    events: VecDeque<LobbyEvent>,
    lobbies: Vec<String>,
}

/// Everything the game and the `arcane-session` thread share about lobbies:
/// the arming flag, the event queue, the cursor, and which lobbies are open.
#[derive(Debug)]
pub(crate) struct P2pState {
    armed: AtomicBool,
    shared: Mutex<Shared>,
    launch_code: Mutex<Option<Option<String>>>,
    waker: Mutex<Option<Weak<SessionInner>>>,
}

impl P2pState {
    pub(crate) fn new() -> Self {
        Self {
            armed: AtomicBool::new(false),
            shared: Mutex::new(Shared {
                polling: LobbyPollingState::Off,
                cursor: None,
                events: VecDeque::new(),
                lobbies: Vec::new(),
            }),
            launch_code: Mutex::new(None),
            waker: Mutex::new(None),
        }
    }

    /// Let the session thread be woken when arming or a lobby changes what it
    /// should do next, instead of waiting out a sleep of up to a minute.
    pub(crate) fn set_waker(&self, waker: Weak<SessionInner>) {
        *self.waker.lock().unwrap_or_else(|e| e.into_inner()) = Some(waker);
    }

    fn notify(&self) {
        let waker = self
            .waker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(Weak::upgrade);
        if let Some(session) = waker {
            session.wake_now();
        }
    }

    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Start polling, unless a `feature_unavailable` already retired it for
    /// this client.
    fn arm(&self) {
        if self.armed.load(Ordering::Relaxed) {
            return;
        }
        let mut shared = self.lock();
        if shared.polling == LobbyPollingState::Unavailable {
            return;
        }
        shared.polling = LobbyPollingState::Active;
        self.armed.store(true, Ordering::Relaxed);
        drop(shared);
        self.notify();
    }

    pub(crate) fn armed(&self) -> bool {
        self.armed.load(Ordering::Relaxed)
    }

    pub(crate) fn polling(&self) -> LobbyPollingState {
        self.lock().polling
    }

    /// The poll period for this tick: 5 seconds while a lobby is open, the
    /// session tick otherwise, and never slower than the session tick.
    pub(crate) fn poll_period(&self, tick: Duration) -> Duration {
        if self.lock().lobbies.is_empty() {
            tick
        } else {
            IN_LOBBY_POLL.min(tick)
        }
    }

    fn enter(&self, lobby_id: &str) {
        let mut shared = self.lock();
        if !shared.lobbies.iter().any(|open| open == lobby_id) {
            shared.lobbies.push(lobby_id.to_string());
        }
        drop(shared);
        self.notify();
    }

    fn exit(&self, lobby_id: &str) {
        self.lock().lobbies.retain(|open| open != lobby_id);
        self.notify();
    }

    fn take_events(&self) -> Vec<LobbyEvent> {
        let mut shared = self.lock();
        shared.events.drain(..).collect()
    }

    /// The queued events as C ABI JSON, and how many were rendered — so the
    /// caller can drop exactly those once they are safely in the buffer.
    pub(crate) fn events_json(&self) -> (String, usize) {
        let shared = self.lock();
        (render_events(shared.events.iter()), shared.events.len())
    }

    pub(crate) fn discard(&self, count: usize) {
        let mut shared = self.lock();
        for _ in 0..count.min(shared.events.len()) {
            shared.events.pop_front();
        }
    }

    fn ingest(&self, events: Vec<LobbyEvent>, cursor: Option<String>) {
        let mut shared = self.lock();
        if let Some(cursor) = cursor.filter(|value| !value.is_empty()) {
            shared.cursor = Some(cursor);
        }
        for event in events {
            if let LobbyEvent::LobbyClosed { lobby_id } = &event {
                shared.lobbies.retain(|open| open != lobby_id);
            }
            if shared.events.len() >= MAX_QUEUED_EVENTS {
                shared.events.pop_front();
            }
            shared.events.push_back(event);
        }
    }

    /// Retire polling for good: the desktop app does not know the route.
    fn retire(&self) {
        self.armed.store(false, Ordering::Relaxed);
        self.lock().polling = LobbyPollingState::Unavailable;
    }
}

/// One round of event polling, called by the `arcane-session` thread.
///
/// Returns the failure to record on the session, if any. A
/// `feature_unavailable` is not one: it retires polling silently and shows up
/// as [`LobbyPollingState::Unavailable`] in the session snapshot instead.
pub(crate) fn poll_once(public_key: &str, state: &P2pState) -> Option<SdkError> {
    let cursor = state.lock().cursor.clone();
    let response: WireEvents =
        match get_json(&events_path(public_key, cursor.as_deref()), CALL_TIMEOUT) {
            Ok(response) => response,
            Err(call) => {
                let err = call.into_sdk_error();
                if err.error_code() == crate::error::ErrorCode::FeatureUnavailable {
                    state.retire();
                    return None;
                }
                return Some(err);
            }
        };

    let events = response.events.into_iter().filter_map(map_event).collect();
    state.ingest(events, response.cursor);
    None
}

/// The lobby accessor, borrowed from the client.
///
/// Build it with [`crate::ArcaneClient::p2p`]. Doing so is what arms lobby
/// event polling on the session thread, so a game that never calls it pays
/// nothing.
#[derive(Debug)]
pub struct P2p<'a> {
    public_key: &'a str,
    state: &'a P2pState,
}

impl<'a> P2p<'a> {
    pub(crate) fn new(public_key: &'a str, state: &'a P2pState) -> Self {
        state.arm();
        Self { public_key, state }
    }

    /// Open a lobby with this player as its host.
    ///
    /// `payload` is your connection blob — up to [`MAX_LOBBY_PAYLOAD_LEN`]
    /// opaque bytes that Arcane carries to whoever joins. `max_players`
    /// includes the host.
    ///
    /// One synchronous loopback round trip. The returned [`Lobby`] carries the
    /// [`join_code`](Lobby::join_code) to show the player when the visibility
    /// issues one.
    ///
    /// # Errors
    ///
    /// `invalid_argument` when `payload` is longer than
    /// [`MAX_LOBBY_PAYLOAD_LEN`] (raised before any call), `not_owned`,
    /// `not_authenticated`, `network_required` (including under
    /// `ARCANE_OFFLINE_ONLY`), `arcane_unavailable`, `feature_unavailable`.
    pub fn create_lobby(
        &self,
        max_players: u8,
        visibility: Visibility,
        payload: &[u8],
    ) -> Result<Lobby, SdkError> {
        let encoded = encode_payload(payload)?;
        self.guard_offline("Creating a lobby")?;

        let body = json!({
            "max_players": max_players,
            "visibility": visibility.as_str(),
            "payload": encoded,
        });
        let wire: WireLobby = post_json(&self.lobbies_path(), Some(body), CALL_TIMEOUT)
            .map_err(DesktopCall::into_sdk_error)?;

        self.entered(map_lobby(wire)?)
    }

    /// Join the lobby a six-character code points at.
    ///
    /// The code is uppercased before it is checked, so a player typing
    /// `k7p3qx` is fine. `payload` is your own connection blob, published to
    /// the members already there.
    ///
    /// # Errors
    ///
    /// `invalid_argument` when the code is not six characters of
    /// `A–H J–N P–Z 2–9` or `payload` is too long (both raised before any
    /// call), `lobby_not_found`, `lobby_full`, `lobby_closed`, plus the codes
    /// of [`P2p::create_lobby`].
    pub fn join_by_code(&self, join_code: &str, payload: &[u8]) -> Result<Lobby, SdkError> {
        let code = normalize_join_code(join_code)?;
        let encoded = encode_payload(payload)?;
        self.guard_offline("Joining a lobby")?;

        let body = json!({ "join_code": code, "payload": encoded });
        let wire: WireLobby = post_json(
            &format!("{}/join", self.lobbies_path()),
            Some(body),
            CALL_TIMEOUT,
        )
        .map_err(|call| call.into_sdk_error().with_context("join_code", &code))?;

        self.entered(map_lobby(wire)?)
    }

    /// Join a lobby by id — what an [`LobbyEvent::Invite`] carries, and what
    /// the launcher passes when a friend hits "Join".
    ///
    /// # Errors
    ///
    /// `invalid_argument` for a malformed id or an oversized payload,
    /// `not_friends` when the lobby is open to the host's friends only,
    /// `lobby_not_found`, `lobby_full`, `lobby_closed`, plus the codes of
    /// [`P2p::create_lobby`].
    pub fn join(&self, lobby_id: &str, payload: &[u8]) -> Result<Lobby, SdkError> {
        validate_id("lobby_id", lobby_id)?;
        let encoded = encode_payload(payload)?;
        self.guard_offline("Joining a lobby")?;

        let body = json!({ "payload": encoded });
        let wire: WireLobby = post_json(
            &format!("{}/{lobby_id}/join", self.lobbies_path()),
            Some(body),
            CALL_TIMEOUT,
        )
        .map_err(|call| call.into_sdk_error().with_context("lobby_id", lobby_id))?;

        self.entered(map_lobby(wire)?)
    }

    /// Invite one friend to a lobby.
    ///
    /// Arcane delivers it to their launcher: they get an
    /// [`LobbyEvent::Invite`] if they are already playing this title, and the
    /// code for their next launch otherwise
    /// ([`P2p::launch_join_code`]).
    ///
    /// # Errors
    ///
    /// `invalid_argument` for a malformed id, `lobby_not_found`,
    /// `lobby_closed`, `not_friends` when that account is not a friend, plus
    /// the codes of [`P2p::create_lobby`].
    pub fn invite(&self, lobby_id: &str, to_user_id: &str) -> Result<(), SdkError> {
        validate_id("lobby_id", lobby_id)?;
        validate_id("to_user_id", to_user_id)?;
        self.guard_offline("Inviting a friend")?;

        let body = json!({ "to_user_id": to_user_id });
        let _: WireOk = post_json(
            &format!("{}/{lobby_id}/invite", self.lobbies_path()),
            Some(body),
            CALL_TIMEOUT,
        )
        .map_err(|call| {
            call.into_sdk_error()
                .with_context("lobby_id", lobby_id)
                .with_context("to_user_id", to_user_id)
        })?;

        Ok(())
    }

    /// Leave a lobby. For the host this is the same as [`P2p::close`] on
    /// Arcane's side: there is no host migration.
    ///
    /// # Errors
    ///
    /// `invalid_argument` for a malformed id, `lobby_not_found`, plus the
    /// codes of [`P2p::create_lobby`].
    pub fn leave(&self, lobby_id: &str) -> Result<(), SdkError> {
        validate_id("lobby_id", lobby_id)?;
        self.guard_offline("Leaving a lobby")?;

        let sent: Result<WireOk, DesktopCall> = post_json(
            &format!("{}/{lobby_id}/leave", self.lobbies_path()),
            None,
            CALL_TIMEOUT,
        );
        self.state.exit(lobby_id);
        sent.map_err(|call| call.into_sdk_error().with_context("lobby_id", lobby_id))?;

        Ok(())
    }

    /// Close a lobby this player hosts. Its members get a
    /// [`LobbyEvent::LobbyClosed`].
    ///
    /// # Errors
    ///
    /// `invalid_argument` for a malformed id, `lobby_not_found`, plus the
    /// codes of [`P2p::create_lobby`].
    pub fn close(&self, lobby_id: &str) -> Result<(), SdkError> {
        validate_id("lobby_id", lobby_id)?;
        self.guard_offline("Closing a lobby")?;

        let sent: Result<WireOk, DesktopCall> =
            delete_json(&format!("{}/{lobby_id}", self.lobbies_path()), CALL_TIMEOUT);
        self.state.exit(lobby_id);
        sent.map_err(|call| call.into_sdk_error().with_context("lobby_id", lobby_id))?;

        Ok(())
    }

    /// The join code this game was launched with, when the player started it
    /// from a friend's "Join" in the launcher.
    ///
    /// Read from the Arcane desktop app on the **first** call and cached for
    /// the client's lifetime — the desktop app clears it once it has been
    /// served, so it belongs to this launch and no other. `None` when the game
    /// was started normally, when the desktop app predates the route, and in
    /// offline-only mode. Never fails.
    ///
    /// ```no_run
    /// # let client = arcane_sdk::ArcaneClient::init("pk_...")?;
    /// # let my_endpoint = b"udp://203.0.113.7:7777";
    /// if let Some(code) = client.p2p().launch_join_code() {
    ///     let lobby = client.p2p().join_by_code(&code, my_endpoint)?;
    ///     println!("connect to {} bytes", lobby.host_payload.len());
    /// }
    /// # Ok::<(), arcane_sdk::SdkError>(())
    /// ```
    pub fn launch_join_code(&self) -> Option<String> {
        let mut cached = self
            .state
            .launch_code
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(code) = cached.as_ref() {
            return code.clone();
        }
        if offline_only() {
            return None;
        }

        let fetched: Option<String> = get_json::<WireLaunchContext>(
            &format!("{GAMES_PATH_PREFIX}/{}/launch-context", self.public_key),
            CALL_TIMEOUT,
        )
        .ok()
        .and_then(|context| context.join_code)
        .and_then(|code| normalize_join_code(&code).ok());

        *cached = Some(fetched.clone());
        fetched
    }

    /// Take everything the session thread has collected since the last call.
    ///
    /// Reads memory only — no I/O, no failure, no callback and no extra
    /// thread. Once a second is plenty. Events come oldest first and are
    /// delivered exactly once.
    ///
    /// ```no_run
    /// # use arcane_sdk::LobbyEvent;
    /// # let client = arcane_sdk::ArcaneClient::init("pk_...")?;
    /// for event in client.p2p().poll_events() {
    ///     if let LobbyEvent::MemberJoined { payload, .. } = event {
    ///         // connect_to(&payload)
    ///     }
    /// }
    /// # Ok::<(), arcane_sdk::SdkError>(())
    /// ```
    pub fn poll_events(&self) -> Vec<LobbyEvent> {
        self.state.take_events()
    }

    pub(crate) fn events_json(&self) -> (String, usize) {
        self.state.events_json()
    }

    pub(crate) fn discard(&self, count: usize) {
        self.state.discard(count);
    }

    fn entered(&self, lobby: Lobby) -> Result<Lobby, SdkError> {
        self.state.enter(&lobby.lobby_id);
        Ok(lobby)
    }

    fn lobbies_path(&self) -> String {
        format!("{GAMES_PATH_PREFIX}/{}/lobbies", self.public_key)
    }

    fn guard_offline(&self, action: &str) -> Result<(), SdkError> {
        if !offline_only() {
            return Ok(());
        }
        Err(SdkError::network_required(format!(
            "{action} needs the Arcane desktop app, and the SDK is running in offline-only mode."
        ))
        .with_hint(format!(
            "Unset {OFFLINE_ONLY_ENV} to let the SDK contact the Arcane desktop app."
        ))
        .with_context("env", OFFLINE_ONLY_ENV))
    }
}

/// Reject an oversized payload before any network call, and encode the rest.
pub(crate) fn encode_payload(payload: &[u8]) -> Result<String, SdkError> {
    if payload.len() > MAX_LOBBY_PAYLOAD_LEN {
        return Err(
            SdkError::invalid_argument("The lobby payload is larger than Arcane carries.")
                .with_hint(
                    "Send a reference your own netcode can resolve — an address or a ticket — \
             not the data itself.",
                )
                .with_context("length", payload.len())
                .with_context("max_length", MAX_LOBBY_PAYLOAD_LEN),
        );
    }
    Ok(BASE64.encode(payload))
}

/// Decode a base64 payload argument from the C ABI. `None` when it is not
/// base64 at all — the caller reports a bad argument rather than sending bytes
/// the game did not mean.
pub(crate) fn decode_payload_arg(raw: &str) -> Option<Vec<u8>> {
    BASE64.decode(raw.trim()).ok()
}

/// Uppercase a join code and check it against the unambiguous alphabet the
/// backend generates from — no `I`, `O`, `0` or `1` to mistype.
pub(crate) fn normalize_join_code(join_code: &str) -> Result<String, SdkError> {
    let code = join_code.trim().to_ascii_uppercase();
    let malformed = SdkError::invalid_argument("That is not an Arcane join code.")
        .with_hint(
            "Join codes are 6 characters long, from `A`–`Z` without `I` or `O` and \
             `2`–`9`. Check what the player typed.",
        )
        .with_context("join_code", join_code)
        .with_context("expected_length", JOIN_CODE_LEN);

    if code.chars().count() != JOIN_CODE_LEN {
        return Err(malformed);
    }
    if !code
        .chars()
        .all(|c| matches!(c, 'A'..='H' | 'J'..='N' | 'P'..='Z' | '2'..='9'))
    {
        return Err(malformed);
    }
    Ok(code)
}

/// Reject an id that would not survive being interpolated into a loopback path.
pub(crate) fn validate_id(field: &str, value: &str) -> Result<(), SdkError> {
    let malformed = |reason: &str| {
        SdkError::invalid_argument(format!("The {field} is {reason}."))
            .with_hint(
                "Pass the value Arcane gave you — ids are ASCII letters, digits and `-`, \
                 up to 64 characters.",
            )
            .with_context("field", field)
            .with_context(field, value)
    };

    if value.is_empty() {
        return Err(malformed("empty"));
    }
    if value.len() > MAX_ID_LEN {
        return Err(malformed("too long"));
    }
    if !value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(malformed("not an Arcane id"));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct WireOk {
    #[serde(default)]
    #[allow(dead_code)]
    ok: bool,
}

#[derive(Debug, Deserialize)]
struct WireLaunchContext {
    #[serde(default)]
    join_code: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireLobby {
    #[serde(default)]
    lobby_id: String,
    #[serde(default)]
    join_code: Option<String>,
    #[serde(default)]
    host_user_id: String,
    #[serde(default)]
    host_payload: Option<String>,
    #[serde(default)]
    max_players: u8,
    #[serde(default)]
    members: Vec<WireMember>,
}

#[derive(Debug, Deserialize)]
struct WireMember {
    #[serde(default)]
    user_id: String,
    #[serde(default)]
    pseudo: String,
    #[serde(default)]
    payload: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireEvents {
    #[serde(default)]
    events: Vec<WireEvent>,
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireEvent {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    lobby_id: String,
    #[serde(default)]
    join_code: Option<String>,
    #[serde(default)]
    from_user_id: Option<String>,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    pseudo: Option<String>,
    #[serde(default)]
    payload: Option<String>,
}

fn map_lobby(wire: WireLobby) -> Result<Lobby, SdkError> {
    Ok(Lobby {
        lobby_id: wire.lobby_id,
        join_code: wire.join_code.filter(|code| !code.trim().is_empty()),
        host_user_id: wire.host_user_id,
        host_payload: decode_payload(wire.host_payload.as_deref(), "host_payload")?,
        members: wire
            .members
            .into_iter()
            .map(|member| {
                Ok(LobbyMember {
                    user_id: member.user_id,
                    pseudo: member.pseudo,
                    payload: decode_payload(member.payload.as_deref(), "payload")?,
                })
            })
            .collect::<Result<Vec<_>, SdkError>>()?,
        max_players: wire.max_players,
    })
}

/// An event whose payload the desktop app mangled is still delivered — there is
/// no caller to hand an error to on the session thread — with an empty payload.
fn map_event(wire: WireEvent) -> Option<LobbyEvent> {
    let lobby_id = wire.lobby_id;
    if lobby_id.is_empty() {
        return None;
    }
    let payload = || decode_payload(wire.payload.as_deref(), "payload").unwrap_or_default();

    Some(match wire.kind.as_str() {
        "invite" => LobbyEvent::Invite {
            lobby_id,
            join_code: wire.join_code.filter(|code| !code.trim().is_empty()),
            from_user_id: wire.from_user_id.unwrap_or_default(),
            pseudo: wire.pseudo.unwrap_or_default(),
        },
        "member_joined" => LobbyEvent::MemberJoined {
            lobby_id,
            user_id: wire.user_id.unwrap_or_default(),
            pseudo: wire.pseudo.unwrap_or_default(),
            payload: payload(),
        },
        "member_left" => LobbyEvent::MemberLeft {
            lobby_id,
            user_id: wire.user_id.unwrap_or_default(),
        },
        "lobby_closed" => LobbyEvent::LobbyClosed { lobby_id },
        _ => return None,
    })
}

fn decode_payload(encoded: Option<&str>, field: &str) -> Result<Vec<u8>, SdkError> {
    let Some(encoded) = encoded.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(Vec::new());
    };
    BASE64.decode(encoded).map_err(|e| {
        SdkError::arcane_unavailable(format!("Unexpected Arcane desktop payload: {e}"))
            .with_hint("The Arcane desktop app must relay a payload untouched — update it.")
            .with_context("field", field)
    })
}

fn events_path(public_key: &str, cursor: Option<&str>) -> String {
    let base = format!("{GAMES_PATH_PREFIX}/{public_key}/lobbies/events");
    match cursor {
        Some(cursor) => format!("{base}?after={}", encode_query(cursor)),
        None => base,
    }
}

/// Percent-encode an opaque cursor so it survives the query string whatever the
/// desktop app puts in it.
fn encode_query(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(*byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// The C ABI rendering, in the field order the header and the docs promise —
/// which `serde_json::json!` would not keep, as it sorts its keys.
#[derive(Serialize)]
struct JsonLobby<'a> {
    lobby_id: &'a str,
    join_code: Option<&'a str>,
    host_user_id: &'a str,
    host_payload: String,
    members: Vec<JsonMember<'a>>,
    max_players: u8,
}

#[derive(Serialize)]
struct JsonMember<'a> {
    user_id: &'a str,
    pseudo: &'a str,
    payload: String,
}

#[derive(Serialize)]
struct JsonEvents<'a> {
    events: Vec<JsonEvent<'a>>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum JsonEvent<'a> {
    Invite {
        lobby_id: &'a str,
        join_code: Option<&'a str>,
        from_user_id: &'a str,
        pseudo: &'a str,
    },
    MemberJoined {
        lobby_id: &'a str,
        user_id: &'a str,
        pseudo: &'a str,
        payload: String,
    },
    MemberLeft {
        lobby_id: &'a str,
        user_id: &'a str,
    },
    LobbyClosed {
        lobby_id: &'a str,
    },
}

pub(crate) fn to_json(lobby: &Lobby) -> String {
    let rendered = JsonLobby {
        lobby_id: &lobby.lobby_id,
        join_code: lobby.join_code.as_deref(),
        host_user_id: &lobby.host_user_id,
        host_payload: BASE64.encode(&lobby.host_payload),
        members: lobby
            .members
            .iter()
            .map(|member| JsonMember {
                user_id: &member.user_id,
                pseudo: &member.pseudo,
                payload: BASE64.encode(&member.payload),
            })
            .collect(),
        max_players: lobby.max_players,
    };
    serde_json::to_string(&rendered).unwrap_or_else(|_| r#"{"lobby_id":""}"#.to_string())
}

fn render_events<'a>(events: impl Iterator<Item = &'a LobbyEvent>) -> String {
    let rendered = JsonEvents {
        events: events
            .map(|event| match event {
                LobbyEvent::Invite {
                    lobby_id,
                    join_code,
                    from_user_id,
                    pseudo,
                } => JsonEvent::Invite {
                    lobby_id,
                    join_code: join_code.as_deref(),
                    from_user_id,
                    pseudo,
                },
                LobbyEvent::MemberJoined {
                    lobby_id,
                    user_id,
                    pseudo,
                    payload,
                } => JsonEvent::MemberJoined {
                    lobby_id,
                    user_id,
                    pseudo,
                    payload: BASE64.encode(payload),
                },
                LobbyEvent::MemberLeft { lobby_id, user_id } => {
                    JsonEvent::MemberLeft { lobby_id, user_id }
                }
                LobbyEvent::LobbyClosed { lobby_id } => JsonEvent::LobbyClosed { lobby_id },
            })
            .collect(),
    };
    serde_json::to_string(&rendered).unwrap_or_else(|_| r#"{"events":[]}"#.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_lobby(raw: &str) -> Lobby {
        map_lobby(serde_json::from_str(raw).expect("wire lobby")).expect("mapped lobby")
    }

    const LOBBY: &str = r#"{
        "lobby_id": "lobby-1",
        "join_code": "K7P3QX",
        "host_user_id": "user-host",
        "host_payload": "dWRwOi8vMTAuMC4wLjE6Nzc3Nw==",
        "visibility": "friends_and_code",
        "max_players": 4,
        "members": [
            {"user_id":"user-host","pseudo":"Ada","payload":"dWRwOi8vMTAuMC4wLjE6Nzc3Nw=="},
            {"user_id":"user-b","pseudo":"Bo","payload":null}
        ],
        "expires_at": "2026-05-01T12:34:56Z"
    }"#;

    #[test]
    fn a_payload_survives_the_base64_round_trip() {
        let payload: Vec<u8> = (0u8..=255).collect();
        let encoded = encode_payload(&payload).expect("encoded");

        assert_eq!(
            decode_payload(Some(&encoded), "payload").expect("decoded"),
            payload
        );
        assert_eq!(encode_payload(b"").expect("empty"), "");
        assert_eq!(
            decode_payload(Some(""), "payload").expect("empty"),
            Vec::<u8>::new()
        );
        assert_eq!(
            decode_payload(None, "payload").expect("absent"),
            Vec::<u8>::new()
        );
    }

    #[test]
    fn a_payload_at_the_limit_is_accepted_and_one_byte_over_is_not() {
        assert!(encode_payload(&vec![7u8; MAX_LOBBY_PAYLOAD_LEN]).is_ok());

        let err = encode_payload(&vec![7u8; MAX_LOBBY_PAYLOAD_LEN + 1]).expect_err("too long");
        assert_eq!(err.code(), "invalid_argument");
        assert!(err
            .context()
            .iter()
            .any(|(k, v)| k == "length" && v == &(MAX_LOBBY_PAYLOAD_LEN + 1).to_string()));
    }

    #[test]
    fn a_c_abi_payload_argument_decodes_or_is_refused() {
        assert_eq!(decode_payload_arg("aGk=").expect("base64"), b"hi");
        assert_eq!(decode_payload_arg("").expect("empty"), Vec::<u8>::new());
        assert!(decode_payload_arg("not base64!!").is_none());
    }

    #[test]
    fn a_join_code_is_uppercased_before_it_is_checked() {
        assert_eq!(normalize_join_code("k7p3qx").expect("lowercase"), "K7P3QX");
        assert_eq!(normalize_join_code(" K7P3QX ").expect("padded"), "K7P3QX");
    }

    #[test]
    fn the_ambiguous_characters_are_not_join_codes() {
        for code in [
            "", "K7P3Q", "K7P3QXY", "K7P3Q0", "K7P3Q1", "K7P3QI", "K7P3QO", "K7-3QX", "K7P3Q ",
            "K7P3Q✨",
        ] {
            let err = normalize_join_code(code).expect_err("accepted {code}");
            assert_eq!(err.code(), "invalid_argument", "accepted {code:?}");
        }
    }

    #[test]
    fn ids_are_checked_before_they_reach_a_url() {
        assert!(validate_id("lobby_id", "lobby-1").is_ok());
        assert!(validate_id("lobby_id", &"a".repeat(MAX_ID_LEN)).is_ok());

        for bad in [
            "",
            "lobby/../evil",
            "lobby 1",
            "lobby?x=1",
            "lobby#f",
            "lobbÿ",
        ] {
            let err = validate_id("lobby_id", bad).expect_err("accepted");
            assert_eq!(err.code(), "invalid_argument", "accepted {bad:?}");
        }
        assert_eq!(
            validate_id("lobby_id", &"a".repeat(MAX_ID_LEN + 1))
                .expect_err("too long")
                .code(),
            "invalid_argument"
        );
    }

    #[test]
    fn the_wire_lobby_maps_onto_the_public_struct() {
        let lobby = parse_lobby(LOBBY);

        assert_eq!(lobby.lobby_id, "lobby-1");
        assert_eq!(lobby.join_code.as_deref(), Some("K7P3QX"));
        assert_eq!(lobby.host_user_id, "user-host");
        assert_eq!(lobby.host_payload, b"udp://10.0.0.1:7777");
        assert_eq!(lobby.max_players, 4);
        assert_eq!(lobby.members.len(), 2);
        assert_eq!(lobby.members[0].pseudo, "Ada");
        assert_eq!(lobby.members[0].payload, b"udp://10.0.0.1:7777");
        assert!(lobby.members[1].payload.is_empty());
    }

    #[test]
    fn a_null_join_code_reads_as_no_code() {
        let lobby = parse_lobby(
            r#"{"lobby_id":"lobby-1","join_code":null,"host_user_id":"user-host",
                "max_players":2,"members":[]}"#,
        );

        assert_eq!(lobby.join_code, None);
        assert!(lobby.host_payload.is_empty());
    }

    #[test]
    fn a_payload_that_is_not_base64_fails_the_lobby_rather_than_reading_as_empty() {
        let wire: WireLobby = serde_json::from_str(
            r#"{"lobby_id":"lobby-1","host_user_id":"u","host_payload":"not base64!!","members":[]}"#,
        )
        .expect("wire lobby");

        let err = map_lobby(wire).expect_err("bad base64");
        assert_eq!(err.code(), "arcane_unavailable");
        assert!(err
            .context()
            .iter()
            .any(|(k, v)| k == "field" && v == "host_payload"));
    }

    fn parse_events(raw: &str) -> Vec<LobbyEvent> {
        let wire: WireEvents = serde_json::from_str(raw).expect("wire events");
        wire.events.into_iter().filter_map(map_event).collect()
    }

    #[test]
    fn every_event_type_maps_and_unknown_ones_are_skipped() {
        let events = parse_events(
            r#"{"events":[
                {"id":"1","type":"invite","lobby_id":"lobby-1","join_code":"K7P3QX",
                 "from_user_id":"user-a","pseudo":"Ada"},
                {"id":"2","type":"member_joined","lobby_id":"lobby-1","user_id":"user-b",
                 "pseudo":"Bo","payload":"dWRwOi8vMTAuMC4wLjI6Nzc3Nw=="},
                {"id":"3","type":"member_left","lobby_id":"lobby-1","user_id":"user-b"},
                {"id":"4","type":"lobby_closed","lobby_id":"lobby-1"},
                {"id":"5","type":"lobby_renamed","lobby_id":"lobby-1"},
                {"id":"6","type":"invite","lobby_id":""}
            ],"cursor":"c-2"}"#,
        );

        assert_eq!(events.len(), 4, "{events:?}");
        assert_eq!(
            events[0],
            LobbyEvent::Invite {
                lobby_id: "lobby-1".into(),
                join_code: Some("K7P3QX".into()),
                from_user_id: "user-a".into(),
                pseudo: "Ada".into(),
            }
        );
        assert_eq!(
            events[1],
            LobbyEvent::MemberJoined {
                lobby_id: "lobby-1".into(),
                user_id: "user-b".into(),
                pseudo: "Bo".into(),
                payload: b"udp://10.0.0.2:7777".to_vec(),
            }
        );
        assert_eq!(events[2].lobby_id(), "lobby-1");
        assert_eq!(
            events[3],
            LobbyEvent::LobbyClosed {
                lobby_id: "lobby-1".into()
            }
        );
    }

    #[test]
    fn an_event_payload_that_is_not_base64_arrives_empty() {
        let events = parse_events(
            r#"{"events":[{"type":"member_joined","lobby_id":"lobby-1","user_id":"user-b",
                 "pseudo":"Bo","payload":"not base64!!"}]}"#,
        );

        assert_eq!(
            events[0],
            LobbyEvent::MemberJoined {
                lobby_id: "lobby-1".into(),
                user_id: "user-b".into(),
                pseudo: "Bo".into(),
                payload: Vec::new(),
            }
        );
    }

    #[test]
    fn the_events_path_carries_the_cursor_only_once_there_is_one() {
        assert_eq!(
            events_path("pk_test", None),
            "/v1/games/pk_test/lobbies/events"
        );
        assert_eq!(
            events_path("pk_test", Some("c-2")),
            "/v1/games/pk_test/lobbies/events?after=c-2"
        );
        assert_eq!(
            events_path("pk_test", Some("a b&c=1/2")),
            "/v1/games/pk_test/lobbies/events?after=a%20b%26c%3D1%2F2"
        );
    }

    fn state() -> P2pState {
        P2pState::new()
    }

    #[test]
    fn polling_is_off_until_the_accessor_is_built() {
        let state = state();
        assert_eq!(state.polling(), LobbyPollingState::Off);
        assert!(!state.armed());

        P2p::new("pk_test", &state);

        assert_eq!(state.polling(), LobbyPollingState::Active);
        assert!(state.armed());
    }

    #[test]
    fn an_unavailable_desktop_retires_polling_for_good() {
        let state = state();
        P2p::new("pk_test", &state);
        state.retire();

        assert!(!state.armed());
        assert_eq!(state.polling(), LobbyPollingState::Unavailable);

        P2p::new("pk_test", &state);

        assert!(!state.armed(), "a later p2p() call must not re-arm polling");
        assert_eq!(state.polling(), LobbyPollingState::Unavailable);
    }

    #[test]
    fn the_poll_period_drops_to_five_seconds_inside_a_lobby() {
        let state = state();
        let tick = Duration::from_secs(60);
        assert_eq!(state.poll_period(tick), tick);

        state.enter("lobby-1");
        assert_eq!(state.poll_period(tick), IN_LOBBY_POLL);
        assert_eq!(
            state.poll_period(Duration::from_millis(50)),
            Duration::from_millis(50),
            "the poll never runs slower than the session tick"
        );

        state.exit("lobby-1");
        assert_eq!(state.poll_period(tick), tick);
    }

    #[test]
    fn a_closed_lobby_event_clears_the_in_lobby_state() {
        let state = state();
        state.enter("lobby-1");
        state.enter("lobby-2");

        state.ingest(
            vec![LobbyEvent::LobbyClosed {
                lobby_id: "lobby-1".into(),
            }],
            Some("c-2".into()),
        );

        assert_eq!(state.poll_period(Duration::from_secs(60)), IN_LOBBY_POLL);
        state.exit("lobby-2");
        assert_eq!(
            state.poll_period(Duration::from_secs(60)),
            Duration::from_secs(60)
        );
        assert_eq!(state.lock().cursor.as_deref(), Some("c-2"));
    }

    #[test]
    fn an_empty_cursor_does_not_replace_the_one_we_have() {
        let state = state();
        state.ingest(Vec::new(), Some("c-1".into()));
        state.ingest(Vec::new(), None);
        state.ingest(Vec::new(), Some(String::new()));

        assert_eq!(state.lock().cursor.as_deref(), Some("c-1"));
    }

    #[test]
    fn events_are_drained_once_and_in_order() {
        let state = state();
        state.ingest(
            vec![
                LobbyEvent::MemberLeft {
                    lobby_id: "lobby-1".into(),
                    user_id: "user-b".into(),
                },
                LobbyEvent::LobbyClosed {
                    lobby_id: "lobby-1".into(),
                },
            ],
            None,
        );

        let first = state.take_events();
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].lobby_id(), "lobby-1");
        assert!(matches!(first[1], LobbyEvent::LobbyClosed { .. }));
        assert!(state.take_events().is_empty());
    }

    #[test]
    fn the_queue_is_bounded_and_drops_the_oldest_events() {
        let state = state();
        for index in 0..MAX_QUEUED_EVENTS + 10 {
            state.ingest(
                vec![LobbyEvent::MemberLeft {
                    lobby_id: format!("lobby-{index}"),
                    user_id: "user-b".into(),
                }],
                None,
            );
        }

        let drained = state.take_events();
        assert_eq!(drained.len(), MAX_QUEUED_EVENTS);
        assert_eq!(drained[0].lobby_id(), "lobby-10");
    }

    #[test]
    fn rendering_the_queue_does_not_drain_it_and_discard_takes_exactly_what_was_rendered() {
        let state = state();
        state.ingest(
            vec![LobbyEvent::MemberLeft {
                lobby_id: "lobby-1".into(),
                user_id: "user-b".into(),
            }],
            None,
        );

        let (rendered, count) = state.events_json();
        assert_eq!(count, 1);
        assert_eq!(
            rendered,
            r#"{"events":[{"type":"member_left","lobby_id":"lobby-1","user_id":"user-b"}]}"#
        );

        state.ingest(
            vec![LobbyEvent::LobbyClosed {
                lobby_id: "lobby-1".into(),
            }],
            None,
        );
        state.discard(count);

        let left = state.take_events();
        assert_eq!(left.len(), 1, "an event queued after the render survives");
        assert!(matches!(left[0], LobbyEvent::LobbyClosed { .. }));
    }

    #[test]
    fn the_lobby_json_keeps_the_documented_field_order() {
        let rendered = to_json(&parse_lobby(LOBBY));

        assert_eq!(
            rendered,
            r#"{"lobby_id":"lobby-1","join_code":"K7P3QX","host_user_id":"user-host","host_payload":"dWRwOi8vMTAuMC4wLjE6Nzc3Nw==","members":[{"user_id":"user-host","pseudo":"Ada","payload":"dWRwOi8vMTAuMC4wLjE6Nzc3Nw=="},{"user_id":"user-b","pseudo":"Bo","payload":""}],"max_players":4}"#
        );
    }

    #[test]
    fn every_event_renders_with_its_type_first() {
        let rendered = render_events(
            [
                LobbyEvent::Invite {
                    lobby_id: "lobby-1".into(),
                    join_code: None,
                    from_user_id: "user-a".into(),
                    pseudo: "Ada".into(),
                },
                LobbyEvent::MemberJoined {
                    lobby_id: "lobby-1".into(),
                    user_id: "user-b".into(),
                    pseudo: "Bo".into(),
                    payload: b"hi".to_vec(),
                },
            ]
            .iter(),
        );

        let parsed: serde_json::Value = serde_json::from_str(&rendered).expect("json");
        assert_eq!(parsed["events"][0]["type"], "invite");
        assert_eq!(parsed["events"][0]["join_code"], serde_json::Value::Null);
        assert_eq!(parsed["events"][1]["type"], "member_joined");
        assert_eq!(parsed["events"][1]["payload"], "aGk=");
        assert!(rendered.starts_with(r#"{"events":[{"type":"invite","lobby_id":"lobby-1""#));
    }

    #[test]
    fn an_empty_queue_still_renders_a_valid_document() {
        let (rendered, count) = state().events_json();
        assert_eq!(rendered, r#"{"events":[]}"#);
        assert_eq!(count, 0);
    }
}
