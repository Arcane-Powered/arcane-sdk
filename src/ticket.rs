//! Offline ownership ticket verification.

use std::fs;

use jsonwebtoken::jwk::Jwk;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;

use crate::device::{device_hashes, now_unix, short_hashes, CachedTicketFile};
use crate::error::{OwnershipStatus, SdkError};
use crate::paths::{jwks_path, resolve_ticket};

const ISS: &str = "arcane-drm";
const AUD: &str = "arcane-game-sdk";
const CLOCK_SKEW_SECS: i64 = 300;

/// The `dev` claim: one fingerprint, or every fingerprint the account could
/// present when the ticket was minted.
///
/// The backend emits a list so a key rotation does not invalidate tickets that
/// are already cached; older backends emitted a bare string. Both shapes are
/// accepted, and a build that only understood the string would reject every
/// ticket the current backend mints.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum DevClaim {
    One(String),
    Many(Vec<String>),
}

impl DevClaim {
    fn hashes(&self) -> &[String] {
        match self {
            Self::One(hash) => std::slice::from_ref(hash),
            Self::Many(hashes) => hashes,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // full JWT claim set; only a subset is enforced today
pub(crate) struct OwnershipTicketClaims {
    pub sub: String,
    pub gid: String,
    pub own: bool,
    pub iat: i64,
    pub nbf: i64,
    pub exp: i64,
    pub jti: String,
    pub dev: DevClaim,
    pub ver: i64,
    #[serde(default)]
    pub iss: Option<String>,
    #[serde(default)]
    pub aud: Option<serde_json::Value>,
}

/// A ticket that passed every check, and the fingerprint it turned out to be
/// bound to — which of this machine's fingerprints matched, not whichever one
/// happens to be listed first.
#[derive(Debug, Clone)]
pub(crate) struct VerifiedTicket {
    pub claims: OwnershipTicketClaims,
    pub device_hash: String,
}

/// Everything a successful offline check learned — the client keeps this in memory
/// so callers never re-derive it.
#[derive(Debug, Clone)]
pub(crate) struct OwnershipCheck {
    pub status: OwnershipStatus,
    pub user_id: Option<String>,
    pub ticket_expires_at: Option<i64>,
    pub device_hash: String,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<serde_json::Value>,
}

fn load_decoding_key(kid: Option<&str>) -> Result<DecodingKey, SdkError> {
    let path = jwks_path()?;
    let raw = fs::read_to_string(&path).map_err(|e| {
        SdkError::ticket_invalid(format!("The local JWKS verification keys are missing: {e}"))
            .with_hint(
                "Open the Arcane desktop app once while online — it downloads jwks.json \
                 alongside the ticket.",
            )
            .with_context("path", path.display())
    })?;
    let jwks: Jwks = serde_json::from_str(&raw).map_err(|e| {
        SdkError::ticket_invalid(format!("The local JWKS cache is not valid JSON: {e}"))
            .with_hint("Delete the file and let Arcane desktop re-download it.")
            .with_context("path", path.display())
    })?;

    let key_jwk = match kid {
        Some(kid) => jwks
            .keys
            .into_iter()
            .find(|k| k.get("kid").and_then(|v| v.as_str()) == Some(kid))
            .ok_or_else(|| {
                SdkError::ticket_invalid(
                    "The ticket was signed with a key that is not in the local JWKS cache.",
                )
                .with_hint("Refresh online via Arcane desktop to pick up rotated signing keys.")
                .with_context("kid", kid)
                .with_context("path", path.display())
            })?,
        None => jwks.keys.into_iter().next().ok_or_else(|| {
            SdkError::ticket_invalid("The local JWKS cache contains no keys.")
                .with_hint("Delete the file and let Arcane desktop re-download it.")
                .with_context("path", path.display())
        })?,
    };

    let jwk: Jwk = serde_json::from_value(key_jwk).map_err(|e| {
        SdkError::ticket_invalid(format!("A JWKS entry is not a valid JWK: {e}"))
            .with_context("path", path.display())
    })?;
    DecodingKey::from_jwk(&jwk).map_err(|e| {
        SdkError::ticket_invalid(format!(
            "Could not build a verification key from the JWKS: {e}"
        ))
        .with_context("path", path.display())
    })
}

/// Verify `jwt` against the fingerprints this machine can present.
///
/// `local_devices` is a set, not a single value: an account that rotated its key
/// can present the old fingerprint and the new one, and a ticket minted before
/// the rotation names the old one. Widening the local set proves nothing on its
/// own — `dev` is signed by the backend, so only a hash it actually minted can
/// match.
pub(crate) fn verify_ticket(
    jwt: &str,
    game_id: &str,
    local_devices: &[String],
) -> Result<VerifiedTicket, SdkError> {
    let header = decode_header(jwt).map_err(|e| {
        SdkError::ticket_invalid(format!("The ticket is not a readable JWT: {e}"))
            .with_hint("Delete the cached ticket and refresh via Arcane desktop.")
    })?;
    let key = load_decoding_key(header.kid.as_deref())?;

    let mut validation = Validation::new(Algorithm::ES256);
    validation.set_issuer(&[ISS]);
    validation.set_audience(&[AUD]);
    validation.leeway = CLOCK_SKEW_SECS as u64;

    let data = decode::<OwnershipTicketClaims>(jwt, &key, &validation).map_err(|e| {
        let detail = e.to_string();
        if detail.to_lowercase().contains("expired") {
            SdkError::ticket_expired("The cached ownership ticket has expired.")
                .with_hint("Reconnect online via Arcane desktop to mint a fresh ticket.")
                .with_context("detail", detail)
        } else {
            SdkError::ticket_invalid(format!("Ticket signature or claims rejected: {detail}"))
                .with_hint("Refresh via Arcane desktop; if it persists, the ticket is corrupt.")
                .with_context("issuer_expected", ISS)
                .with_context("audience_expected", AUD)
        }
    })?;

    let claims = data.claims;
    let now = now_unix();
    if now + CLOCK_SKEW_SECS < claims.iat || now + CLOCK_SKEW_SECS < claims.nbf {
        return Err(SdkError::clock_rollback(
            "The system clock is earlier than the ticket issue time.",
        )
        .with_hint("Enable automatic date & time on this machine, then retry.")
        .with_context("now", now)
        .with_context("ticket_iat", claims.iat)
        .with_context("ticket_nbf", claims.nbf));
    }
    if claims.gid != game_id {
        return Err(SdkError::ticket_invalid(
            "The cached ticket was issued for a different title.",
        )
        .with_hint(
            "Confirm the game id compiled into your build matches the one in the Arcane portal.",
        )
        .with_context("expected", game_id)
        .with_context("ticket_gid", &claims.gid));
    }
    if !claims.own {
        return Err(
            SdkError::ticket_invalid("The ticket does not assert ownership.")
                .with_hint(
                    "Refresh via Arcane desktop; the account may have lost access to this title.",
                )
                .with_context("game_id", game_id),
        );
    }
    let ticket_devices = claims.dev.hashes();
    let Some(device_hash) = local_devices
        .iter()
        .find(|local| {
            ticket_devices
                .iter()
                .any(|claimed| claimed.eq_ignore_ascii_case(local))
        })
        .cloned()
    else {
        return Err(SdkError::device_mismatch(
            "This ownership ticket was issued for a different machine.",
        )
        .with_hint("Refresh ownership on this machine via Arcane desktop while online.")
        .with_context("this_device", short_hashes(local_devices))
        .with_context("ticket_device", short_hashes(ticket_devices)));
    };

    Ok(VerifiedTicket {
        device_hash,
        claims,
    })
}

fn check_clock_rollback(file: &CachedTicketFile) -> Result<(), SdkError> {
    let now = now_unix();
    if let Some(last) = file.last_seen_wall_time {
        if now + CLOCK_SKEW_SECS < last {
            return Err(SdkError::clock_rollback(
                "The system clock moved backwards since the last ownership check.",
            )
            .with_hint("Enable automatic date & time on this machine, then retry.")
            .with_context("now", now)
            .with_context("last_seen", last));
        }
    }
    Ok(())
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Verify the cached ticket for `game_id` without touching the network.
pub(crate) fn check_ownership_offline(game_id: &str) -> Result<OwnershipCheck, SdkError> {
    let resolved = resolve_ticket(game_id)?;
    let file = &resolved.file;
    check_clock_rollback(file)?;

    let user_id = non_empty(&file.user_id).unwrap_or_else(|| resolved.account.clone());

    if !file.drm_enabled {
        // Best-effort: a title that does not enforce DRM must not fail to start
        // because this machine has no fingerprint file yet.
        return Ok(OwnershipCheck {
            device_hash: crate::device::primary_device_hash(&resolved.account).unwrap_or_default(),
            status: OwnershipStatus::DrmDisabled,
            user_id: Some(user_id),
            ticket_expires_at: None,
        });
    }

    if file.ticket.trim().is_empty() {
        return Err(SdkError::ticket_missing(
            "DRM is enabled for this title but the cached ticket is empty.",
        )
        .with_hint("Open the Arcane desktop app while online so it can mint a ticket.")
        .with_context("game_id", game_id)
        .with_context("path", resolved.path.display()));
    }

    let local_devices = device_hashes(&resolved.account)?;

    // The `device_hash` field of the ticket file is deliberately *not* enforced.
    // It is unsigned, so it proves nothing the `dev` claim does not, and it can
    // legitimately disagree: the desktop stores whichever fingerprint it holds
    // first, which may be a session key, while the signed claim covers every
    // fingerprint the account can present. Failing on it rejects valid tickets.
    let verified = verify_ticket(&file.ticket, game_id, &local_devices)
        .map_err(|e| e.with_context("path", resolved.path.display()))?;

    Ok(OwnershipCheck {
        status: OwnershipStatus::Owned,
        user_id: Some(user_id),
        device_hash: verified.device_hash,
        ticket_expires_at: Some(verified.claims.exp),
    })
}
