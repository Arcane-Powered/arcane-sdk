//! Minimal C ABI for Unity / Unreal / native engines.
//!
//! Return codes: 0 = ok (owned or DRM disabled), non-zero = error (see err buffer).

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};

use crate::{arcane_init as rust_arcane_init, check_ownership_offline, SdkError};

fn write_err(err: &SdkError, buf: *mut c_char, len: usize) {
    if buf.is_null() || len == 0 {
        return;
    }
    let msg = format!("{}: {}", err.code(), err);
    let c = CString::new(msg.replace('\0', "")).unwrap_or_default();
    let bytes = c.as_bytes_with_nul();
    let copy_len = bytes.len().min(len);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf as *mut u8, copy_len);
        if copy_len > 0 {
            *buf.add(copy_len - 1) = 0;
        }
    }
}

/// Default launch check (portal public key). Returns 0 on success, 1 on bad args, 2 on SdkError.
///
/// `game_id` must be a valid NUL-terminated UTF-8 C string (portal public key).
#[no_mangle]
pub unsafe extern "C" fn arcane_sdk_init(
    game_id: *const c_char,
    err_buf: *mut c_char,
    err_len: usize,
) -> c_int {
    if game_id.is_null() {
        return 1;
    }
    let id = match CStr::from_ptr(game_id).to_str() {
        Ok(s) => s,
        Err(_) => return 1,
    };
    match rust_arcane_init(id) {
        Ok(_) => 0,
        Err(e) => {
            write_err(&e, err_buf, err_len);
            2
        }
    }
}

/// Force offline ownership check. Returns 0 on success, 1 on bad args, 2 on SdkError.
///
/// `game_id` must be a valid NUL-terminated UTF-8 C string (portal public key).
#[no_mangle]
pub unsafe extern "C" fn arcane_check_ownership_offline(
    game_id: *const c_char,
    err_buf: *mut c_char,
    err_len: usize,
) -> c_int {
    if game_id.is_null() {
        return 1;
    }
    let id = match CStr::from_ptr(game_id).to_str() {
        Ok(s) => s,
        Err(_) => return 1,
    };
    match check_ownership_offline(id) {
        Ok(_) => 0,
        Err(e) => {
            write_err(&e, err_buf, err_len);
            2
        }
    }
}
