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

const GAME_ID: &str = "7c9e6f21-4b58-4a3d-8e10-5d2f9b0c1a34";
/// A 32-byte account-key SPKI fingerprint, hex — what the desktop publishes.
const ACCOUNT_KEY_FPR: &str = "8f14e45fea8f4b4a9c0a1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f60718293a4b5c6";
/// The fingerprint of a key this account has since rotated away from.
const ROTATED_KEY_FPR: &str = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";

/// `ARCANE_DRM_ROOT` and the launch ids are process-global, so fixtures must
/// not overlap.
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
        std::env::set_var("ARCANE_GAME_ID", GAME_ID);
        std::env::remove_var("ARCANE_USER_ID");

        Self {
            _dir: dir,
            root,
            _guard: guard,
        }
    }

    /// The hash the SDK must derive from `ACCOUNT_KEY_FPR` — computed here the
    /// long way round, so a change to the SDK's derivation fails the test rather
    /// than silently agreeing with itself.
    fn device_hash(&self) -> String {
        hash_fpr(ACCOUNT_KEY_FPR)
    }

    /// Publish fingerprint pre-images for `user_id`, as the desktop app does.
    fn write_device(&self, user_id: &str, fprs: &[&str]) {
        let entries: Vec<String> = fprs
            .iter()
            .map(|fpr| format!(r#"{{"kind":"account_key_fpr","value":"{fpr}"}}"#))
            .collect();
        write_file(
            &self.root.join("device").join(format!("{user_id}.json")),
            &format!(
                r#"{{"user_id":"{user_id}","updated_at":1,"fingerprints":[{}]}}"#,
                entries.join(",")
            ),
        );
    }

    fn launch_as(&self, user_id: &str) {
        std::env::set_var("ARCANE_USER_ID", user_id);
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
            &self.root.join("flags").join(format!("{GAME_ID}.json")),
            &format!(r#"{{"drm_enabled":{drm_enabled}}}"#),
        );
    }

    /// Writes the account's fingerprint file too: a ticket on disk without one
    /// is the "desktop too old" case, and tests that want it say so explicitly.
    fn write_ticket(&self, user_id: &str, ticket: &Ticket) {
        self.write_device(user_id, &[ACCOUNT_KEY_FPR]);
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
                .join(format!("{GAME_ID}.ticket")),
            &body,
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::env::remove_var("ARCANE_DRM_ROOT");
        std::env::remove_var("ARCANE_OFFLINE_ONLY");
        std::env::remove_var("ARCANE_GAME_ID");
        std::env::remove_var("ARCANE_USER_ID");
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

fn hash_fpr(fpr_hex: &str) -> String {
    let bytes = hex::decode(fpr_hex).expect("fingerprint is hex");
    assert_eq!(bytes.len(), 32, "an SPKI fingerprint is 32 bytes");
    hex::encode(&Sha256::digest(bytes)[..16])
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

    let client = ArcaneClient::init().expect("init");

    assert_eq!(client.ownership(), OwnershipStatus::DrmDisabled);
    assert_eq!(client.user_id(), Some("user-a"));
    assert_eq!(
        client.game_id(),
        GAME_ID,
        "game_id is the value passed to init, not the one in the ticket file"
    );
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

    let err = ArcaneClient::init().expect_err("must not accept user-a's ticket");

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

    let err = ArcaneClient::init().expect_err("nobody is signed in");

    assert_eq!(err.code(), "not_authenticated");
    assert!(err.is_retryable());
    assert!(err.hint().expect("hint").contains("Sign in"));
}

#[test]
fn a_single_ticket_is_used_when_no_session_is_recorded() {
    let fixture = Fixture::new();
    fixture.write_ticket("user-a", &Ticket::default());

    let client = ArcaneClient::init().expect("init");

    assert_eq!(client.ownership(), OwnershipStatus::DrmDisabled);
    assert_eq!(client.user_id(), Some("user-a"));
}

#[test]
fn several_tickets_without_a_session_are_ambiguous_rather_than_guessed() {
    let fixture = Fixture::new();
    fixture.write_ticket("user-a", &Ticket::default());
    fixture.write_ticket("user-b", &Ticket::default());

    let err = ArcaneClient::init().expect_err("must refuse to guess");

    assert_eq!(err.code(), "ambiguous_session");
    assert_eq!(context_of(&err, "candidates").as_deref(), Some("2"));
    assert!(err.hint().expect("hint").contains("session.json"));
}

/// The unsigned `device_hash` field of the ticket file is not a verdict: only
/// the signed `dev` claim is. A file that disagrees with it must not fail the
/// check on its own — see `a_signed_ticket_bound_to_the_published_fingerprint_
/// resolves_as_owned` in `owned_ticket.rs` for the claim being enforced.
#[test]
fn the_unsigned_cached_device_hash_is_not_what_decides() {
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

    let err = ArcaneClient::init().expect_err("the JWT is not readable");

    // The unreadable ticket is what is reported, not a mismatch on a field that
    // nothing signed.
    assert_eq!(err.code(), "ticket_invalid");
    assert!(context_of(&err, "path").is_some());
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

    let err = ArcaneClient::init().expect_err("empty ticket");

    assert_eq!(err.code(), "ticket_missing");
    assert!(context_of(&err, "path").is_some());
}

#[test]
fn the_drm_flag_short_circuits_before_any_ticket_lookup() {
    let fixture = Fixture::new();
    fixture.write_flag(false);
    fixture.write_session(Some("user-a"));
    // No tickets on disk at all.

    let client = ArcaneClient::init().expect("init");

    assert_eq!(client.ownership(), OwnershipStatus::DrmDisabled);
    assert!(!client.is_owned());
    assert_eq!(client.user_id(), Some("user-a"));
    assert_eq!(client.game_id(), GAME_ID);
    assert_eq!(client.ticket_expires_at(), None);
}

#[test]
fn a_missing_tickets_directory_names_the_path_it_looked_at() {
    let fixture = Fixture::new();
    let _ = &fixture;

    let err = ArcaneClient::init().expect_err("no tickets");

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
            .join(format!("{GAME_ID}.ticket")),
        "{ this is not json",
    );

    let err = ArcaneClient::init().expect_err("corrupt ticket");

    assert_eq!(err.code(), "ticket_invalid");
    assert!(context_of(&err, "path").is_some());
}

#[test]
fn an_invalid_game_id_fails_before_touching_the_filesystem() {
    let fixture = Fixture::new();
    let _ = &fixture;
    std::env::set_var("ARCANE_GAME_ID", "game/../../etc/passwd");

    let err = ArcaneClient::init().expect_err("invalid game id");

    assert_eq!(err.code(), "invalid_game_id");
    assert!(err.hint().is_some());
}

#[test]
fn a_missing_game_id_env_is_reported_before_any_lookup() {
    let fixture = Fixture::new();
    fixture.write_session(Some("user-a"));
    fixture.write_ticket("user-a", &Ticket::default());
    std::env::remove_var("ARCANE_GAME_ID");

    let err = ArcaneClient::init().expect_err("nothing named a title");

    assert_eq!(err.code(), "missing_game_id");
    assert!(!err.is_retryable());
    assert_eq!(context_of(&err, "env").as_deref(), Some("ARCANE_GAME_ID"));
}

/// The launcher knows which account it started the game for, so its answer wins
/// over whatever `session.json` last recorded.
#[test]
fn the_launch_user_id_picks_that_accounts_ticket_over_the_session_file() {
    let fixture = Fixture::new();
    fixture.write_session(Some("user-a"));
    fixture.write_ticket("user-a", &Ticket::default());
    fixture.write_ticket("user-b", &Ticket::default());
    fixture.launch_as("user-b");

    let client = ArcaneClient::init().expect("init");

    assert_eq!(client.ownership(), OwnershipStatus::DrmDisabled);
    assert_eq!(client.user_id(), Some("user-b"));
}

#[test]
fn the_launch_user_id_never_falls_back_to_another_accounts_ticket() {
    let fixture = Fixture::new();
    fixture.write_session(Some("user-a"));
    fixture.write_ticket("user-a", &Ticket::default());
    fixture.launch_as("user-b");

    let err = ArcaneClient::init().expect_err("must not accept user-a's ticket");

    assert_eq!(err.code(), "ticket_missing");
    assert_eq!(context_of(&err, "user_id").as_deref(), Some("user-b"));
    assert_eq!(context_of(&err, "source").as_deref(), Some("ARCANE_USER_ID"));
    assert!(context_of(&err, "expected_path")
        .expect("expected_path in context")
        .contains("user-b"));
}

#[test]
fn the_launch_user_id_names_the_account_when_drm_is_disabled() {
    let fixture = Fixture::new();
    fixture.write_flag(false);
    fixture.write_session(Some("user-a"));
    fixture.launch_as("user-b");

    let client = ArcaneClient::init().expect("init");

    assert_eq!(client.ownership(), OwnershipStatus::DrmDisabled);
    assert_eq!(client.user_id(), Some("user-b"));
}

#[test]
fn a_malformed_launch_user_id_is_ignored_rather_than_reported() {
    for bad in ["../user-b", "user b", "", &"b".repeat(257)] {
        let fixture = Fixture::new();
        fixture.write_session(Some("user-a"));
        fixture.write_ticket("user-a", &Ticket::default());
        fixture.launch_as(bad);

        let client = ArcaneClient::init().expect("init falls back to session.json");

        assert_eq!(
            client.user_id(),
            Some("user-a"),
            "a hint the SDK cannot use is ignored, not an error: {bad:?}"
        );
    }
}

#[test]
fn refresh_is_refused_in_offline_only_mode() {
    let fixture = Fixture::new();
    fixture.write_session(Some("user-a"));
    fixture.write_ticket("user-a", &Ticket::default());

    let mut client = ArcaneClient::init().expect("init");
    let err = client.refresh().expect_err("offline-only blocks refresh");

    assert_eq!(err.code(), "network_required");
    assert!(err.hint().expect("hint").contains("ARCANE_OFFLINE_ONLY"));
    // State is untouched on failure.
    assert_eq!(client.ownership(), OwnershipStatus::DrmDisabled);
}

/// The failure this change fixes: the desktop app never published a fingerprint
/// for this account, so nothing local can be compared to the ticket's `dev`.
#[test]
fn a_missing_fingerprint_file_names_the_account_and_the_path() {
    let fixture = Fixture::new();
    fixture.write_session(Some("user-a"));
    fixture.write_ticket(
        "user-a",
        &Ticket {
            jwt: "not.a.real.jwt".into(),
            drm_enabled: true,
            ..Ticket::default()
        },
    );
    fs::remove_file(fixture.root.join("device").join("user-a.json")).expect("remove device file");

    let err = ArcaneClient::init().expect_err("no fingerprint to compare against");

    assert_eq!(err.code(), "device_mismatch");
    assert_eq!(context_of(&err, "user_id").as_deref(), Some("user-a"));
    assert!(context_of(&err, "path")
        .expect("path in context")
        .contains("user-a.json"));
    assert!(err.hint().expect("hint").contains("Arcane desktop"));
}

/// A ticket minted before the account rotated its key still verifies offline,
/// because the desktop keeps publishing the fingerprint it was bound to.
#[test]
fn a_ticket_bound_to_a_rotated_key_still_resolves() {
    let fixture = Fixture::new();
    fixture.write_session(Some("user-a"));
    fixture.write_ticket(
        "user-a",
        &Ticket {
            device_hash: Some(hash_fpr(ROTATED_KEY_FPR)),
            ..Ticket::default()
        },
    );
    fixture.write_device("user-a", &[ACCOUNT_KEY_FPR, ROTATED_KEY_FPR]);
    fixture.write_flag(true);

    // DRM is off in this ticket, so the JWT is not exercised — what is under test
    // is that the cached `device_hash` is matched against the whole set.
    let client = ArcaneClient::init().expect("init");

    assert_eq!(client.ownership(), OwnershipStatus::DrmDisabled);
}

/// A fingerprint file listing only kinds this SDK cannot rehash must fail
/// loudly, not silently accept an empty candidate set.
#[test]
fn an_unrecognised_fingerprint_kind_is_not_treated_as_a_match() {
    let fixture = Fixture::new();
    fixture.write_session(Some("user-a"));
    fixture.write_ticket(
        "user-a",
        &Ticket {
            jwt: "not.a.real.jwt".into(),
            drm_enabled: true,
            ..Ticket::default()
        },
    );
    write_file(
        &fixture.root.join("device").join("user-a.json"),
        r#"{"user_id":"user-a","fingerprints":[{"kind":"tpm_attestation","value":"ab"}]}"#,
    );

    let err = ArcaneClient::init().expect_err("nothing verifiable on this machine");

    assert_eq!(err.code(), "device_mismatch");
    assert_eq!(context_of(&err, "entries").as_deref(), Some("1"));
}
