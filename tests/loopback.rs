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
use arcane_sdk::{ArcaneClient, OwnershipStatus, TrackingState};
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
