//! The desktop loopback path, against a stub HTTP server on `ARCANE_SDK_PORT`.
//!
//! No ticket is ever written, so `init` always falls through to the refresh
//! branch — which is exactly the path under test.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Mutex, MutexGuard};
use std::thread;

use arcane_sdk::{ArcaneClient, OwnershipStatus};
use tempfile::TempDir;

const PUBLIC_KEY: &str = "pk_test_title";

/// `ARCANE_SDK_PORT` and `ARCANE_DRM_ROOT` are process-global.
static ENV_LOCK: Mutex<()> = Mutex::new(());

struct Reply {
    status: &'static str,
    body: &'static str,
}

struct Stub {
    _dir: TempDir,
    _guard: MutexGuard<'static, ()>,
}

impl Stub {
    /// Serve `health` on `GET /v1/health` and `refresh` on the ownership POST.
    fn start(health: Reply, refresh: Reply) -> Self {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = TempDir::new().expect("temp dir");

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();

        std::env::set_var("ARCANE_DRM_ROOT", dir.path());
        std::env::set_var("ARCANE_SDK_PORT", port.to_string());
        std::env::remove_var("ARCANE_OFFLINE_ONLY");

        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve(stream, &health, &refresh);
            }
        });

        Self {
            _dir: dir,
            _guard: guard,
        }
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        std::env::remove_var("ARCANE_DRM_ROOT");
        std::env::remove_var("ARCANE_SDK_PORT");
    }
}

fn serve(mut stream: TcpStream, health: &Reply, refresh: &Reply) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    // Drain headers so the client sees a clean request/response pair.
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) if line == "\r\n" || line == "\n" => break,
            Ok(_) => continue,
            Err(_) => return,
        }
    }

    let reply = if request_line.contains("/v1/health") {
        health
    } else {
        refresh
    };

    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        reply.status,
        reply.body.len(),
        reply.body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

#[test]
fn a_refresh_that_reports_drm_off_succeeds_without_a_ticket() {
    let _stub = Stub::start(
        Reply {
            status: "200 OK",
            body: r#"{"ok":true,"authenticated":true,"user_id":"user-from-health"}"#,
        },
        Reply {
            status: "200 OK",
            body: r#"{"ok":true,"drm_enabled":false,"game_id":"game-canonical-id"}"#,
        },
    );

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");

    assert_eq!(client.ownership(), OwnershipStatus::DrmDisabled);
    // user_id comes from /v1/health, game_id from the refresh response.
    assert_eq!(client.user_id(), Some("user-from-health"));
    assert_eq!(client.game_id(), Some("game-canonical-id"));
}

#[test]
fn a_refresh_response_field_wins_over_the_health_field() {
    let _stub = Stub::start(
        Reply {
            status: "200 OK",
            body: r#"{"ok":true,"authenticated":true,"user_id":"user-from-health"}"#,
        },
        Reply {
            status: "200 OK",
            body: r#"{"ok":true,"drm_enabled":false,"user_id":"user-from-refresh"}"#,
        },
    );

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");

    assert_eq!(client.user_id(), Some("user-from-refresh"));
}

#[test]
fn a_desktop_error_body_maps_to_its_sdk_code() {
    let _stub = Stub::start(
        Reply {
            status: "200 OK",
            body: r#"{"ok":true,"authenticated":true}"#,
        },
        Reply {
            status: "403 Forbidden",
            body: r#"{"error":"not_owned","message":"This account does not own the game."}"#,
        },
    );

    let err = ArcaneClient::init(PUBLIC_KEY).expect_err("not owned");

    assert_eq!(err.code(), "not_owned");
    assert!(!err.is_retryable());
    assert!(err.hint().expect("hint").contains("Arcane Store"));
}

#[test]
fn a_signed_out_desktop_is_reported_before_the_refresh_call() {
    let _stub = Stub::start(
        Reply {
            status: "200 OK",
            body: r#"{"ok":true,"authenticated":false}"#,
        },
        Reply {
            status: "500 Internal Server Error",
            body: r#"{"error":"internal","message":"should never be reached"}"#,
        },
    );

    let err = ArcaneClient::init(PUBLIC_KEY).expect_err("signed out");

    assert_eq!(err.code(), "not_authenticated");
    assert!(err.is_retryable());
}

#[test]
fn an_unhealthy_desktop_is_reported_as_unavailable() {
    let _stub = Stub::start(
        Reply {
            status: "200 OK",
            body: r#"{"ok":false,"authenticated":true}"#,
        },
        Reply {
            status: "200 OK",
            body: r#"{"ok":true,"drm_enabled":false}"#,
        },
    );

    let err = ArcaneClient::init(PUBLIC_KEY).expect_err("unhealthy");

    assert_eq!(err.code(), "arcane_unavailable");
}

#[test]
fn an_unknown_desktop_error_code_keeps_the_original_in_context() {
    let _stub = Stub::start(
        Reply {
            status: "200 OK",
            body: r#"{"ok":true,"authenticated":true}"#,
        },
        Reply {
            status: "451 Unavailable For Legal Reasons",
            body: r#"{"error":"region_locked","message":"not available in your region"}"#,
        },
    );

    let err = ArcaneClient::init(PUBLIC_KEY).expect_err("region locked");

    assert_eq!(err.code(), "ticket_invalid");
    assert!(err
        .context()
        .iter()
        .any(|(k, v)| k == "desktop_error" && v == "region_locked"));
    assert!(err.hint().expect("hint").contains("update arcane-sdk"));
}

#[test]
fn an_unreadable_error_body_still_names_the_status_and_url() {
    let _stub = Stub::start(
        Reply {
            status: "200 OK",
            body: r#"{"ok":true,"authenticated":true}"#,
        },
        Reply {
            status: "502 Bad Gateway",
            body: "<html>proxy exploded</html>",
        },
    );

    let err = ArcaneClient::init(PUBLIC_KEY).expect_err("bad gateway");

    let keys: Vec<&str> = err.context().iter().map(|(k, _)| k.as_str()).collect();
    assert!(keys.contains(&"http_status"));
    assert!(keys.contains(&"url"));
    assert!(keys.contains(&"body"));
}
