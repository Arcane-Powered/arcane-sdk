//! Which ticket the client picks, and what it reports when it cannot pick one.
//!
//! These run with `ARCANE_OFFLINE_ONLY=1` so a missing ticket surfaces directly
//! instead of trying to launch the Arcane desktop app.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use arcane_sdk::{ArcaneClient, OwnershipStatus};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const PUBLIC_KEY: &str = "pk_test_title";
const MACHINE_ID: &str = "8f14e45f-ea8f-4b4a-9c0a-1d2e3f4a5b6c";

/// `ARCANE_DRM_ROOT` is process-global, so fixtures must not overlap.
static ENV_LOCK: Mutex<()> = Mutex::new(());

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    _guard: MutexGuard<'static, ()>,
}

impl Fixture {
    fn new() -> Self {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = TempDir::new().expect("temp dir");
        let root = dir.path().to_path_buf();

        std::env::set_var("ARCANE_DRM_ROOT", &root);
        std::env::set_var("ARCANE_OFFLINE_ONLY", "1");

        // Pin the machine id so ticket device hashes are predictable.
        fs::write(root.join("machine_id"), MACHINE_ID).expect("write machine_id");

        Self {
            _dir: dir,
            root,
            _guard: guard,
        }
    }

    fn device_hash(&self) -> String {
        let digest = Sha256::digest(MACHINE_ID.as_bytes());
        hex::encode(&digest[..16])
    }

    fn write_session(&self, user_id: Option<&str>) {
        let body = match user_id {
            Some(id) => format!(r#"{{"user_id":"{id}","updated_at":1}}"#),
            None => r#"{"user_id":null,"updated_at":1}"#.to_string(),
        };
        write_file(&self.root.join("session.json"), &body);
    }

    fn write_flag(&self, drm_enabled: bool) {
        write_file(
            &self.root.join("flags").join(format!("{PUBLIC_KEY}.json")),
            &format!(r#"{{"drm_enabled":{drm_enabled}}}"#),
        );
    }

    fn write_ticket(&self, user_id: &str, ticket: &Ticket) {
        let device_hash = ticket
            .device_hash
            .clone()
            .unwrap_or_else(|| self.device_hash());
        let body = format!(
            r#"{{
                "ticket": "{jwt}",
                "cached_at": "2026-01-01T00:00:00Z",
                "expires_at": "2027-01-01T00:00:00Z",
                "game_id": "{game_id}",
                "user_id": "{user_id}",
                "device_hash": "{device_hash}",
                "drm_enabled": {drm_enabled}
            }}"#,
            jwt = ticket.jwt,
            game_id = ticket.game_id,
            drm_enabled = ticket.drm_enabled,
        );
        write_file(
            &self
                .root
                .join("tickets")
                .join(user_id)
                .join(format!("{PUBLIC_KEY}.ticket")),
            &body,
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::env::remove_var("ARCANE_DRM_ROOT");
        std::env::remove_var("ARCANE_OFFLINE_ONLY");
    }
}

struct Ticket {
    jwt: String,
    game_id: String,
    drm_enabled: bool,
    device_hash: Option<String>,
}

impl Default for Ticket {
    /// A DRM-off ticket: a complete success path that needs no signed JWT.
    fn default() -> Self {
        Self {
            jwt: String::new(),
            game_id: "game-canonical-id".into(),
            drm_enabled: false,
            device_hash: None,
        }
    }
}

fn write_file(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
    fs::write(path, body).expect("write file");
}

fn context_of(err: &arcane_sdk::SdkError, key: &str) -> Option<String> {
    err.context()
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

#[test]
fn uses_the_ticket_of_the_signed_in_account() {
    let fixture = Fixture::new();
    fixture.write_session(Some("user-a"));
    fixture.write_ticket("user-a", &Ticket::default());

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");

    assert_eq!(client.ownership(), OwnershipStatus::DrmDisabled);
    assert_eq!(client.user_id(), Some("user-a"));
    assert_eq!(client.game_id(), Some("game-canonical-id"));
    assert_eq!(client.public_key(), PUBLIC_KEY);
    assert_eq!(client.device_hash(), fixture.device_hash());
    assert!(client.checked_at() > 0);
}

/// The regression this whole change exists for: user A buys the title and signs
/// out, user B signs in on the same machine. A's ticket must not satisfy B.
#[test]
fn another_accounts_ticket_never_satisfies_the_signed_in_account() {
    let fixture = Fixture::new();
    fixture.write_session(Some("user-b"));
    fixture.write_ticket("user-a", &Ticket::default());

    let err = ArcaneClient::init(PUBLIC_KEY).expect_err("must not accept user-a's ticket");

    assert_eq!(err.code(), "ticket_missing");
    assert_eq!(context_of(&err, "user_id").as_deref(), Some("user-b"));
    assert!(context_of(&err, "expected_path")
        .expect("expected_path in context")
        .contains("user-b"));
}

#[test]
fn a_signed_out_session_is_reported_as_not_authenticated() {
    let fixture = Fixture::new();
    fixture.write_session(None);
    fixture.write_ticket("user-a", &Ticket::default());

    let err = ArcaneClient::init(PUBLIC_KEY).expect_err("nobody is signed in");

    assert_eq!(err.code(), "not_authenticated");
    assert!(err.is_retryable());
    assert!(err.hint().expect("hint").contains("Sign in"));
}

#[test]
fn a_single_ticket_is_used_when_no_session_is_recorded() {
    let fixture = Fixture::new();
    fixture.write_ticket("user-a", &Ticket::default());

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");

    assert_eq!(client.ownership(), OwnershipStatus::DrmDisabled);
    assert_eq!(client.user_id(), Some("user-a"));
}

#[test]
fn several_tickets_without_a_session_are_ambiguous_rather_than_guessed() {
    let fixture = Fixture::new();
    fixture.write_ticket("user-a", &Ticket::default());
    fixture.write_ticket("user-b", &Ticket::default());

    let err = ArcaneClient::init(PUBLIC_KEY).expect_err("must refuse to guess");

    assert_eq!(err.code(), "ambiguous_session");
    assert_eq!(context_of(&err, "candidates").as_deref(), Some("2"));
    assert!(err.hint().expect("hint").contains("session.json"));
}

#[test]
fn a_ticket_bound_to_another_machine_is_a_device_mismatch() {
    let fixture = Fixture::new();
    fixture.write_session(Some("user-a"));
    fixture.write_ticket(
        "user-a",
        &Ticket {
            jwt: "not.a.real.jwt".into(),
            drm_enabled: true,
            device_hash: Some("00112233445566778899aabbccddeeff".into()),
            ..Ticket::default()
        },
    );

    let err = ArcaneClient::init(PUBLIC_KEY).expect_err("device mismatch");

    assert_eq!(err.code(), "device_mismatch");
    assert!(!err.is_retryable());
    // Both fingerprints are named so the developer can see which is which.
    assert!(context_of(&err, "this_device").is_some());
    assert_eq!(
        context_of(&err, "cached_device").as_deref(),
        Some("001122334455…")
    );
}

#[test]
fn an_empty_ticket_with_drm_on_is_reported_as_missing() {
    let fixture = Fixture::new();
    fixture.write_session(Some("user-a"));
    fixture.write_ticket(
        "user-a",
        &Ticket {
            jwt: "   ".into(),
            drm_enabled: true,
            ..Ticket::default()
        },
    );

    let err = ArcaneClient::init(PUBLIC_KEY).expect_err("empty ticket");

    assert_eq!(err.code(), "ticket_missing");
    assert!(context_of(&err, "path").is_some());
}

#[test]
fn the_drm_flag_short_circuits_before_any_ticket_lookup() {
    let fixture = Fixture::new();
    fixture.write_flag(false);
    fixture.write_session(Some("user-a"));
    // No tickets on disk at all.

    let client = ArcaneClient::init(PUBLIC_KEY).expect("init");

    assert_eq!(client.ownership(), OwnershipStatus::DrmDisabled);
    assert!(!client.is_owned());
    assert_eq!(client.user_id(), Some("user-a"));
    assert_eq!(client.ticket_expires_at(), None);
}

#[test]
fn a_missing_tickets_directory_names_the_path_it_looked_at() {
    let fixture = Fixture::new();
    let _ = &fixture;

    let err = ArcaneClient::init(PUBLIC_KEY).expect_err("no tickets");

    assert_eq!(err.code(), "ticket_missing");
    assert!(context_of(&err, "tickets_root").is_some());
}

#[test]
fn a_corrupt_ticket_file_is_invalid_not_missing() {
    let fixture = Fixture::new();
    fixture.write_session(Some("user-a"));
    write_file(
        &fixture
            .root
            .join("tickets")
            .join("user-a")
            .join(format!("{PUBLIC_KEY}.ticket")),
        "{ this is not json",
    );

    let err = ArcaneClient::init(PUBLIC_KEY).expect_err("corrupt ticket");

    assert_eq!(err.code(), "ticket_invalid");
    assert!(context_of(&err, "path").is_some());
}

#[test]
fn an_invalid_public_key_fails_before_touching_the_filesystem() {
    // No fixture: nothing should be read, so no DRM root is needed.
    let err = ArcaneClient::init("pk_abc/../../etc/passwd").expect_err("invalid key");

    assert_eq!(err.code(), "invalid_public_key");
    assert!(err.hint().is_some());
}

#[test]
fn refresh_is_refused_in_offline_only_mode() {
    let fixture = Fixture::new();
    fixture.write_session(Some("user-a"));
    fixture.write_ticket("user-a", &Ticket::default());

    let mut client = ArcaneClient::init(PUBLIC_KEY).expect("init");
    let err = client.refresh().expect_err("offline-only blocks refresh");

    assert_eq!(err.code(), "network_required");
    assert!(err.hint().expect("hint").contains("ARCANE_OFFLINE_ONLY"));
    // State is untouched on failure.
    assert_eq!(client.ownership(), OwnershipStatus::DrmDisabled);
}
