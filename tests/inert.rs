//! A game started outside Arcane Powered: no `ARCANE_GAME_ID`, no desktop app.
//! The SDK must fail fast, stay inert, and never crash or block the game.

use std::net::TcpListener;
use std::os::raw::c_char;
use std::time::{Duration, Instant};

use arcane_sdk::ffi;
use arcane_sdk::ArcaneClient;

fn unused_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("addr")
        .port()
}

fn read_c_string(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|b| **b != 0)
        .map(|b| *b as u8)
        .collect();
    String::from_utf8(bytes).expect("utf-8")
}

#[test]
fn a_game_started_outside_arcane_stays_inert() {
    std::env::remove_var("ARCANE_GAME_ID");
    std::env::remove_var("ARCANE_USER_ID");
    std::env::set_var("ARCANE_SDK_PORT", unused_port().to_string());

    let started = Instant::now();
    let err = ArcaneClient::init().expect_err("nothing named a title");
    assert_eq!(err.code(), "missing_game_id");

    let mut err_buf = [0 as c_char; 512];
    let rc = unsafe { ffi::arcane_sdk_init(err_buf.as_mut_ptr(), err_buf.len()) };
    assert_eq!(rc, ffi::ARCANE_ERR_SDK);
    assert!(read_c_string(&err_buf).starts_with("missing_game_id:"));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "init must fail fast, took {:?}",
        started.elapsed()
    );
    assert_eq!(ffi::arcane_sdk_is_initialized(), 0);

    let mut buf = [0 as c_char; 1024];
    let (ptr, len) = (buf.as_mut_ptr(), buf.len());
    unsafe {
        assert!(ffi::arcane_sdk_user_id(ptr, len) < 0);
        assert!(ffi::arcane_sdk_game_id(ptr, len) < 0);
        assert!(ffi::arcane_sdk_device_hash(ptr, len) < 0);
        assert!(ffi::arcane_sdk_session_json(ptr, len) < 0);
        assert!(ffi::arcane_sdk_achievements_json(ptr, len) < 0);
        assert!(ffi::arcane_sdk_friends_json(ptr, len) < 0);
        assert!(
            ffi::arcane_sdk_lobby_create(4, ffi::ARCANE_LOBBY_PUBLIC, std::ptr::null(), ptr, len)
                < 0
        );
        assert!(
            ffi::arcane_sdk_lobby_create_json(
                c"{\"max_players\":4,\"visibility\":\"public\"}".as_ptr(),
                ptr,
                len
            ) < 0
        );
        assert!(ffi::arcane_sdk_lobby_search(std::ptr::null(), ptr, len) < 0);
        assert!(
            ffi::arcane_sdk_lobby_set_metadata(
                c"lobby-1".as_ptr(),
                c"{\"metadata\":{}}".as_ptr(),
                ptr,
                len
            ) < 0
        );
        assert!(ffi::arcane_sdk_lobby_get(c"lobby-1".as_ptr(), ptr, len) < 0);
        assert!(ffi::arcane_sdk_lobby_join(c"lobby-1".as_ptr(), std::ptr::null(), ptr, len) < 0);
        assert!(
            ffi::arcane_sdk_lobby_join_code(c"K7P3QX".as_ptr(), std::ptr::null(), ptr, len) < 0
        );
        assert!(ffi::arcane_sdk_lobby_events_json(ptr, len) < 0);
        assert!(ffi::arcane_sdk_launch_join_code(ptr, len) < 0);

        assert_ne!(ffi::arcane_sdk_refresh(ptr, len), 0);
        assert_ne!(
            ffi::arcane_sdk_achievement_unlock(c"first_blood".as_ptr(), ptr, len),
            0
        );
        assert_ne!(
            ffi::arcane_sdk_lobby_invite(c"lobby-1".as_ptr(), c"user-b".as_ptr(), ptr, len),
            0
        );
        assert_ne!(
            ffi::arcane_sdk_lobby_leave(c"lobby-1".as_ptr(), ptr, len),
            0
        );
        assert_ne!(
            ffi::arcane_sdk_lobby_close(c"lobby-1".as_ptr(), ptr, len),
            0
        );
        assert!(ffi::arcane_sdk_achievement_is_unlocked(c"first_blood".as_ptr()) < 0);
    }
    assert!(ffi::arcane_sdk_ownership() < 0);
    assert_eq!(ffi::arcane_sdk_ticket_expires_at(), -1);
    assert_eq!(ffi::arcane_sdk_checked_at(), -1);
    ffi::arcane_sdk_frame();
    ffi::arcane_sdk_shutdown();

    assert!(
        started.elapsed() < Duration::from_secs(2),
        "every call must answer at once without a client, took {:?}",
        started.elapsed()
    );
}
