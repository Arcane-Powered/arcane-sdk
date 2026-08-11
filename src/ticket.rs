//! Offline ownership ticket verification.

use std::fs;

use jsonwebtoken::jwk::Jwk;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;

use crate::device::{device_hash, now_unix, short_hash, CachedTicketFile};
use crate::error::{OwnershipStatus, SdkError};
use crate::paths::{jwks_path, resolve_ticket};

const ISS: &str = "arcane-drm";
const AUD: &str = "arcane-game-sdk";
const CLOCK_SKEW_SECS: i64 = 300;

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
    pub dev: String,
    pub ver: i64,
    #[serde(default)]
    pub iss: Option<String>,
    #[serde(default)]
    pub aud: Option<serde_json::Value>,
}

/// Everything a successful offline check learned — the client keeps this in memory
/// so callers never re-derive it.
#[derive(Debug, Clone)]
pub(crate) struct OwnershipCheck {
    pub status: OwnershipStatus,
    pub user_id: Option<String>,
    pub game_id: Option<String>,
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

pub(crate) fn verify_ticket(
    jwt: &str,
    public_key: &str,
    expected_device_hash: &str,
) -> Result<OwnershipTicketClaims, SdkError> {
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
    if claims.gid != public_key {
        return Err(SdkError::ticket_invalid(
            "The cached ticket was issued for a different title.",
        )
        .with_hint(
            "Confirm the public key compiled into your build matches the one in the Arcane portal.",
        )
        .with_context("expected", public_key)
        .with_context("ticket_gid", &claims.gid));
    }
    if !claims.own {
        return Err(
            SdkError::ticket_invalid("The ticket does not assert ownership.")
                .with_hint(
                    "Refresh via Arcane desktop; the account may have lost access to this title.",
                )
                .with_context("public_key", public_key),
        );
    }
    if claims.dev != expected_device_hash {
        return Err(SdkError::device_mismatch(
            "This ownership ticket was issued for a different machine.",
        )
        .with_hint("Refresh ownership on this machine via Arcane desktop while online.")
        .with_context("this_device", short_hash(expected_device_hash))
        .with_context("ticket_device", short_hash(&claims.dev)));
    }
    Ok(claims)
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

/// Verify the cached ticket for `public_key` without touching the network.
pub(crate) fn check_ownership_offline(public_key: &str) -> Result<OwnershipCheck, SdkError> {
    let resolved = resolve_ticket(public_key)?;
    let file = &resolved.file;
    check_clock_rollback(file)?;

    let local_device = device_hash()?;
    let user_id = non_empty(&file.user_id);
    let game_id = non_empty(&file.game_id);

    if !file.drm_enabled {
        return Ok(OwnershipCheck {
            status: OwnershipStatus::DrmDisabled,
            user_id,
            game_id,
            ticket_expires_at: None,
            device_hash: local_device,
        });
    }

    if file.ticket.trim().is_empty() {
        return Err(SdkError::ticket_missing(
            "DRM is enabled for this title but the cached ticket is empty.",
        )
        .with_hint("Open the Arcane desktop app while online so it can mint a ticket.")
        .with_context("public_key", public_key)
        .with_context("path", resolved.path.display()));
    }

    if file.device_hash != local_device {
        return Err(SdkError::device_mismatch(
            "The cached ticket was stored for a different machine.",
        )
        .with_hint("Refresh ownership on this machine via Arcane desktop while online.")
        .with_context("this_device", short_hash(&local_device))
        .with_context("cached_device", short_hash(&file.device_hash))
        .with_context("path", resolved.path.display()));
    }

    let claims = verify_ticket(&file.ticket, public_key, &local_device)?;

    Ok(OwnershipCheck {
        status: OwnershipStatus::Owned,
        user_id: user_id.or_else(|| non_empty(&claims.sub)),
        game_id,
        ticket_expires_at: Some(claims.exp),
        device_hash: local_device,
    })
}
