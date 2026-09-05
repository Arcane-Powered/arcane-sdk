//! Minimal C ABI for Unity / Unreal / native engines.
//!
//! The client is a process-wide singleton: call `arcane_sdk_init` once at launch,
//! then read state with the getters. No handle to carry around.
//!
//! Two return conventions:
//!
//! - **Actions** (`arcane_sdk_init`, `arcane_sdk_refresh`,
//!   `arcane_sdk_set_graphics`, `arcane_sdk_achievement_unlock`) return `0` on
//!   success, `1` on a bad argument (`arcane_sdk_init` takes none, so it never
//!   does), `2` on an SDK error whose `"code: message"` rendering is written
//!   into `err_buf`.
//! - **Getters** return the number of bytes written (excluding the NUL) when they
//!   succeed, or a negative value: `-1` not initialised, `-2` bad argument,
//!   `-3` buffer too small, `-4` value not available.
//!
//! Every failure is also recorded for `arcane_sdk_last_error_json`.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_longlong};
use std::sync::RwLock;

use crate::achievements;
use crate::client::ArcaneClient;
use crate::error::{OwnershipStatus, SdkError};
use crate::friends;
use crate::p2p::{self, Visibility};

/// Action return codes.
pub const ARCANE_OK: c_int = 0;
pub const ARCANE_ERR_ARGUMENT: c_int = 1;
pub const ARCANE_ERR_SDK: c_int = 2;

/// Getter return codes (negative).
pub const ARCANE_ERR_NOT_INITIALIZED: c_int = -1;
pub const ARCANE_ERR_BAD_BUFFER: c_int = -2;
pub const ARCANE_ERR_BUFFER_TOO_SMALL: c_int = -3;
pub const ARCANE_ERR_UNAVAILABLE: c_int = -4;

/// `arcane_sdk_ownership` results.
pub const ARCANE_OWNERSHIP_OWNED: c_int = 0;
pub const ARCANE_OWNERSHIP_DRM_DISABLED: c_int = 1;

/// `arcane_sdk_lobby_create` visibility values.
pub const ARCANE_LOBBY_FRIENDS: c_int = 0;
pub const ARCANE_LOBBY_CODE: c_int = 1;
pub const ARCANE_LOBBY_FRIENDS_AND_CODE: c_int = 2;

static CLIENT: RwLock<Option<ArcaneClient>> = RwLock::new(None);
static LAST_ERROR: RwLock<Option<SdkError>> = RwLock::new(None);

fn store_error(err: &SdkError) {
    let mut slot = LAST_ERROR.write().unwrap_or_else(|e| e.into_inner());
    *slot = Some(err.clone());
}

fn clear_error() {
    let mut slot = LAST_ERROR.write().unwrap_or_else(|e| e.into_inner());
    *slot = None;
}

/// Copy `value` into `buf` as a NUL-terminated C string.
///
/// Returns the byte count written excluding the NUL, or a negative error code.
/// Never writes a partial string: on overflow the buffer is left as an empty
/// C string so callers cannot mistake truncated output for a real value.
fn write_str(value: &str, buf: *mut c_char, len: usize) -> c_int {
    if buf.is_null() || len == 0 {
        return ARCANE_ERR_BAD_BUFFER;
    }
    let Ok(c_value) = CString::new(value) else {
        return ARCANE_ERR_UNAVAILABLE;
    };
    let bytes = c_value.as_bytes_with_nul();
    if bytes.len() > len {
        unsafe { *buf = 0 };
        return ARCANE_ERR_BUFFER_TOO_SMALL;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf as *mut u8, bytes.len());
    }
    (bytes.len() - 1) as c_int
}

/// Write an error into the caller's buffer, truncating rather than failing —
/// a partial diagnostic still beats none.
fn write_err(err: &SdkError, buf: *mut c_char, len: usize) {
    store_error(err);
    if buf.is_null() || len == 0 {
        return;
    }
    let rendered = err.to_string().replace('\0', "");
    let c_value = CString::new(rendered).unwrap_or_default();
    let bytes = c_value.as_bytes_with_nul();
    let copy_len = bytes.len().min(len);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf as *mut u8, copy_len);
        *buf.add(copy_len - 1) = 0;
    }
}

/// Clone the singleton out of its lock, so a blocking loopback call never holds
/// the lock the render thread and the lifecycle entry points need. The clone
/// shares the session and the achievement cache, so updates still land on the
/// singleton.
fn client_snapshot() -> Option<ArcaneClient> {
    let guard = CLIENT.read().unwrap_or_else(|e| e.into_inner());
    guard.as_ref().cloned()
}

/// Read a client field, or return the appropriate negative code.
fn with_client<F>(buf: *mut c_char, len: usize, read: F) -> c_int
where
    F: FnOnce(&ArcaneClient) -> Option<String>,
{
    let guard = CLIENT.read().unwrap_or_else(|e| e.into_inner());
    let Some(client) = guard.as_ref() else {
        let err = SdkError::not_initialized("The Arcane SDK client is not initialised.")
            .with_hint("Call arcane_sdk_init once at launch before reading client state.");
        store_error(&err);
        return ARCANE_ERR_NOT_INITIALIZED;
    };
    match read(client) {
        Some(value) => write_str(&value, buf, len),
        None => ARCANE_ERR_UNAVAILABLE,
    }
}

/// Verify ownership and build the process-wide client. Call once at launch.
///
/// You pass no id: Arcane Powered puts the game id of this title in
/// `ARCANE_GAME_ID` and the signed-in account in `ARCANE_USER_ID` when it
/// launches the game, and the SDK reads both. For local development you set
/// them yourself.
///
/// Returns 0 on success, 2 on an SDK error written to `err_buf` as
/// `"code: message"` — `missing_game_id` when `ARCANE_GAME_ID` is not set.
///
/// # Safety
///
/// `err_buf` must be null or point to at least `err_len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_init(err_buf: *mut c_char, err_len: usize) -> c_int {
    match ArcaneClient::init() {
        Ok(client) => {
            *CLIENT.write().unwrap_or_else(|e| e.into_inner()) = Some(client);
            clear_error();
            ARCANE_OK
        }
        Err(err) => {
            write_err(&err, err_buf, err_len);
            ARCANE_ERR_SDK
        }
    }
}

/// Re-run the ownership check against Arcane desktop and update the client.
///
/// Returns 0 on success, 1 if the client is not initialised, 2 on an SDK error.
/// On failure the client keeps its previous state.
///
/// # Safety
///
/// `err_buf` must be null or point to at least `err_len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_refresh(err_buf: *mut c_char, err_len: usize) -> c_int {
    let mut guard = CLIENT.write().unwrap_or_else(|e| e.into_inner());
    let Some(client) = guard.as_mut() else {
        let err = SdkError::not_initialized("The Arcane SDK client is not initialised.")
            .with_hint("Call arcane_sdk_init once at launch before calling arcane_sdk_refresh.");
        write_err(&err, err_buf, err_len);
        return ARCANE_ERR_ARGUMENT;
    };
    match client.refresh() {
        Ok(_) => {
            clear_error();
            ARCANE_OK
        }
        Err(err) => {
            write_err(&err, err_buf, err_len);
            ARCANE_ERR_SDK
        }
    }
}

/// End the play session and drop the client.
///
/// Reports the final playtime to the Arcane desktop app with a 2-second timeout,
/// then releases the singleton. Call it when the game exits; it is also what you
/// want on an editor play-mode reload.
#[no_mangle]
pub extern "C" fn arcane_sdk_shutdown() {
    let client = { CLIENT.write().unwrap_or_else(|e| e.into_inner()).take() };
    if let Some(client) = client {
        client.shutdown();
    }
    clear_error();
}

/// Count one rendered frame, for FPS sampling. Call once per frame.
///
/// Outside a sampling window this is a relaxed atomic load; inside one it adds a
/// relaxed increment. Does nothing before `arcane_sdk_init` succeeds.
#[no_mangle]
pub extern "C" fn arcane_sdk_frame() {
    let guard = CLIENT.read().unwrap_or_else(|e| e.into_inner());
    if let Some(client) = guard.as_ref() {
        client.frame();
    }
}

/// Record the current display settings, attached to the FPS samples that follow.
///
/// Both strings are NUL-terminated UTF-8, for example `"2560x1440"` and
/// `"high"`. Empty strings clear the values. Never call this from the render
/// loop — it takes a short lock.
///
/// Returns 0 on success, 1 on a null / non-UTF-8 argument or when the client is
/// not initialised.
///
/// # Safety
///
/// `resolution` and `preset` must be valid NUL-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_set_graphics(
    resolution: *const c_char,
    preset: *const c_char,
) -> c_int {
    if resolution.is_null() || preset.is_null() {
        return ARCANE_ERR_ARGUMENT;
    }
    let (Ok(resolution), Ok(preset)) = (
        CStr::from_ptr(resolution).to_str(),
        CStr::from_ptr(preset).to_str(),
    ) else {
        return ARCANE_ERR_ARGUMENT;
    };

    let guard = CLIENT.read().unwrap_or_else(|e| e.into_inner());
    let Some(client) = guard.as_ref() else {
        let err = SdkError::not_initialized("The Arcane SDK client is not initialised.")
            .with_hint("Call arcane_sdk_init once at launch before arcane_sdk_set_graphics.");
        store_error(&err);
        return ARCANE_ERR_ARGUMENT;
    };
    client.set_graphics(resolution, preset);
    ARCANE_OK
}

/// Write the play session state as JSON into `buf`:
/// `{"session_id","tracking","played_seconds","fps_sampling","samples_taken","last_fps_avg"}`.
///
/// `tracking` is `"active"`, `"pending"` or `"disabled"`. 256 bytes is enough.
///
/// # Safety
///
/// `buf` must be null or point to at least `len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_session_json(buf: *mut c_char, len: usize) -> c_int {
    with_client(buf, len, |c| Some(c.session().to_json()))
}

/// Write every achievement of this title as JSON into `buf`:
/// `{"achievements":[{"key","title","description","icon_url","hidden","unlocked_at"}]}`.
///
/// `unlocked_at` is a Unix timestamp, or `null` while the achievement is locked.
/// This makes one synchronous loopback call — call it on a loading screen or the
/// achievements screen, never from the render loop. It also fills the cache
/// `arcane_sdk_achievement_is_unlocked` reads.
///
/// Returns the bytes written, or `-1` when not initialised, `-2` / `-3` for the
/// buffer, `-4` when the call failed — the failure is then readable with
/// `arcane_sdk_last_error_json`.
///
/// # Safety
///
/// `buf` must be null or point to at least `len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_achievements_json(buf: *mut c_char, len: usize) -> c_int {
    let Some(client) = client_snapshot() else {
        let err = SdkError::not_initialized("The Arcane SDK client is not initialised.")
            .with_hint("Call arcane_sdk_init once at launch before reading achievements.");
        store_error(&err);
        return ARCANE_ERR_NOT_INITIALIZED;
    };
    match client.achievements().list() {
        Ok(list) => {
            clear_error();
            write_str(&achievements::to_json(&list), buf, len)
        }
        Err(err) => {
            store_error(&err);
            ARCANE_ERR_UNAVAILABLE
        }
    }
}

/// Unlock an achievement for the signed-in player.
///
/// `key` is the achievement key from the Arcane portal, NUL-terminated UTF-8.
/// Idempotent — call it every time the condition holds. One synchronous loopback
/// call, so never call it from the render loop.
///
/// Returns 0 on success (including an already-unlocked or queued answer), 1 if
/// `key` is null or not UTF-8 or the client is not initialised, 2 on an SDK
/// error written to `err_buf` as `"code: message"`.
///
/// # Safety
///
/// `key` must be a valid NUL-terminated C string. `err_buf` must be null or
/// point to at least `err_len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_achievement_unlock(
    key: *const c_char,
    err_buf: *mut c_char,
    err_len: usize,
) -> c_int {
    if key.is_null() {
        return ARCANE_ERR_ARGUMENT;
    }
    let Ok(key) = CStr::from_ptr(key).to_str() else {
        return ARCANE_ERR_ARGUMENT;
    };

    let Some(client) = client_snapshot() else {
        let err = SdkError::not_initialized("The Arcane SDK client is not initialised.")
            .with_hint("Call arcane_sdk_init once at launch before arcane_sdk_achievement_unlock.");
        write_err(&err, err_buf, err_len);
        return ARCANE_ERR_ARGUMENT;
    };
    match client.achievements().unlock(key) {
        Ok(_) => {
            clear_error();
            ARCANE_OK
        }
        Err(err) => {
            write_err(&err, err_buf, err_len);
            ARCANE_ERR_SDK
        }
    }
}

/// Whether an achievement is unlocked, from the cache the last
/// `arcane_sdk_achievements_json` call filled.
///
/// Reads memory only. Returns 1 (unlocked), 0 (locked), `-1` when the client is
/// not initialised, `-2` when `key` is null or not UTF-8, and `-4` when the SDK
/// has nothing to answer with: the list was never loaded, or it did not carry
/// this key.
///
/// # Safety
///
/// `key` must be a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_achievement_is_unlocked(key: *const c_char) -> c_int {
    if key.is_null() {
        return ARCANE_ERR_BAD_BUFFER;
    }
    let Ok(key) = CStr::from_ptr(key).to_str() else {
        return ARCANE_ERR_BAD_BUFFER;
    };

    let guard = CLIENT.read().unwrap_or_else(|e| e.into_inner());
    let Some(client) = guard.as_ref() else {
        return ARCANE_ERR_NOT_INITIALIZED;
    };
    match client.achievements().is_unlocked(key) {
        Some(unlocked) => c_int::from(unlocked),
        None => ARCANE_ERR_UNAVAILABLE,
    }
}

/// Write this player's friends as JSON into `buf`:
/// `{"friends":[{"user_id","pseudo","online","in_game"}],"stale":bool}`.
///
/// `in_game` is `true` for a friend playing this title right now. `stale` is
/// `true` when the Arcane desktop app answered from its cache because it is
/// offline. This makes one synchronous loopback call — call it when a menu
/// opens or on a timer of your own, never from the render loop.
///
/// Returns the bytes written, or `-1` when not initialised, `-2` / `-3` for the
/// buffer, `-4` when the call failed — the failure is then readable with
/// `arcane_sdk_last_error_json`.
///
/// # Safety
///
/// `buf` must be null or point to at least `len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_friends_json(buf: *mut c_char, len: usize) -> c_int {
    let Some(client) = client_snapshot() else {
        let err = SdkError::not_initialized("The Arcane SDK client is not initialised.")
            .with_hint("Call arcane_sdk_init once at launch before reading friends.");
        store_error(&err);
        return ARCANE_ERR_NOT_INITIALIZED;
    };
    match client.friends().list() {
        Ok(list) => {
            clear_error();
            write_str(&friends::to_json(&list), buf, len)
        }
        Err(err) => {
            store_error(&err);
            ARCANE_ERR_UNAVAILABLE
        }
    }
}

/// Read a NUL-terminated C string, treating null as absent.
///
/// # Safety
///
/// `value` must be null or a valid NUL-terminated C string.
unsafe fn c_str<'a>(value: *const c_char) -> Option<Option<&'a str>> {
    if value.is_null() {
        return Some(None);
    }
    match CStr::from_ptr(value).to_str() {
        Ok(value) => Some(Some(value)),
        Err(_) => None,
    }
}

/// Decode a base64 payload argument. A null pointer is an empty payload; a
/// string that is not base64 is a bad argument.
///
/// # Safety
///
/// `payload_b64` must be null or a valid NUL-terminated C string.
unsafe fn payload_arg(payload_b64: *const c_char) -> Option<Vec<u8>> {
    let raw = c_str(payload_b64)?.unwrap_or_default();
    p2p::decode_payload_arg(raw)
}

fn visibility_arg(visibility: c_int) -> Option<Visibility> {
    match visibility {
        ARCANE_LOBBY_FRIENDS => Some(Visibility::Friends),
        ARCANE_LOBBY_CODE => Some(Visibility::Code),
        ARCANE_LOBBY_FRIENDS_AND_CODE => Some(Visibility::FriendsAndCode),
        _ => None,
    }
}

fn bad_lobby_argument(detail: &str) -> c_int {
    let err = SdkError::invalid_argument(detail)
        .with_hint("Pass a base64 payload of at most 4096 raw bytes, and a known visibility.");
    store_error(&err);
    ARCANE_ERR_BAD_BUFFER
}

fn lobby_result(lobby: Result<crate::p2p::Lobby, SdkError>, buf: *mut c_char, len: usize) -> c_int {
    match lobby {
        Ok(lobby) => {
            clear_error();
            write_str(&p2p::to_json(&lobby), buf, len)
        }
        Err(err) => {
            store_error(&err);
            ARCANE_ERR_UNAVAILABLE
        }
    }
}

fn lobby_client(action: &str) -> Result<ArcaneClient, SdkError> {
    client_snapshot().ok_or_else(|| {
        SdkError::not_initialized("The Arcane SDK client is not initialised.").with_hint(format!(
            "Call arcane_sdk_init once at launch before {action}."
        ))
    })
}

/// Open a lobby with this player as its host and write it as JSON into `buf`.
///
/// `visibility` is `ARCANE_LOBBY_FRIENDS` (0), `ARCANE_LOBBY_CODE` (1) or
/// `ARCANE_LOBBY_FRIENDS_AND_CODE` (2). `payload_b64` is your connection blob,
/// base64-encoded, at most 4096 raw bytes — null means no payload. The JSON is
/// `{"lobby_id","join_code","host_user_id","host_payload","members":[{"user_id",
/// "pseudo","payload"}],"max_players"}`, payloads base64.
///
/// One synchronous loopback call — never from the render loop. Returns the
/// bytes written, or `-1` when not initialised, `-2` on a bad argument or
/// buffer, `-3` when the buffer is too small, `-4` when the call failed — then
/// readable with `arcane_sdk_last_error_json`.
///
/// # Safety
///
/// `payload_b64` must be null or a valid NUL-terminated C string. `buf` must be
/// null or point to at least `len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_lobby_create(
    max_players: u8,
    visibility: c_int,
    payload_b64: *const c_char,
    buf: *mut c_char,
    len: usize,
) -> c_int {
    let Some(payload) = payload_arg(payload_b64) else {
        return bad_lobby_argument("The lobby payload is null-terminated but not base64.");
    };
    let Some(visibility) = visibility_arg(visibility) else {
        return bad_lobby_argument("That is not an Arcane lobby visibility.");
    };
    let client = match lobby_client("arcane_sdk_lobby_create") {
        Ok(client) => client,
        Err(err) => {
            store_error(&err);
            return ARCANE_ERR_NOT_INITIALIZED;
        }
    };

    lobby_result(
        client.p2p().create_lobby(max_players, visibility, &payload),
        buf,
        len,
    )
}

/// Join the lobby a six-character code points at, and write it as JSON into
/// `buf`.
///
/// The code is uppercased before it is checked. Same JSON, return values and
/// threading rules as `arcane_sdk_lobby_create`; a malformed code gives `-4`
/// with `invalid_argument` in `arcane_sdk_last_error_json`, raised before any
/// call.
///
/// # Safety
///
/// `join_code` and `payload_b64` must be null or valid NUL-terminated C
/// strings. `buf` must be null or point to at least `len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_lobby_join_code(
    join_code: *const c_char,
    payload_b64: *const c_char,
    buf: *mut c_char,
    len: usize,
) -> c_int {
    let Some(Some(join_code)) = c_str(join_code) else {
        return bad_lobby_argument("The join code is null or not UTF-8.");
    };
    let Some(payload) = payload_arg(payload_b64) else {
        return bad_lobby_argument("The lobby payload is null-terminated but not base64.");
    };
    let client = match lobby_client("arcane_sdk_lobby_join_code") {
        Ok(client) => client,
        Err(err) => {
            store_error(&err);
            return ARCANE_ERR_NOT_INITIALIZED;
        }
    };

    lobby_result(client.p2p().join_by_code(join_code, &payload), buf, len)
}

/// Join a lobby by id — what an invite event carries — and write it as JSON
/// into `buf`.
///
/// Same JSON, return values and threading rules as `arcane_sdk_lobby_create`.
///
/// # Safety
///
/// `lobby_id` and `payload_b64` must be null or valid NUL-terminated C strings.
/// `buf` must be null or point to at least `len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_lobby_join(
    lobby_id: *const c_char,
    payload_b64: *const c_char,
    buf: *mut c_char,
    len: usize,
) -> c_int {
    let Some(Some(lobby_id)) = c_str(lobby_id) else {
        return bad_lobby_argument("The lobby id is null or not UTF-8.");
    };
    let Some(payload) = payload_arg(payload_b64) else {
        return bad_lobby_argument("The lobby payload is null-terminated but not base64.");
    };
    let client = match lobby_client("arcane_sdk_lobby_join") {
        Ok(client) => client,
        Err(err) => {
            store_error(&err);
            return ARCANE_ERR_NOT_INITIALIZED;
        }
    };

    lobby_result(client.p2p().join(lobby_id, &payload), buf, len)
}

/// Read a lobby as Arcane knows it right now and write it as JSON into `buf`.
///
/// Use it after a `resync` event, or whenever you would rather ask than replay
/// events. Same JSON, return values and threading rules as
/// `arcane_sdk_lobby_create`; it joins nothing and leaves nothing.
///
/// # Safety
///
/// `lobby_id` must be null or a valid NUL-terminated C string. `buf` must be
/// null or point to at least `len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_lobby_get(
    lobby_id: *const c_char,
    buf: *mut c_char,
    len: usize,
) -> c_int {
    let Some(Some(lobby_id)) = c_str(lobby_id) else {
        return bad_lobby_argument("The lobby id is null or not UTF-8.");
    };
    let client = match lobby_client("arcane_sdk_lobby_get") {
        Ok(client) => client,
        Err(err) => {
            store_error(&err);
            return ARCANE_ERR_NOT_INITIALIZED;
        }
    };

    lobby_result(client.p2p().get_lobby(lobby_id), buf, len)
}

/// Invite one friend to a lobby.
///
/// Both ids are NUL-terminated UTF-8. One synchronous loopback call. Returns 0
/// on success, 1 on a null / non-UTF-8 argument or before init, 2 on an SDK
/// error written to `err_buf` as `"code: message"` — `not_friends` when that
/// account is not a friend.
///
/// # Safety
///
/// `lobby_id` and `to_user_id` must be valid NUL-terminated C strings.
/// `err_buf` must be null or point to at least `err_len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_lobby_invite(
    lobby_id: *const c_char,
    to_user_id: *const c_char,
    err_buf: *mut c_char,
    err_len: usize,
) -> c_int {
    let (Some(Some(lobby_id)), Some(Some(to_user_id))) = (c_str(lobby_id), c_str(to_user_id))
    else {
        return ARCANE_ERR_ARGUMENT;
    };
    let client = match lobby_client("arcane_sdk_lobby_invite") {
        Ok(client) => client,
        Err(err) => {
            write_err(&err, err_buf, err_len);
            return ARCANE_ERR_ARGUMENT;
        }
    };

    match client.p2p().invite(lobby_id, to_user_id) {
        Ok(()) => {
            clear_error();
            ARCANE_OK
        }
        Err(err) => {
            write_err(&err, err_buf, err_len);
            ARCANE_ERR_SDK
        }
    }
}

/// Leave a lobby. For the host this ends it — there is no host migration.
///
/// Same return values as `arcane_sdk_lobby_invite`.
///
/// # Safety
///
/// `lobby_id` must be a valid NUL-terminated C string. `err_buf` must be null
/// or point to at least `err_len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_lobby_leave(
    lobby_id: *const c_char,
    err_buf: *mut c_char,
    err_len: usize,
) -> c_int {
    lobby_action(
        lobby_id,
        err_buf,
        err_len,
        "arcane_sdk_lobby_leave",
        |client, id| client.p2p().leave(id),
    )
}

/// Close a lobby this player hosts. Its members get a `lobby_closed` event.
///
/// Same return values as `arcane_sdk_lobby_invite`.
///
/// # Safety
///
/// `lobby_id` must be a valid NUL-terminated C string. `err_buf` must be null
/// or point to at least `err_len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_lobby_close(
    lobby_id: *const c_char,
    err_buf: *mut c_char,
    err_len: usize,
) -> c_int {
    lobby_action(
        lobby_id,
        err_buf,
        err_len,
        "arcane_sdk_lobby_close",
        |client, id| client.p2p().close(id),
    )
}

/// # Safety
///
/// `lobby_id` must be a valid NUL-terminated C string. `err_buf` must be null
/// or point to at least `err_len` writable bytes.
unsafe fn lobby_action<F>(
    lobby_id: *const c_char,
    err_buf: *mut c_char,
    err_len: usize,
    action: &str,
    call: F,
) -> c_int
where
    F: FnOnce(&ArcaneClient, &str) -> Result<(), SdkError>,
{
    let Some(Some(lobby_id)) = c_str(lobby_id) else {
        return ARCANE_ERR_ARGUMENT;
    };
    let client = match lobby_client(action) {
        Ok(client) => client,
        Err(err) => {
            write_err(&err, err_buf, err_len);
            return ARCANE_ERR_ARGUMENT;
        }
    };

    match call(&client, lobby_id) {
        Ok(()) => {
            clear_error();
            ARCANE_OK
        }
        Err(err) => {
            write_err(&err, err_buf, err_len);
            ARCANE_ERR_SDK
        }
    }
}

/// Write the join code this game was launched with into `buf`.
///
/// Set when the player started the game from a friend's "Join" in the
/// launcher. Read from the Arcane desktop app on the first call and cached for
/// the process; `-4` when there is none, when the desktop app predates the
/// route, or in offline-only mode. 8 bytes is enough.
///
/// # Safety
///
/// `buf` must be null or point to at least `len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_launch_join_code(buf: *mut c_char, len: usize) -> c_int {
    let client = match lobby_client("arcane_sdk_launch_join_code") {
        Ok(client) => client,
        Err(err) => {
            store_error(&err);
            return ARCANE_ERR_NOT_INITIALIZED;
        }
    };
    match client.p2p().launch_join_code() {
        Some(code) => write_str(&code, buf, len),
        None => ARCANE_ERR_UNAVAILABLE,
    }
}

/// Write the lobby events collected since the last call as JSON into `buf`, and
/// drop them:
/// `{"events":[{"type":"invite|member_joined|member_left|lobby_closed","lobby_id",…}]}`.
///
/// Reads memory only — the `arcane-session` thread does the polling, armed by
/// the first lobby call. Payloads are base64. `member_joined` carries
/// `user_id`, `pseudo` and `payload`; `invite` carries `join_code`,
/// `from_user_id` and `pseudo`; `member_left` carries `user_id`.
///
/// The queue is drained **only** once the JSON is safely in `buf`: a `-3` keeps
/// every event so you can retry with a larger buffer.
///
/// # Safety
///
/// `buf` must be null or point to at least `len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_lobby_events_json(buf: *mut c_char, len: usize) -> c_int {
    let client = match lobby_client("arcane_sdk_lobby_events_json") {
        Ok(client) => client,
        Err(err) => {
            store_error(&err);
            return ARCANE_ERR_NOT_INITIALIZED;
        }
    };

    let (rendered, through) = client.p2p().events_json();
    let written = write_str(&rendered, buf, len);
    if let (true, Some(through)) = (written >= 0, through) {
        client.p2p().discard(through);
    }
    written
}

/// Whether a client is currently initialised. Returns 1 or 0.
#[no_mangle]
pub extern "C" fn arcane_sdk_is_initialized() -> c_int {
    let guard = CLIENT.read().unwrap_or_else(|e| e.into_inner());
    c_int::from(guard.is_some())
}

/// Ownership as of the last check.
///
/// Returns 0 (owned), 1 (DRM disabled for this title), or -1 if not initialised.
#[no_mangle]
pub extern "C" fn arcane_sdk_ownership() -> c_int {
    let guard = CLIENT.read().unwrap_or_else(|e| e.into_inner());
    match guard.as_ref().map(ArcaneClient::ownership) {
        Some(OwnershipStatus::Owned) => ARCANE_OWNERSHIP_OWNED,
        Some(OwnershipStatus::DrmDisabled) => ARCANE_OWNERSHIP_DRM_DISABLED,
        None => ARCANE_ERR_NOT_INITIALIZED,
    }
}

/// Unix timestamp of the ownership ticket's expiry, or -1 when unknown.
#[no_mangle]
pub extern "C" fn arcane_sdk_ticket_expires_at() -> c_longlong {
    let guard = CLIENT.read().unwrap_or_else(|e| e.into_inner());
    guard
        .as_ref()
        .and_then(ArcaneClient::ticket_expires_at)
        .unwrap_or(-1) as c_longlong
}

/// Unix timestamp of the last successful check, or -1 when not initialised.
#[no_mangle]
pub extern "C" fn arcane_sdk_checked_at() -> c_longlong {
    let guard = CLIENT.read().unwrap_or_else(|e| e.into_inner());
    guard.as_ref().map(ArcaneClient::checked_at).unwrap_or(-1) as c_longlong
}

/// Write the signed-in Arcane account id into `buf`.
///
/// # Safety
///
/// `buf` must be null or point to at least `len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_user_id(buf: *mut c_char, len: usize) -> c_int {
    with_client(buf, len, |c| c.user_id().map(str::to_string))
}

/// Write the game id this client was initialised with into `buf`.
///
/// # Safety
///
/// `buf` must be null or point to at least `len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_game_id(buf: *mut c_char, len: usize) -> c_int {
    with_client(buf, len, |c| Some(c.game_id().to_string()))
}

/// Write this machine's device fingerprint into `buf`.
///
/// # Safety
///
/// `buf` must be null or point to at least `len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_device_hash(buf: *mut c_char, len: usize) -> c_int {
    with_client(buf, len, |c| Some(c.device_hash().to_string()))
}

/// Write the last error as JSON into `buf`:
/// `{"code","message","hint","retryable","context"}`.
///
/// Returns -4 when no error has been recorded since the last success.
///
/// # Safety
///
/// `buf` must be null or point to at least `len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_last_error_json(buf: *mut c_char, len: usize) -> c_int {
    let guard = LAST_ERROR.read().unwrap_or_else(|e| e.into_inner());
    match guard.as_ref() {
        Some(err) => write_str(&err.to_json(), buf, len),
        None => ARCANE_ERR_UNAVAILABLE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `CLIENT` and `LAST_ERROR` are process-wide, so tests that touch them must
    /// not run concurrently.
    static GLOBAL_STATE: Mutex<()> = Mutex::new(());

    fn read_c_string(buf: &[c_char]) -> String {
        let bytes: Vec<u8> = buf
            .iter()
            .take_while(|b| **b != 0)
            .map(|b| *b as u8)
            .collect();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn write_str_reports_bytes_and_nul_terminates() {
        let mut buf = [1 as c_char; 16];
        let written = write_str("game-abc", buf.as_mut_ptr(), buf.len());
        assert_eq!(written, 8);
        assert_eq!(read_c_string(&buf), "game-abc");
        assert_eq!(buf[8], 0);
    }

    #[test]
    fn write_str_refuses_to_truncate() {
        let mut buf = [1 as c_char; 4];
        let written = write_str("game-abc", buf.as_mut_ptr(), buf.len());
        assert_eq!(written, ARCANE_ERR_BUFFER_TOO_SMALL);
        // Left as an empty C string, never a partial value.
        assert_eq!(buf[0], 0);
    }

    #[test]
    fn write_str_rejects_null_and_zero_length_buffers() {
        let mut buf = [0 as c_char; 8];
        assert_eq!(
            write_str("x", std::ptr::null_mut(), 8),
            ARCANE_ERR_BAD_BUFFER
        );
        assert_eq!(write_str("x", buf.as_mut_ptr(), 0), ARCANE_ERR_BAD_BUFFER);
    }

    #[test]
    fn write_str_exact_fit_succeeds() {
        let mut buf = [1 as c_char; 4];
        assert_eq!(write_str("abc", buf.as_mut_ptr(), buf.len()), 3);
        assert_eq!(read_c_string(&buf), "abc");
    }

    #[test]
    fn write_err_truncates_and_always_nul_terminates() {
        let err = SdkError::not_owned("You do not own this game.")
            .with_hint("Buy it on the Arcane Store.");
        let mut buf = [1 as c_char; 12];
        write_err(&err, buf.as_mut_ptr(), buf.len());
        assert_eq!(buf[buf.len() - 1], 0);
        assert!(read_c_string(&buf).starts_with("not_owned"));
    }

    #[test]
    fn write_err_records_the_error_for_last_error_json() {
        let _guard = GLOBAL_STATE.lock().unwrap_or_else(|e| e.into_inner());

        let err = SdkError::network_required("Go online once.").with_hint("Open Arcane.");
        write_err(&err, std::ptr::null_mut(), 0);

        let mut buf = [0 as c_char; 512];
        let written = unsafe { arcane_sdk_last_error_json(buf.as_mut_ptr(), buf.len()) };
        assert!(written > 0);
        let parsed: serde_json::Value = serde_json::from_str(&read_c_string(&buf)).unwrap();
        assert_eq!(parsed["code"], "network_required");
        assert_eq!(parsed["retryable"], true);

        clear_error();
    }

    #[test]
    fn getters_report_not_initialized_before_init() {
        let _guard = GLOBAL_STATE.lock().unwrap_or_else(|e| e.into_inner());

        arcane_sdk_shutdown();
        let mut buf = [0 as c_char; 64];

        assert_eq!(
            unsafe { arcane_sdk_user_id(buf.as_mut_ptr(), buf.len()) },
            ARCANE_ERR_NOT_INITIALIZED
        );
        assert_eq!(
            unsafe { arcane_sdk_game_id(buf.as_mut_ptr(), buf.len()) },
            ARCANE_ERR_NOT_INITIALIZED
        );
        assert_eq!(arcane_sdk_ownership(), ARCANE_ERR_NOT_INITIALIZED);
        assert_eq!(arcane_sdk_is_initialized(), 0);
        assert_eq!(arcane_sdk_ticket_expires_at(), -1);
        assert_eq!(arcane_sdk_checked_at(), -1);
    }

    #[test]
    fn frame_and_session_json_are_safe_before_init() {
        let _guard = GLOBAL_STATE.lock().unwrap_or_else(|e| e.into_inner());

        arcane_sdk_shutdown();
        arcane_sdk_frame();

        let mut buf = [0 as c_char; 256];
        assert_eq!(
            unsafe { arcane_sdk_session_json(buf.as_mut_ptr(), buf.len()) },
            ARCANE_ERR_NOT_INITIALIZED
        );
        assert_eq!(
            unsafe { arcane_sdk_set_graphics(c"1920x1080".as_ptr(), c"ultra".as_ptr()) },
            ARCANE_ERR_ARGUMENT
        );
    }

    #[test]
    fn the_achievement_entry_points_are_safe_before_init() {
        let _guard = GLOBAL_STATE.lock().unwrap_or_else(|e| e.into_inner());

        arcane_sdk_shutdown();
        let mut buf = [0 as c_char; 256];

        assert_eq!(
            unsafe { arcane_sdk_achievements_json(buf.as_mut_ptr(), buf.len()) },
            ARCANE_ERR_NOT_INITIALIZED
        );
        assert_eq!(
            unsafe {
                arcane_sdk_achievement_unlock(c"first_blood".as_ptr(), buf.as_mut_ptr(), buf.len())
            },
            ARCANE_ERR_ARGUMENT
        );
        assert_eq!(
            unsafe { arcane_sdk_achievement_is_unlocked(c"first_blood".as_ptr()) },
            ARCANE_ERR_NOT_INITIALIZED
        );
    }

    #[test]
    fn the_friends_entry_point_is_safe_before_init() {
        let _guard = GLOBAL_STATE.lock().unwrap_or_else(|e| e.into_inner());

        arcane_sdk_shutdown();
        let mut buf = [0 as c_char; 256];

        assert_eq!(
            unsafe { arcane_sdk_friends_json(buf.as_mut_ptr(), buf.len()) },
            ARCANE_ERR_NOT_INITIALIZED
        );
    }

    #[test]
    fn the_achievement_entry_points_reject_a_null_key() {
        let mut buf = [0 as c_char; 64];
        assert_eq!(
            unsafe { arcane_sdk_achievement_unlock(std::ptr::null(), buf.as_mut_ptr(), buf.len()) },
            ARCANE_ERR_ARGUMENT
        );
        assert_eq!(
            unsafe { arcane_sdk_achievement_is_unlocked(std::ptr::null()) },
            ARCANE_ERR_BAD_BUFFER
        );
    }

    #[test]
    fn set_graphics_rejects_null_arguments() {
        assert_eq!(
            unsafe { arcane_sdk_set_graphics(std::ptr::null(), std::ptr::null()) },
            ARCANE_ERR_ARGUMENT
        );
    }
}
