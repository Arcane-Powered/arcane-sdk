//! Minimal C ABI for Unity / Unreal / native engines.
//!
//! The client is a process-wide singleton: call `arcane_sdk_init` once at launch,
//! then read state with the getters. No handle to carry around.
//!
//! Two return conventions:
//!
//! - **Actions** (`arcane_sdk_init`, `arcane_sdk_refresh`,
//!   `arcane_sdk_set_graphics`) return `0` on success, `1` on a bad argument,
//!   `2` on an SDK error whose `"code: message"` rendering is written into
//!   `err_buf`.
//! - **Getters** return the number of bytes written (excluding the NUL) when they
//!   succeed, or a negative value: `-1` not initialised, `-2` bad argument,
//!   `-3` buffer too small, `-4` value not available.
//!
//! Every failure is also recorded for `arcane_sdk_last_error_json`.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_longlong};
use std::sync::RwLock;

use crate::client::ArcaneClient;
use crate::error::{OwnershipStatus, SdkError};

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
/// `public_key` must be a NUL-terminated UTF-8 string — the public key generated
/// for this title in the Arcane portal.
///
/// Returns 0 on success, 1 if `public_key` is null or not UTF-8, 2 on an SDK
/// error written to `err_buf` as `"code: message"`.
///
/// # Safety
///
/// `public_key` must be a valid NUL-terminated C string. `err_buf` must be null
/// or point to at least `err_len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_init(
    public_key: *const c_char,
    err_buf: *mut c_char,
    err_len: usize,
) -> c_int {
    if public_key.is_null() {
        return ARCANE_ERR_ARGUMENT;
    }
    let Ok(key) = CStr::from_ptr(public_key).to_str() else {
        return ARCANE_ERR_ARGUMENT;
    };

    match ArcaneClient::init(key) {
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

/// Write the canonical title id into `buf`.
///
/// # Safety
///
/// `buf` must be null or point to at least `len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_game_id(buf: *mut c_char, len: usize) -> c_int {
    with_client(buf, len, |c| c.game_id().map(str::to_string))
}

/// Write the public key this client was initialised with into `buf`.
///
/// # Safety
///
/// `buf` must be null or point to at least `len` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_public_key(buf: *mut c_char, len: usize) -> c_int {
    with_client(buf, len, |c| Some(c.public_key().to_string()))
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
        let written = write_str("pk_abc", buf.as_mut_ptr(), buf.len());
        assert_eq!(written, 6);
        assert_eq!(read_c_string(&buf), "pk_abc");
        assert_eq!(buf[6], 0);
    }

    #[test]
    fn write_str_refuses_to_truncate() {
        let mut buf = [1 as c_char; 4];
        let written = write_str("pk_abc", buf.as_mut_ptr(), buf.len());
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
            unsafe { arcane_sdk_public_key(buf.as_mut_ptr(), buf.len()) },
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
    fn set_graphics_rejects_null_arguments() {
        assert_eq!(
            unsafe { arcane_sdk_set_graphics(std::ptr::null(), std::ptr::null()) },
            ARCANE_ERR_ARGUMENT
        );
    }

    #[test]
    fn init_rejects_a_null_public_key() {
        let mut err = [0 as c_char; 64];
        let rc = unsafe { arcane_sdk_init(std::ptr::null(), err.as_mut_ptr(), err.len()) };
        assert_eq!(rc, ARCANE_ERR_ARGUMENT);
    }
}
