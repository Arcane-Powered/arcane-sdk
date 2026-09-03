//! The desktop loopback path, against a stub HTTP server on `ARCANE_SDK_PORT`.
//!
//! Ownership tests write no ticket, so `init` always falls through to the
//! refresh branch — which is exactly the path under test. Session tests drive
//! the `arcane-session` thread with a short `ARCANE_SESSION_TICK_MS`.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use std::os::raw::c_char;

use arcane_sdk::ffi;
use arcane_sdk::{ArcaneClient, LobbyEvent, OwnershipStatus, TrackingState, Visibility};
use tempfile::TempDir;

const PUBLIC_KEY: &str = "pk_test_title";
const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// `ARCANE_SDK_PORT` and `ARCANE_DRM_ROOT` are process-global.
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy)]
struct Reply {
    status: &'static str,
    body: &'static str,
}

#[derive(Clone, Debug)]
struct Request {
    line: String,
    body: String,
}

impl Request {
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).expect("request body is json")
    }
}

struct Stub {
    dir: TempDir,
    _guard: MutexGuard<'static, ()>,
    log: Arc<Mutex<Vec<Request>>>,
}

impl Stub {
    /// Serve `health` on `GET /v1/health` and `refresh` on everything else.
    fn start(health: Reply, refresh: Reply) -> Self {
        Self::start_with(move |request| {
            if request.line.contains("/v1/health") {
                health
            } else {
                refresh
            }
        })
    }

    fn start_with<F>(handler: F) -> Self
    where
        F: Fn(&Request) -> Reply + Send + Sync + 'static,
    {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = TempDir::new().expect("temp dir");

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();

        std::env::set_var("ARCANE_DRM_ROOT", dir.path());
        std::env::set_var("ARCANE_SDK_PORT", port.to_string());
        std::env::remove_var("ARCANE_OFFLINE_ONLY");
        std::env::remove_var("ARCANE_SESSION_TICK_MS");

        let log = Arc::new(Mutex::new(Vec::new()));
        let served = Arc::clone(&log);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                serve(stream, &handler, &served);
            }
        });

        Self {
            dir,
            _guard: guard,
            log,
        }
    }

    fn drm_root(&self) -> &Path {
        self.dir.path()
    }

    fn requests(&self) -> Vec<Request> {
        self.log.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn matching(&self, fragment: &str) -> Vec<Request> {
        self.requests()
            .into_iter()
            .filter(|request| request.line.contains(fragment))
            .collect()
    }

    fn wait_for(&self, fragment: &str, count: usize) -> Vec<Request> {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            let matching = self.matching(fragment);
            if matching.len() >= count {
                return matching;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {count} × {fragment}, saw {:?}",
                self.requests()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn write_drm_off_ticket(&self, user_id: &str) {
        write_file(
            &self.drm_root().join("session.json"),
            &format!(r#"{{"user_id":"{user_id}","updated_at":1}}"#),
        );
        write_file(
            &self
                .drm_root()
                .join("tickets")
                .join(user_id)
                .join(format!("{PUBLIC_KEY}.ticket")),
            &format!(
                r#"{{
                    "ticket": "",
                    "cached_at": "2026-01-01T00:00:00Z",
                    "expires_at": "2027-01-01T00:00:00Z",
                    "game_id": "game-canonical-id",
                    "user_id": "{user_id}",
                    "device_hash": "",
                    "drm_enabled": false
                }}"#
            ),
        );
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        std::env::remove_var("ARCANE_OFFLINE_ONLY");
        std::env::remove_var("ARCANE_DRM_ROOT");
        std::env::remove_var("ARCANE_SDK_PORT");
        std::env::remove_var("ARCANE_SESSION_TICK_MS");
    }
}

/// Read a NUL-terminated string a C ABI getter wrote.
fn read_c_string(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect();
    String::from_utf8(bytes).expect("the C ABI writes utf-8")
}

fn write_file(path: &PathBuf, body: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
    fs::write(path, body).expect("write file");
}

fn serve<F>(mut stream: TcpStream, handler: &F, log: &Arc<Mutex<Vec<Request>>>)
where
    F: Fn(&Request) -> Reply,
{
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) if line == "\r\n" || line == "\n" => break,
            Ok(_) => {
                let lowered = line.to_ascii_lowercase();
                if let Some(value) = lowered.strip_prefix("content-length:") {
                    content_length = value.trim().parse().unwrap_or(0);
                }
            }
            Err(_) => return,
        }
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }

    let request = Request {
        line: request_line,
        body: String::from_utf8_lossy(&body).to_string(),
    };
    let reply = handler(&request);
    log.lock().unwrap_or_else(|e| e.into_inner()).push(request);

    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        reply.status,
        reply.body.len(),
        reply.body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

const HEALTHY: Reply = Reply {
    status: "200 OK",
    body: r#"{"ok":true,"authenticated":true,"user_id":"user-from-health"}"#,
};

const DRM_OFF: Reply = Reply {
    status: "200 OK",
    body: r#"{"ok":true,"drm_enabled":false,"game_id":"game-canonical-id"}"#,
};

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

#[test]
fn a_session_that_cannot_start_never_fails_init() {
    let stub = Stub::start_with(|request| {
        if request.line.contains("/v1/health") {
            HEALTHY
        } else if request.line.contains("/session/start") {
            Reply {
                status: "500 Internal Server Error",
                body: r#"{"error":"internal","message":"session store down"}"#,
            }
        } else {
            DRM_OFF
        }
    });

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init succeeds on ownership alone");
    stub.wait_for("/session/start", 1);

    let session = client.session();
    assert_eq!(session.tracking, TrackingState::Pending);
    assert_eq!(session.session_id, None);
    assert!(!session.fps_sampling);
    assert_eq!(session.samples_taken, 0);
}

#[test]
fn a_desktop_without_the_session_routes_leaves_init_alone() {
    let stub = Stub::start_with(|request| {
        if request.line.contains("/v1/health") {
            HEALTHY
        } else if request.line.contains("/session/") {
            Reply {
                status: "404 Not Found",
                body: "Not Found",
            }
        } else {
            DRM_OFF
        }
    });

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");
    stub.wait_for("/session/start", 1);

    assert_eq!(client.session().tracking, TrackingState::Pending);
}

#[test]
fn a_started_session_reports_active_and_the_player_sampling_setting() {
    let stub = Stub::start_with(|request| {
        if request.line.contains("/v1/health") {
            HEALTHY
        } else if request.line.contains("/session/start") {
            Reply {
                status: "200 OK",
                body: r#"{"session_id":"session-1","user_id":"user-from-health","game_id":"game-canonical-id","fps_sampling":true}"#,
            }
        } else {
            DRM_OFF
        }
    });

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");
    stub.wait_for("/session/start", 1);
    let session = await_active(&client);

    assert_eq!(session.session_id.as_deref(), Some("session-1"));
    assert!(session.fps_sampling);

    client.frame();
    client.set_graphics("2560x1440", "high");
    assert_eq!(client.session().samples_taken, 0);
}

#[test]
fn an_unknown_session_triggers_a_new_start() {
    let stub = Stub::start_with(|request| {
        if request.line.contains("/v1/health") {
            HEALTHY
        } else if request.line.contains("/session/start") {
            Reply {
                status: "200 OK",
                body: r#"{"session_id":"session-1","fps_sampling":false}"#,
            }
        } else if request.line.contains("/session/heartbeat") {
            Reply {
                status: "404 Not Found",
                body: r#"{"error":"unknown_session","message":"the desktop expired it"}"#,
            }
        } else {
            DRM_OFF
        }
    });
    std::env::set_var("ARCANE_SESSION_TICK_MS", "150");

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");
    stub.wait_for("/session/heartbeat", 1);
    let starts = stub.wait_for("/session/start", 2);

    assert!(starts.len() >= 2);
    assert_eq!(client.session().session_id.as_deref(), Some("session-1"));
}

#[test]
fn shutdown_ends_the_session_with_the_cumulative_seconds() {
    let stub = Stub::start_with(|request| {
        if request.line.contains("/v1/health") {
            HEALTHY
        } else if request.line.contains("/session/start") {
            Reply {
                status: "200 OK",
                body: r#"{"session_id":"session-1","fps_sampling":false}"#,
            }
        } else if request.line.contains("/session/heartbeat") {
            Reply {
                status: "200 OK",
                body: r#"{"ok":true,"fps_sampling":false}"#,
            }
        } else if request.line.contains("/session/end") {
            Reply {
                status: "200 OK",
                body: r#"{"ok":true}"#,
            }
        } else {
            DRM_OFF
        }
    });
    std::env::set_var("ARCANE_SESSION_TICK_MS", "150");

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");
    stub.wait_for("/session/heartbeat", 1);

    let heartbeat = stub.matching("/session/heartbeat")[0].json();
    assert_eq!(heartbeat["session_id"], "session-1");
    assert_eq!(heartbeat["samples"], serde_json::json!([]));

    thread::sleep(Duration::from_millis(1_100));
    client.shutdown();

    let ends = stub.matching("/session/end");
    assert_eq!(ends.len(), 1, "shutdown sends exactly one end");
    let body = ends[0].json();
    assert_eq!(body["session_id"], "session-1");
    assert!(
        body["seconds"].as_u64().expect("seconds") >= 1,
        "end carries the cumulative seconds, got {}",
        body["seconds"]
    );
}

#[test]
fn a_cached_ticket_starts_a_session_without_ever_contacting_the_desktop_for_ownership() {
    let stub = Stub::start_with(|request| {
        if request.line.contains("/session/start") {
            Reply {
                status: "200 OK",
                body: r#"{"session_id":"session-1","fps_sampling":false}"#,
            }
        } else {
            Reply {
                status: "500 Internal Server Error",
                body: r#"{"error":"internal","message":"init must not call this"}"#,
            }
        }
    });
    stub.write_drm_off_ticket("user-a");

    let client = ArcaneClient::init(PUBLIC_KEY).expect("a cached ticket is enough");

    assert_eq!(client.ownership(), OwnershipStatus::DrmDisabled);
    assert_eq!(client.user_id(), Some("user-a"));

    stub.wait_for("/session/start", 1);
    assert!(
        stub.matching("/v1/health").is_empty(),
        "init probed the desktop, so it could have opened the deep link"
    );
    assert!(stub.matching("ownership/refresh").is_empty());
    await_active(&client);
}

fn await_active(client: &ArcaneClient) -> arcane_sdk::SessionSnapshot {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    loop {
        let session = client.session();
        if session.tracking == TrackingState::Active {
            return session;
        }
        assert!(
            Instant::now() < deadline,
            "session never became active: {session:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

const ACHIEVEMENTS: Reply = Reply {
    status: "200 OK",
    body: r#"{"achievements":[
        {"key":"first_blood","title":"First blood","description":"Win a duel.",
         "icon_url":"https://cdn.arcane/first.png","hidden":false,
         "unlocked_at":"2026-05-01T12:34:56Z"},
        {"key":"boss.01","title":"The gatekeeper","description":"Beat the first boss.",
         "icon_url":null,"hidden":true,"unlocked_at":null}
    ]}"#,
};

/// Serve the achievement routes; everything else follows the ownership path.
fn achievement_stub(unlock: Reply, list: Reply) -> Stub {
    Stub::start_with(move |request| {
        if request.line.contains("/v1/health") {
            HEALTHY
        } else if request.line.contains("/unlock") {
            unlock
        } else if request.line.contains("/achievements") {
            list
        } else {
            DRM_OFF
        }
    })
}

const UNLOCK_OK: Reply = Reply {
    status: "200 OK",
    body: r#"{"key":"first_blood","unlocked_at":"2026-05-01T12:34:56Z",
              "already_unlocked":false,"queued":false}"#,
};

#[test]
fn listing_achievements_fills_the_cache_that_is_unlocked_reads() {
    let stub = achievement_stub(UNLOCK_OK, ACHIEVEMENTS);

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");
    assert_eq!(
        client.achievements().is_unlocked("first_blood"),
        None,
        "nothing is known before list()"
    );

    let list = client.achievements().list().expect("list");

    assert_eq!(list.len(), 2);
    assert_eq!(list[0].key, "first_blood");
    assert_eq!(list[0].title, "First blood");
    assert_eq!(
        list[0].icon_url.as_deref(),
        Some("https://cdn.arcane/first.png")
    );
    assert_eq!(list[0].unlocked_at, Some(1_777_638_896));
    assert!(list[1].hidden);
    assert_eq!(list[1].unlocked_at, None);

    assert_eq!(client.achievements().is_unlocked("first_blood"), Some(true));
    assert_eq!(client.achievements().is_unlocked("boss.01"), Some(false));
    assert_eq!(client.achievements().is_unlocked("never_defined"), None);
    assert_eq!(
        client.clone().achievements().is_unlocked("first_blood"),
        Some(true),
        "clones share the cache"
    );

    let listed = stub.matching("/achievements");
    assert_eq!(listed.len(), 1);
    assert!(listed[0]
        .line
        .starts_with("GET /v1/games/pk_test_title/achievements"));
}

#[test]
fn unlocking_reports_an_already_unlocked_achievement_as_a_success() {
    let stub = achievement_stub(
        Reply {
            status: "200 OK",
            body: r#"{"key":"first_blood","unlocked_at":"2026-05-01T12:34:56Z",
                      "already_unlocked":true,"queued":false}"#,
        },
        ACHIEVEMENTS,
    );

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");
    let unlock = client.achievements().unlock("first_blood").expect("unlock");

    assert_eq!(unlock.key, "first_blood");
    assert_eq!(unlock.unlocked_at, 1_777_638_896);
    assert!(unlock.already_unlocked);
    assert!(!unlock.queued);

    let posted = stub.matching("/unlock");
    assert_eq!(posted.len(), 1);
    assert!(posted[0]
        .line
        .starts_with("POST /v1/games/pk_test_title/achievements/first_blood/unlock"));
}

#[test]
fn a_queued_unlock_is_a_success_and_updates_the_cache() {
    let _stub = achievement_stub(
        Reply {
            status: "200 OK",
            body: r#"{"key":"boss.01","unlocked_at":"2026-05-01T12:34:56Z",
                      "already_unlocked":false,"queued":true}"#,
        },
        ACHIEVEMENTS,
    );

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");
    client.achievements().list().expect("list");
    assert_eq!(client.achievements().is_unlocked("boss.01"), Some(false));

    let unlock = client
        .achievements()
        .unlock("boss.01")
        .expect("queued unlock");

    assert!(unlock.queued);
    assert!(!unlock.already_unlocked);
    assert_eq!(unlock.unlocked_at, 1_777_638_896);
    assert_eq!(client.achievements().is_unlocked("boss.01"), Some(true));
}

#[test]
fn an_unknown_achievement_is_its_own_error_code() {
    let _stub = achievement_stub(
        Reply {
            status: "404 Not Found",
            body: r#"{"error":"unknown_achievement","message":"no such key for this title"}"#,
        },
        ACHIEVEMENTS,
    );

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");
    let err = client
        .achievements()
        .unlock("not_in_the_portal")
        .expect_err("unknown achievement");

    assert_eq!(err.code(), "unknown_achievement");
    assert!(!err.is_retryable());
    assert!(err
        .context()
        .iter()
        .any(|(k, v)| k == "achievement_key" && v == "not_in_the_portal"));
}

#[test]
fn a_desktop_without_the_achievement_routes_degrades_to_feature_unavailable() {
    let bare_404 = Reply {
        status: "404 Not Found",
        body: "Not Found",
    };
    let _stub = achievement_stub(bare_404, bare_404);

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");

    let listed = client.achievements().list().expect_err("no route");
    assert_eq!(listed.code(), "feature_unavailable");
    assert_eq!(client.achievements().is_unlocked("first_blood"), None);

    let unlocked = client
        .achievements()
        .unlock("first_blood")
        .expect_err("no route");
    assert_eq!(unlocked.code(), "feature_unavailable");
    assert!(unlocked.hint().expect("hint").contains("Update"));
}

#[test]
fn an_invalid_key_fails_before_any_request_is_sent() {
    let stub = achievement_stub(UNLOCK_OK, ACHIEVEMENTS);

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");
    let too_long = "a".repeat(65);
    for key in [
        "",
        "first blood",
        "first/../blood",
        "First_Blood",
        ".",
        "..",
        too_long.as_str(),
    ] {
        let err = client.achievements().unlock(key).expect_err("invalid key");
        assert_eq!(err.code(), "invalid_argument", "accepted {key:?}");
    }

    assert!(
        stub.matching("/achievements").is_empty(),
        "an invalid key must not reach the desktop: {:?}",
        stub.requests()
    );
}

#[test]
fn a_json_404_with_a_code_this_sdk_does_not_know_is_still_feature_unavailable() {
    let not_found = Reply {
        status: "404 Not Found",
        body: r#"{"error":"not_found","message":"no such route"}"#,
    };
    let _stub = achievement_stub(not_found, not_found);

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");

    assert_eq!(
        client.achievements().list().expect_err("no route").code(),
        "feature_unavailable"
    );
    assert_eq!(
        client
            .achievements()
            .unlock("first_blood")
            .expect_err("no route")
            .code(),
        "feature_unavailable"
    );
}

#[test]
fn offline_only_mode_refuses_both_achievement_calls_without_a_request() {
    let stub = achievement_stub(UNLOCK_OK, ACHIEVEMENTS);
    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");

    std::env::set_var("ARCANE_OFFLINE_ONLY", "1");

    let listed = client.achievements().list().expect_err("offline only");
    assert_eq!(listed.code(), "network_required");
    assert!(listed.is_retryable());

    let unlocked = client
        .achievements()
        .unlock("first_blood")
        .expect_err("offline only");
    assert_eq!(unlocked.code(), "network_required");
    assert!(unlocked
        .context()
        .iter()
        .any(|(k, v)| k == "env" && v == "ARCANE_OFFLINE_ONLY"));

    assert!(
        stub.matching("/achievements").is_empty(),
        "offline-only mode must not reach the desktop: {:?}",
        stub.requests()
    );

    std::env::remove_var("ARCANE_OFFLINE_ONLY");
}

#[test]
fn the_c_abi_singleton_sees_the_cache_filled_through_its_clone() {
    let _stub = achievement_stub(
        Reply {
            status: "200 OK",
            body: r#"{"key":"boss.01","unlocked_at":"2026-05-01T12:34:56Z",
                      "already_unlocked":false,"queued":false}"#,
        },
        ACHIEVEMENTS,
    );

    assert_eq!(
        unsafe { ffi::arcane_sdk_init(c"pk_test_title".as_ptr(), std::ptr::null_mut(), 0) },
        0
    );
    assert_eq!(
        unsafe { ffi::arcane_sdk_achievement_is_unlocked(c"first_blood".as_ptr()) },
        -4,
        "nothing is known before the list is loaded"
    );

    let mut buf = [0 as c_char; 1024];
    assert!(unsafe { ffi::arcane_sdk_achievements_json(buf.as_mut_ptr(), buf.len()) } > 0);

    assert_eq!(
        unsafe { ffi::arcane_sdk_achievement_is_unlocked(c"first_blood".as_ptr()) },
        1
    );
    assert_eq!(
        unsafe { ffi::arcane_sdk_achievement_is_unlocked(c"boss.01".as_ptr()) },
        0
    );
    assert_eq!(
        unsafe { ffi::arcane_sdk_achievement_unlock(c"boss.01".as_ptr(), std::ptr::null_mut(), 0) },
        0
    );
    assert_eq!(
        unsafe { ffi::arcane_sdk_achievement_is_unlocked(c"boss.01".as_ptr()) },
        1,
        "the unlock landed on the singleton's cache, not on a detached clone"
    );

    ffi::arcane_sdk_shutdown();
}

#[test]
fn a_key_the_desktop_rejects_is_reported_as_an_invalid_argument() {
    let _stub = achievement_stub(
        Reply {
            status: "400 Bad Request",
            body: r#"{"error":"invalid_key","message":"achievement keys are lowercase"}"#,
        },
        ACHIEVEMENTS,
    );

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");
    let err = client
        .achievements()
        .unlock("first_blood")
        .expect_err("rejected key");

    assert_eq!(err.code(), "invalid_argument");
    assert!(!err.is_retryable());
    assert!(err.hint().expect("hint").contains("lowercase"));
}

const FRIENDS: Reply = Reply {
    status: "200 OK",
    body: r#"{"friends":[
        {"user_id":"user-a","pseudo":"Ada","online":true,
         "playing_game_id":"game-canonical-id"},
        {"user_id":"user-b","pseudo":"Bo","online":true,
         "playing_game_id":"another-game"},
        {"user_id":"user-c","pseudo":"Cy","online":false,"playing_game_id":null}
    ],"stale":false}"#,
};

/// Serve `GET /v1/friends`; everything else follows the ownership path, which
/// reports `game_id` so `in_game` can be derived.
fn friends_stub(friends: Reply) -> Stub {
    Stub::start_with(move |request| {
        if request.line.contains("/v1/health") {
            HEALTHY
        } else if request.line.contains("/v1/friends") {
            friends
        } else {
            DRM_OFF
        }
    })
}

#[test]
fn listing_friends_marks_the_ones_playing_this_title() {
    let stub = friends_stub(FRIENDS);

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");
    assert_eq!(client.game_id(), Some("game-canonical-id"));

    let list = client.friends().list().expect("list");

    assert!(!list.stale);
    assert_eq!(list.friends.len(), 3);

    assert_eq!(list.friends[0].user_id, "user-a");
    assert_eq!(list.friends[0].pseudo, "Ada");
    assert!(list.friends[0].online);
    assert!(list.friends[0].in_game);

    assert!(list.friends[1].online);
    assert!(!list.friends[1].in_game, "another title is not this one");

    assert!(!list.friends[2].online);
    assert!(!list.friends[2].in_game);

    let listed = stub.matching("/v1/friends");
    assert_eq!(listed.len(), 1);
    assert!(listed[0].line.starts_with("GET /v1/friends"));
}

#[test]
fn a_stale_friend_list_is_a_success_that_says_so() {
    let _stub = friends_stub(Reply {
        status: "200 OK",
        body: r#"{"friends":[{"user_id":"user-a","pseudo":"Ada","online":true,
                  "playing_game_id":"game-canonical-id"}],"stale":true}"#,
    });

    let list = ArcaneClient::init(PUBLIC_KEY)
        .expect("init")
        .friends()
        .list()
        .expect("a stale list is still Ok");

    assert!(list.stale);
    assert!(list.friends[0].in_game);
}

#[test]
fn a_client_without_a_game_id_reports_nobody_in_game() {
    let _stub = Stub::start_with(|request| {
        if request.line.contains("/v1/health") {
            HEALTHY
        } else if request.line.contains("/v1/friends") {
            FRIENDS
        } else {
            Reply {
                status: "200 OK",
                body: r#"{"ok":true,"drm_enabled":false}"#,
            }
        }
    });

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");
    assert_eq!(client.game_id(), None);

    let list = client.friends().list().expect("list");

    assert!(list.friends.iter().all(|friend| !friend.in_game));
    assert!(list.friends[0].online, "presence still comes through");
}

#[test]
fn a_signed_out_desktop_fails_the_friend_list_with_not_authenticated() {
    let _stub = friends_stub(Reply {
        status: "401 Unauthorized",
        body: r#"{"error":"not_authenticated","message":"nobody is signed in"}"#,
    });

    let err = ArcaneClient::init(PUBLIC_KEY)
        .expect("init")
        .friends()
        .list()
        .expect_err("signed out");

    assert_eq!(err.code(), "not_authenticated");
    assert!(err.is_retryable());
}

#[test]
fn a_desktop_without_the_friends_route_degrades_to_feature_unavailable() {
    let _stub = friends_stub(Reply {
        status: "404 Not Found",
        body: "Not Found",
    });

    let err = ArcaneClient::init(PUBLIC_KEY)
        .expect("init")
        .friends()
        .list()
        .expect_err("no route");

    assert_eq!(err.code(), "feature_unavailable");
    assert!(err.hint().expect("hint").contains("Update"));
}

#[test]
fn offline_only_mode_refuses_the_friend_list_without_a_request() {
    let stub = friends_stub(FRIENDS);
    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");

    std::env::set_var("ARCANE_OFFLINE_ONLY", "1");

    let err = client.friends().list().expect_err("offline only");

    assert_eq!(err.code(), "network_required");
    assert!(err.is_retryable());
    assert!(err
        .context()
        .iter()
        .any(|(k, v)| k == "env" && v == "ARCANE_OFFLINE_ONLY"));
    assert!(
        stub.matching("/v1/friends").is_empty(),
        "offline-only mode must not reach the desktop: {:?}",
        stub.requests()
    );

    std::env::remove_var("ARCANE_OFFLINE_ONLY");
}

#[test]
fn the_c_abi_writes_the_friend_list_as_json() {
    let _stub = friends_stub(FRIENDS);

    assert_eq!(
        unsafe { ffi::arcane_sdk_init(c"pk_test_title".as_ptr(), std::ptr::null_mut(), 0) },
        0
    );

    let mut buf = [0 as c_char; 2048];
    let written = unsafe { ffi::arcane_sdk_friends_json(buf.as_mut_ptr(), buf.len()) };
    assert!(written > 0);

    let json: Vec<u8> = buf
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect();
    let parsed: serde_json::Value =
        serde_json::from_slice(&json).expect("the C ABI writes valid json");

    assert_eq!(parsed["stale"], false);
    assert_eq!(parsed["friends"][0]["user_id"], "user-a");
    assert_eq!(parsed["friends"][0]["pseudo"], "Ada");
    assert_eq!(parsed["friends"][0]["online"], true);
    assert_eq!(parsed["friends"][0]["in_game"], true);
    assert_eq!(parsed["friends"][1]["in_game"], false);

    ffi::arcane_sdk_shutdown();
}

#[test]
fn two_list_calls_make_two_requests_because_the_sdk_caches_nothing() {
    let stub = friends_stub(FRIENDS);
    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");

    let first = client.friends().list().expect("first list");
    let second = client.friends().list().expect("second list");

    assert_eq!(first, second);
    assert_eq!(
        stub.matching("/v1/friends").len(),
        2,
        "the desktop app owns the cache, the SDK holds no list of its own"
    );
}

/// A pseudo carrying every character class that has to survive JSON escaping on
/// the way through the C buffer: a quote, a backslash, a newline, an escaped
/// NUL, CJK and an emoji.
const AWKWARD_PSEUDO: &str = "A\"B\\C\nD\u{0}E 日本語 🎮";

const AWKWARD_FRIEND: Reply = Reply {
    status: "200 OK",
    body: "{\"friends\":[{\"user_id\":\"user-a\",\
           \"pseudo\":\"A\\\"B\\\\C\\nD\\u0000E 日本語 🎮\",\"online\":true,\
           \"playing_game_id\":\"game-canonical-id\"}],\"stale\":false}",
};

#[test]
fn the_c_abi_escapes_a_pseudo_that_carries_json_metacharacters() {
    let _stub = friends_stub(AWKWARD_FRIEND);

    assert_eq!(
        unsafe { ffi::arcane_sdk_init(c"pk_test_title".as_ptr(), std::ptr::null_mut(), 0) },
        0
    );

    let mut buf = [0 as c_char; 2048];
    assert!(unsafe { ffi::arcane_sdk_friends_json(buf.as_mut_ptr(), buf.len()) } > 0);

    let json: Vec<u8> = buf
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect();
    let rendered = String::from_utf8(json).expect("the C ABI writes utf-8");

    assert!(
        rendered.starts_with("{\"friends\":[{\"user_id\":\"user-a\",\"pseudo\":"),
        "field order drifted from the header and the docs: {rendered}"
    );

    let parsed: serde_json::Value =
        serde_json::from_str(&rendered).expect("the C ABI writes valid json");
    assert_eq!(parsed["friends"][0]["pseudo"], AWKWARD_PSEUDO);
    assert_eq!(parsed["friends"][0]["in_game"], true);

    ffi::arcane_sdk_shutdown();
}

const NO_EVENTS: Reply = Reply {
    status: "200 OK",
    body: r#"{"events":[],"cursor":null}"#,
};

const NO_LAUNCH_CODE: Reply = Reply {
    status: "200 OK",
    body: r#"{"join_code":null}"#,
};

/// `udp://10.0.0.1:7777`, base64 — what the stub hands back as a payload.
const HOST_PAYLOAD_B64: &str = "dWRwOi8vMTAuMC4wLjE6Nzc3Nw==";
const HOST_PAYLOAD: &[u8] = b"udp://10.0.0.1:7777";

const LOBBY: Reply = Reply {
    status: "200 OK",
    body: r#"{"lobby_id":"lobby-1","join_code":"K7P3QX","host_user_id":"user-host",
              "host_payload":"dWRwOi8vMTAuMC4wLjE6Nzc3Nw==","visibility":"friends_and_code",
              "max_players":4,"expires_at":"2026-05-01T12:34:56Z",
              "members":[{"user_id":"user-host","pseudo":"Ada",
                          "payload":"dWRwOi8vMTAuMC4wLjE6Nzc3Nw=="},
                         {"user_id":"user-b","pseudo":"Bo","payload":null}]}"#,
};

/// Serve every lobby route with `lobby`, and the polled routes with an empty
/// answer so the session thread never disarms itself mid-test.
fn lobby_stub(lobby: Reply) -> Stub {
    Stub::start_with(move |request| {
        if request.line.contains("/v1/health") {
            HEALTHY
        } else if request.line.contains("/lobbies/events") {
            NO_EVENTS
        } else if request.line.contains("/launch-context") {
            NO_LAUNCH_CODE
        } else if request.line.contains("/lobbies") {
            lobby
        } else {
            DRM_OFF
        }
    })
}

#[test]
fn creating_a_lobby_returns_the_object_with_its_join_code() {
    let stub = lobby_stub(LOBBY);
    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");

    let lobby = client
        .p2p()
        .create_lobby(4, Visibility::FriendsAndCode, HOST_PAYLOAD)
        .expect("create");

    assert_eq!(lobby.lobby_id, "lobby-1");
    assert_eq!(lobby.join_code.as_deref(), Some("K7P3QX"));
    assert_eq!(lobby.host_user_id, "user-host");
    assert_eq!(lobby.host_payload, HOST_PAYLOAD);
    assert_eq!(lobby.max_players, 4);
    assert_eq!(lobby.members.len(), 2);
    assert_eq!(lobby.members[0].pseudo, "Ada");
    assert_eq!(lobby.members[0].payload, HOST_PAYLOAD);
    assert!(lobby.members[1].payload.is_empty());

    let created = stub.matching("/lobbies ");
    assert_eq!(created.len(), 1);
    assert!(created[0]
        .line
        .starts_with("POST /v1/games/pk_test_title/lobbies "));
    let body = created[0].json();
    assert_eq!(body["max_players"], 4);
    assert_eq!(body["visibility"], "friends_and_code");
    assert_eq!(body["payload"], HOST_PAYLOAD_B64);
}

#[test]
fn joining_by_code_uppercases_it_and_decodes_the_host_payload() {
    let stub = lobby_stub(LOBBY);
    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");

    let lobby = client
        .p2p()
        .join_by_code("k7p3qx", b"udp://10.0.0.2:7777")
        .expect("join by code");

    assert_eq!(lobby.host_payload, HOST_PAYLOAD);
    assert_eq!(lobby.join_code.as_deref(), Some("K7P3QX"));

    let joined = stub.matching("/lobbies/join");
    assert_eq!(joined.len(), 1);
    assert_eq!(joined[0].json()["join_code"], "K7P3QX");
    assert_eq!(joined[0].json()["payload"], "dWRwOi8vMTAuMC4wLjI6Nzc3Nw==");
}

#[test]
fn joining_by_id_inviting_leaving_and_closing_hit_their_routes() {
    let stub = lobby_stub(Reply {
        status: "200 OK",
        body: r#"{"lobby_id":"lobby-1","join_code":null,"host_user_id":"user-host",
                  "host_payload":"dWRwOi8vMTAuMC4wLjE6Nzc3Nw==","max_players":4,"members":[]}"#,
    });
    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");

    let lobby = client.p2p().join("lobby-1", b"me").expect("join by id");
    assert_eq!(
        lobby.join_code, None,
        "a friends lobby has no code for a member"
    );

    client.p2p().invite("lobby-1", "user-b").expect("invite");
    client.p2p().leave("lobby-1").expect("leave");
    client.p2p().close("lobby-1").expect("close");

    let joined = stub.matching("/lobbies/lobby-1/join");
    assert_eq!(joined.len(), 1);
    assert!(joined[0].line.starts_with("POST "));
    assert_eq!(joined[0].json()["payload"], "bWU=");

    let invited = stub.matching("/lobbies/lobby-1/invite");
    assert_eq!(invited.len(), 1);
    assert_eq!(invited[0].json()["to_user_id"], "user-b");

    assert_eq!(stub.matching("/lobbies/lobby-1/leave").len(), 1);

    let closed = stub.matching("DELETE /v1/games/pk_test_title/lobbies/lobby-1 ");
    assert_eq!(closed.len(), 1, "close is a DELETE: {:?}", stub.requests());
}

#[test]
fn the_lobby_error_codes_map_from_their_desktop_bodies() {
    for (status, error, code) in [
        ("404 Not Found", "lobby_not_found", "lobby_not_found"),
        ("409 Conflict", "lobby_full", "lobby_full"),
        ("410 Gone", "lobby_closed", "lobby_closed"),
        ("403 Forbidden", "not_friends", "not_friends"),
    ] {
        let body: &'static str =
            Box::leak(format!(r#"{{"error":"{error}","message":"nope"}}"#).into_boxed_str());
        let _stub = lobby_stub(Reply { status, body });

        let err = ArcaneClient::init(PUBLIC_KEY)
            .expect("init")
            .p2p()
            .join_by_code("K7P3QX", b"me")
            .expect_err("the desktop refused");

        assert_eq!(err.code(), code);
        assert!(!err.is_retryable());
        assert!(err.hint().is_some());
        assert!(err
            .context()
            .iter()
            .any(|(k, v)| k == "join_code" && v == "K7P3QX"));
    }
}

#[test]
fn a_desktop_without_the_lobby_routes_degrades_to_feature_unavailable() {
    let _stub = lobby_stub(Reply {
        status: "404 Not Found",
        body: "Not Found",
    });

    let err = ArcaneClient::init(PUBLIC_KEY)
        .expect("init")
        .p2p()
        .create_lobby(4, Visibility::Code, b"me")
        .expect_err("no route");

    assert_eq!(err.code(), "feature_unavailable");
    assert!(err.hint().expect("hint").contains("Update"));
}

#[test]
fn a_malformed_join_code_or_an_oversized_payload_fails_before_any_request() {
    let stub = lobby_stub(LOBBY);
    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");

    for code in [
        "",
        "K7P3Q",
        "K7P3Q0",
        "K7P3QIO",
        "https://arcane.gg/join/K7P3QX",
    ] {
        let err = client
            .p2p()
            .join_by_code(code, b"me")
            .expect_err("accepted a bad code");
        assert_eq!(err.code(), "invalid_argument", "accepted {code:?}");
    }

    let too_big = vec![7u8; 4097];
    let err = client
        .p2p()
        .create_lobby(4, Visibility::Code, &too_big)
        .expect_err("accepted an oversized payload");
    assert_eq!(err.code(), "invalid_argument");

    let err = client
        .p2p()
        .invite("lobby/../evil", "user-b")
        .expect_err("accepted a bad id");
    assert_eq!(err.code(), "invalid_argument");

    assert!(
        stub.matching("/lobbies").is_empty(),
        "a rejected argument must never leave the process: {:?}",
        stub.requests()
    );
}

#[test]
fn offline_only_mode_refuses_the_lobby_calls_without_a_request() {
    let stub = lobby_stub(LOBBY);
    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");

    std::env::set_var("ARCANE_OFFLINE_ONLY", "1");

    let err = client
        .p2p()
        .create_lobby(4, Visibility::Code, b"me")
        .expect_err("offline only");
    assert_eq!(err.code(), "network_required");
    assert!(err
        .context()
        .iter()
        .any(|(k, v)| k == "env" && v == "ARCANE_OFFLINE_ONLY"));

    for code in [
        client.p2p().join_by_code("K7P3QX", b"me").err(),
        client.p2p().join("lobby-1", b"me").err(),
        client.p2p().invite("lobby-1", "user-b").err(),
        client.p2p().leave("lobby-1").err(),
        client.p2p().close("lobby-1").err(),
    ] {
        assert_eq!(code.expect("offline only").code(), "network_required");
    }
    assert_eq!(client.p2p().launch_join_code(), None);

    assert!(
        stub.matching("/lobbies").is_empty() && stub.matching("/launch-context").is_empty(),
        "offline-only mode must not reach the desktop: {:?}",
        stub.requests()
    );

    std::env::remove_var("ARCANE_OFFLINE_ONLY");
}

#[test]
fn the_launch_join_code_is_read_once_and_then_cached() {
    let stub = Stub::start_with(|request| {
        if request.line.contains("/v1/health") {
            HEALTHY
        } else if request.line.contains("/lobbies/events") {
            NO_EVENTS
        } else if request.line.contains("/launch-context") {
            Reply {
                status: "200 OK",
                body: r#"{"join_code":"k7p3qx"}"#,
            }
        } else {
            DRM_OFF
        }
    });
    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");

    assert_eq!(
        client.p2p().launch_join_code().as_deref(),
        Some("K7P3QX"),
        "the launch code is normalised like any other"
    );
    assert_eq!(client.p2p().launch_join_code().as_deref(), Some("K7P3QX"));
    assert_eq!(
        client.clone().p2p().launch_join_code().as_deref(),
        Some("K7P3QX")
    );

    let read = stub.matching("/launch-context");
    assert_eq!(
        read.len(),
        1,
        "the desktop clears it once served, so the SDK must read it once: {:?}",
        stub.requests()
    );
    assert!(read[0]
        .line
        .starts_with("GET /v1/games/pk_test_title/launch-context"));
}

#[test]
fn no_launch_context_is_not_a_failure() {
    let _stub = lobby_stub(LOBBY);
    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");

    assert_eq!(client.p2p().launch_join_code(), None);
}

#[test]
fn lobby_events_are_polled_with_a_cursor_and_delivered_once() {
    let stub = Stub::start_with(|request| {
        if request.line.contains("/v1/health") {
            HEALTHY
        } else if request.line.contains("after=c-1") {
            Reply {
                status: "200 OK",
                body: r#"{"events":[
                    {"id":"3","type":"member_left","lobby_id":"lobby-1","user_id":"user-b"},
                    {"id":"4","type":"lobby_closed","lobby_id":"lobby-1"}
                ],"cursor":"c-2"}"#,
            }
        } else if request.line.contains("after=c-2") {
            Reply {
                status: "200 OK",
                body: r#"{"events":[],"cursor":"c-2"}"#,
            }
        } else if request.line.contains("/lobbies/events") {
            Reply {
                status: "200 OK",
                body: r#"{"events":[
                    {"id":"1","type":"invite","lobby_id":"lobby-1","join_code":"K7P3QX",
                     "from_user_id":"user-a","pseudo":"Ada"},
                    {"id":"2","type":"member_joined","lobby_id":"lobby-1","user_id":"user-b",
                     "pseudo":"Bo","payload":"dWRwOi8vMTAuMC4wLjI6Nzc3Nw=="}
                ],"cursor":"c-1"}"#,
            }
        } else {
            DRM_OFF
        }
    });
    std::env::set_var("ARCANE_SESSION_TICK_MS", "150");

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");
    assert!(
        client.p2p().poll_events().is_empty(),
        "nothing has been polled yet"
    );

    let collected = drain_events(&client, 4);
    stub.wait_for("after=c-2", 1);

    assert_eq!(
        collected,
        vec![
            LobbyEvent::Invite {
                lobby_id: "lobby-1".into(),
                join_code: Some("K7P3QX".into()),
                from_user_id: "user-a".into(),
                pseudo: "Ada".into(),
            },
            LobbyEvent::MemberJoined {
                lobby_id: "lobby-1".into(),
                user_id: "user-b".into(),
                pseudo: "Bo".into(),
                payload: b"udp://10.0.0.2:7777".to_vec(),
            },
            LobbyEvent::MemberLeft {
                lobby_id: "lobby-1".into(),
                user_id: "user-b".into(),
            },
            LobbyEvent::LobbyClosed {
                lobby_id: "lobby-1".into(),
            },
        ]
    );

    let first = stub.matching("/lobbies/events");
    assert!(
        first[0].line.contains("/lobbies/events HTTP"),
        "the first poll carries no cursor: {}",
        first[0].line
    );
    assert_eq!(
        client.session().lobby_events,
        arcane_sdk::LobbyPollingState::Active
    );

    thread::sleep(Duration::from_millis(400));
    assert!(
        client.p2p().poll_events().is_empty(),
        "every event is delivered exactly once"
    );
}

/// Drain `poll_events` until `count` events have arrived, or give up.
fn drain_events(client: &ArcaneClient, count: usize) -> Vec<LobbyEvent> {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    let mut collected = Vec::new();
    while collected.len() < count {
        collected.extend(client.p2p().poll_events());
        assert!(
            Instant::now() < deadline,
            "only {} of {count} events arrived: {collected:?}",
            collected.len()
        );
        thread::sleep(Duration::from_millis(20));
    }
    collected
}

#[test]
fn a_desktop_without_the_events_route_stops_polling_silently() {
    let stub = Stub::start_with(|request| {
        if request.line.contains("/v1/health") {
            HEALTHY
        } else if request.line.contains("/lobbies/events") {
            Reply {
                status: "404 Not Found",
                body: "Not Found",
            }
        } else {
            DRM_OFF
        }
    });
    std::env::set_var("ARCANE_SESSION_TICK_MS", "150");

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");
    assert_eq!(
        client.session().lobby_events,
        arcane_sdk::LobbyPollingState::Off
    );

    client.p2p().poll_events();
    stub.wait_for("/lobbies/events", 1);

    let deadline = Instant::now() + WAIT_TIMEOUT;
    while client.session().lobby_events != arcane_sdk::LobbyPollingState::Unavailable {
        assert!(
            Instant::now() < deadline,
            "polling never reported itself off"
        );
        thread::sleep(Duration::from_millis(20));
    }

    let polled = stub.matching("/lobbies/events").len();
    thread::sleep(Duration::from_millis(600));
    assert_eq!(
        stub.matching("/lobbies/events").len(),
        polled,
        "a bare 404 retires polling for good: {:?}",
        stub.requests()
    );
    assert!(
        client.p2p().poll_events().is_empty(),
        "and a later p2p() call does not restart it"
    );
    assert_eq!(
        client.session().lobby_events,
        arcane_sdk::LobbyPollingState::Unavailable
    );
}

#[test]
fn arming_wakes_the_session_thread_rather_than_waiting_out_its_tick() {
    let stub = lobby_stub(LOBBY);

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");
    assert!(
        stub.matching("/lobbies/events").is_empty(),
        "a game that never calls p2p() never polls: {:?}",
        stub.requests()
    );

    // The default tick is 60 s and the thread is already asleep on it.
    client.p2p().poll_events();
    stub.wait_for("/lobbies/events", 1);
}

#[test]
fn the_c_abi_writes_a_lobby_as_json() {
    let _stub = lobby_stub(LOBBY);

    assert_eq!(
        unsafe { ffi::arcane_sdk_init(c"pk_test_title".as_ptr(), std::ptr::null_mut(), 0) },
        0
    );

    let mut buf = [0 as c_char; 4096];
    let written = unsafe {
        ffi::arcane_sdk_lobby_create(
            4,
            ffi::ARCANE_LOBBY_FRIENDS_AND_CODE,
            c"dWRwOi8vMTAuMC4wLjE6Nzc3Nw==".as_ptr(),
            buf.as_mut_ptr(),
            buf.len(),
        )
    };
    assert!(written > 0);

    let rendered = read_c_string(&buf);
    assert!(
        rendered.starts_with(
            r#"{"lobby_id":"lobby-1","join_code":"K7P3QX","host_user_id":"user-host""#
        ),
        "field order drifted from the header and the docs: {rendered}"
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&rendered).expect("the C ABI writes valid json");
    assert_eq!(parsed["host_payload"], HOST_PAYLOAD_B64);
    assert_eq!(parsed["max_players"], 4);
    assert_eq!(parsed["members"][0]["pseudo"], "Ada");
    assert_eq!(parsed["members"][1]["payload"], "");

    let mut err = [0 as c_char; 512];
    assert_eq!(
        unsafe {
            ffi::arcane_sdk_lobby_invite(
                c"lobby-1".as_ptr(),
                c"user-b".as_ptr(),
                err.as_mut_ptr(),
                err.len(),
            )
        },
        0
    );

    assert_eq!(
        unsafe {
            ffi::arcane_sdk_lobby_create(4, 7, std::ptr::null(), buf.as_mut_ptr(), buf.len())
        },
        -2,
        "an unknown visibility is a bad argument"
    );
    assert_eq!(
        unsafe {
            ffi::arcane_sdk_lobby_create(
                4,
                ffi::ARCANE_LOBBY_CODE,
                c"not base64!!".as_ptr(),
                buf.as_mut_ptr(),
                buf.len(),
            )
        },
        -2
    );

    ffi::arcane_sdk_shutdown();
}

#[test]
fn the_c_abi_keeps_the_events_queued_when_the_buffer_is_too_small() {
    let _stub = Stub::start_with(|request| {
        if request.line.contains("/v1/health") {
            HEALTHY
        } else if request.line.contains("/lobbies/events") {
            Reply {
                status: "200 OK",
                body: r#"{"events":[{"id":"1","type":"member_joined","lobby_id":"lobby-1",
                          "user_id":"user-b","pseudo":"Bo",
                          "payload":"dWRwOi8vMTAuMC4wLjI6Nzc3Nw=="}],"cursor":"c-1"}"#,
            }
        } else {
            DRM_OFF
        }
    });
    std::env::set_var("ARCANE_SESSION_TICK_MS", "150");

    assert_eq!(
        unsafe { ffi::arcane_sdk_init(c"pk_test_title".as_ptr(), std::ptr::null_mut(), 0) },
        0
    );

    // Exactly the size of an empty queue: `{"events":[]}` plus its NUL.
    let mut tiny = [0 as c_char; 14];
    let empty = unsafe { ffi::arcane_sdk_lobby_events_json(tiny.as_mut_ptr(), tiny.len()) };
    assert_eq!(empty, 13, "the empty queue fits and reads as an empty list");
    assert_eq!(read_c_string(&tiny), r#"{"events":[]}"#);

    let deadline = Instant::now() + WAIT_TIMEOUT;
    while unsafe { ffi::arcane_sdk_lobby_events_json(tiny.as_mut_ptr(), tiny.len()) } != -3 {
        assert!(Instant::now() < deadline, "no event ever arrived");
        thread::sleep(Duration::from_millis(20));
    }

    let mut buf = [0 as c_char; 1024];
    let written = unsafe { ffi::arcane_sdk_lobby_events_json(buf.as_mut_ptr(), buf.len()) };
    assert!(written > 0, "a -3 must not have drained the queue");

    let parsed: serde_json::Value =
        serde_json::from_str(&read_c_string(&buf)).expect("the C ABI writes valid json");
    assert_eq!(parsed["events"][0]["type"], "member_joined");
    assert_eq!(parsed["events"][0]["lobby_id"], "lobby-1");
    assert_eq!(parsed["events"][0]["user_id"], "user-b");
    assert_eq!(
        parsed["events"][0]["payload"],
        "dWRwOi8vMTAuMC4wLjI6Nzc3Nw=="
    );

    let drained = unsafe { ffi::arcane_sdk_lobby_events_json(buf.as_mut_ptr(), buf.len()) };
    assert!(drained > 0);
    assert_eq!(read_c_string(&buf), r#"{"events":[]}"#);

    ffi::arcane_sdk_shutdown();
}

#[test]
fn the_lobby_entry_points_are_safe_before_init() {
    ffi::arcane_sdk_shutdown();
    let mut buf = [0 as c_char; 256];

    assert_eq!(
        unsafe {
            ffi::arcane_sdk_lobby_create(
                4,
                ffi::ARCANE_LOBBY_CODE,
                std::ptr::null(),
                buf.as_mut_ptr(),
                buf.len(),
            )
        },
        -1
    );
    assert_eq!(
        unsafe { ffi::arcane_sdk_lobby_events_json(buf.as_mut_ptr(), buf.len()) },
        -1
    );
    assert_eq!(
        unsafe { ffi::arcane_sdk_launch_join_code(buf.as_mut_ptr(), buf.len()) },
        -1
    );
    assert_eq!(
        unsafe { ffi::arcane_sdk_lobby_leave(c"lobby-1".as_ptr(), buf.as_mut_ptr(), buf.len()) },
        1
    );
    assert_eq!(
        unsafe {
            ffi::arcane_sdk_lobby_invite(
                std::ptr::null(),
                std::ptr::null(),
                buf.as_mut_ptr(),
                buf.len(),
            )
        },
        1
    );
}
