//! The happy path, end to end: a real ES256 ticket, verified offline against a
//! fingerprint the SDK derives itself.
//!
//! This is the path a launched game takes, and the one that broke when the SDK
//! and the desktop app derived `device_hash` from different things — the SDK
//! from a random local UUID, the desktop from the account key the backend signs
//! into `dev`. Nothing here asserts against a value the SDK computed: the
//! expected hash is derived independently, the way the desktop does it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use arcane_sdk::{ArcaneClient, OwnershipStatus};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const GAME_ID: &str = "28b53780-503f-4a27-a04e-19214d8e9f6b";
const USER_ID: &str = "5015f926-d8cf-4085-ac59-77ef625f8fdb";
const KID: &str = "test-signing-key";

/// A throwaway P-256 key pair, fixed so the fixture is deterministic. It signs
/// nothing outside this test.
const PRIVATE_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgSLMeeFfBzO3gTkgm
MRRokxGE4otJKfAV1KkC/+VN0UChRANCAATBUnxtU5F3q1nKibTMC7FzNXJ/vi2n
k/rKqhYBgk+LxyHx9OlwZnXGNs+Ew+eGZUatvFi0TdCSkcb6an0Z5ZPD
-----END PRIVATE KEY-----";
const PUBLIC_X: &str = "wVJ8bVORd6tZyom0zAuxczVyf74tp5P6yqoWAYJPi8c";
const PUBLIC_Y: &str = "IfH06XBmdcY2z4TD54ZlRq28WLRN0JKRxvpqfRnlk8M";

/// The account key's SPKI fingerprint, as the desktop publishes it.
const ACCOUNT_KEY_FPR: &str = "8f14e45fea8f4b4a9c0a1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f60718293a4b5c6";

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
        std::env::set_var("ARCANE_USER_ID", USER_ID);

        let fixture = Self {
            _dir: dir,
            root,
            _guard: guard,
        };
        fixture.write(
            "jwks.json",
            &json!({
                "keys": [{
                    "kty": "EC", "crv": "P-256", "alg": "ES256", "use": "sig",
                    "kid": KID, "x": PUBLIC_X, "y": PUBLIC_Y,
                }]
            })
            .to_string(),
        );
        fixture.write(
            "session.json",
            &json!({ "user_id": USER_ID, "updated_at": 1 }).to_string(),
        );
        fixture
    }

    fn write(&self, relative: &str, body: &str) {
        write_file(&self.root.join(relative), body);
    }

    /// What the desktop app publishes so the SDK can rehash it.
    fn write_device(&self, fprs: &[&str]) {
        let fingerprints: Vec<_> = fprs
            .iter()
            .map(|fpr| json!({ "kind": "account_key_fpr", "value": fpr }))
            .collect();
        self.write(
            &format!("device/{USER_ID}.json"),
            &json!({ "user_id": USER_ID, "updated_at": 1, "fingerprints": fingerprints })
                .to_string(),
        );
    }

    fn write_ticket(&self, dev: &str, expires_in: i64) {
        self.write_ticket_claiming(json!(dev), dev, expires_in);
    }

    /// `dev` is written verbatim, so a test can mint the list shape the backend
    /// actually emits as well as the bare string older builds emitted.
    fn write_ticket_claiming(&self, dev: serde_json::Value, cached: &str, expires_in: i64) {
        let now = now_unix();
        let claims = json!({
            "sub": USER_ID, "gid": GAME_ID, "own": true,
            "iat": now - 60, "nbf": now - 60, "exp": now + expires_in,
            "jti": "ticket-1", "dev": dev, "ver": 1,
            "iss": "arcane-drm", "aud": "arcane-game-sdk",
        });
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(KID.into());
        let key = EncodingKey::from_ec_pem(PRIVATE_KEY_PEM.as_bytes()).expect("signing key");
        let jwt = jsonwebtoken::encode(&header, &claims, &key).expect("sign ticket");

        self.write(
            &format!("tickets/{USER_ID}/{GAME_ID}.ticket"),
            &json!({
                "ticket": jwt,
                "cached_at": "2026-01-01T00:00:00Z",
                "expires_at": "2027-01-01T00:00:00Z",
                "game_id": GAME_ID,
                "user_id": USER_ID,
                "device_hash": cached,
                "drm_enabled": true,
            })
            .to_string(),
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for var in [
            "ARCANE_DRM_ROOT",
            "ARCANE_OFFLINE_ONLY",
            "ARCANE_GAME_ID",
            "ARCANE_USER_ID",
        ] {
            std::env::remove_var(var);
        }
    }
}

/// `sha256(fingerprint)[..16]` — the desktop's derivation, spelled out here so
/// the test is not just agreeing with the SDK's copy of it.
fn device_hash_of(fpr_hex: &str) -> String {
    let bytes = hex::decode(fpr_hex).expect("hex fingerprint");
    assert_eq!(bytes.len(), 32);
    hex::encode(&Sha256::digest(bytes)[..16])
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64
}

fn write_file(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
    fs::write(path, body).expect("write file");
}

#[test]
fn a_signed_ticket_bound_to_the_published_fingerprint_resolves_as_owned() {
    let fixture = Fixture::new();
    let expected = device_hash_of(ACCOUNT_KEY_FPR);
    fixture.write_device(&[ACCOUNT_KEY_FPR]);
    fixture.write_ticket(&expected, 3600);

    let client = ArcaneClient::init().expect("a valid ticket must resolve offline");

    assert_eq!(client.ownership(), OwnershipStatus::Owned);
    assert_eq!(client.user_id(), Some(USER_ID));
    assert_eq!(client.game_id(), GAME_ID);
    assert_eq!(
        client.device_hash(),
        expected,
        "the client reports the fingerprint the ticket was minted for"
    );
    assert!(client.ticket_expires_at().expect("expiry") > now_unix());
}

/// The exact production failure: the fingerprint on disk is not the one the
/// ticket was minted for, so `dev` matches nothing this machine can present.
#[test]
fn a_ticket_minted_for_another_fingerprint_is_a_device_mismatch() {
    let fixture = Fixture::new();
    let other = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
    fixture.write_device(&[ACCOUNT_KEY_FPR]);
    fixture.write_ticket(&device_hash_of(other), 3600);

    let err = ArcaneClient::init().expect_err("the ticket belongs to another key");

    assert_eq!(err.code(), "device_mismatch");
}

/// The signature is checked before the device is: a ticket whose `dev` names a
/// fingerprint we do hold must still be rejected when it was not signed by the
/// key in the JWKS.
#[test]
fn a_forged_signature_is_rejected_even_with_the_right_fingerprint() {
    let fixture = Fixture::new();
    let expected = device_hash_of(ACCOUNT_KEY_FPR);
    fixture.write_device(&[ACCOUNT_KEY_FPR]);
    fixture.write_ticket(&expected, 3600);

    // Swap the JWKS for an unrelated key, leaving the ticket untouched.
    fixture.write(
        "jwks.json",
        &json!({
            "keys": [{
                "kty": "EC", "crv": "P-256", "alg": "ES256", "use": "sig", "kid": KID,
                "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
                "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0",
            }]
        })
        .to_string(),
    );

    let err = ArcaneClient::init().expect_err("a ticket we did not sign must not pass");

    assert_eq!(err.code(), "ticket_invalid");
}

#[test]
fn an_expired_ticket_is_reported_as_expired_not_as_a_mismatch() {
    let fixture = Fixture::new();
    fixture.write_device(&[ACCOUNT_KEY_FPR]);
    fixture.write_ticket(&device_hash_of(ACCOUNT_KEY_FPR), -3600);

    let err = ArcaneClient::init().expect_err("the ticket is past exp");

    assert_eq!(err.code(), "ticket_expired");
    assert!(err.is_retryable());
}

/// The shape the backend actually mints: `dev` is a list of every fingerprint
/// the account could present. A build that only read a bare string rejected
/// every one of these before it could compare anything.
#[test]
fn a_dev_claim_that_is_a_list_is_matched_member_by_member() {
    let fixture = Fixture::new();
    let ours = device_hash_of(ACCOUNT_KEY_FPR);
    let theirs = device_hash_of("0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20");
    fixture.write_device(&[ACCOUNT_KEY_FPR]);
    // Ours is not first: the match must not depend on the order of the claim.
    fixture.write_ticket_claiming(json!([theirs, ours]), &ours, 3600);

    let client = ArcaneClient::init().expect("one of the claimed fingerprints is ours");

    assert_eq!(client.ownership(), OwnershipStatus::Owned);
    assert_eq!(
        client.device_hash(),
        ours,
        "the reported fingerprint is the one that matched, not the first claimed"
    );
}

/// A list that names only other machines is still a mismatch — accepting a list
/// must not mean accepting any list.
#[test]
fn a_list_naming_only_other_fingerprints_is_still_a_mismatch() {
    let fixture = Fixture::new();
    let a = device_hash_of("0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20");
    let b = device_hash_of("2122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f40");
    fixture.write_device(&[ACCOUNT_KEY_FPR]);
    fixture.write_ticket_claiming(json!([a, b]), &a, 3600);

    let err = ArcaneClient::init().expect_err("none of the claimed fingerprints is ours");

    assert_eq!(err.code(), "device_mismatch");
}

/// An empty list claims nothing, so it can satisfy nothing.
#[test]
fn an_empty_dev_claim_is_a_mismatch_rather_than_a_free_pass() {
    let fixture = Fixture::new();
    fixture.write_device(&[ACCOUNT_KEY_FPR]);
    fixture.write_ticket_claiming(json!([]), &device_hash_of(ACCOUNT_KEY_FPR), 3600);

    let err = ArcaneClient::init().expect_err("an empty claim binds the ticket to nothing");

    assert_eq!(err.code(), "device_mismatch");
}

/// The bare-string shape older backends emitted still verifies, so a ticket
/// already cached on a player's disk is not invalidated by this change.
#[test]
fn the_legacy_string_dev_claim_still_verifies() {
    let fixture = Fixture::new();
    let ours = device_hash_of(ACCOUNT_KEY_FPR);
    fixture.write_device(&[ACCOUNT_KEY_FPR]);
    fixture.write_ticket_claiming(json!(ours), &ours, 3600);

    let client = ArcaneClient::init().expect("a string claim is still a claim");

    assert_eq!(client.ownership(), OwnershipStatus::Owned);
    assert_eq!(client.device_hash(), ours);
}
