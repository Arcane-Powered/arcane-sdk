use std::fs;

use jsonwebtoken::jwk::Jwk;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;

use crate::device::{device_hash, now_unix, CachedTicketFile};
use crate::error::{OwnershipStatus, SdkError};
use crate::paths::{jwks_path, load_ticket_file};

const ISS: &str = "arcane-drm";
const AUD: &str = "arcane-game-sdk";
const CLOCK_SKEW_SECS: i64 = 300;

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // deserialized JWT claims; only a subset is checked today
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

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<serde_json::Value>,
}

fn load_decoding_key(kid: Option<&str>) -> Result<DecodingKey, SdkError> {
    let path = jwks_path().map_err(SdkError::Io)?;
    let raw = fs::read_to_string(&path).map_err(|_| {
        SdkError::TicketInvalid(
            "Missing local JWKS. Refresh a ticket online via the Arcane desktop app.".into(),
        )
    })?;
    let jwks: Jwks = serde_json::from_str(&raw)
        .map_err(|e| SdkError::TicketInvalid(format!("invalid JWKS cache: {e}")))?;
    let key_jwk = if let Some(kid) = kid {
        jwks.keys
            .into_iter()
            .find(|k| k.get("kid").and_then(|v| v.as_str()) == Some(kid))
            .ok_or_else(|| SdkError::TicketInvalid(format!("unknown kid `{kid}` in JWKS")))?
    } else {
        jwks.keys
            .into_iter()
            .next()
            .ok_or_else(|| SdkError::TicketInvalid("empty JWKS".into()))?
    };
    let jwk: Jwk = serde_json::from_value(key_jwk)
        .map_err(|e| SdkError::TicketInvalid(format!("JWKS entry is not a valid JWK: {e}")))?;
    DecodingKey::from_jwk(&jwk)
        .map_err(|e| SdkError::TicketInvalid(format!("cannot build decoding key: {e}")))
}

pub(crate) fn verify_ticket(
    jwt: &str,
    expected_game: &str,
    expected_device_hash: &str,
) -> Result<OwnershipTicketClaims, SdkError> {
    let header = decode_header(jwt)
        .map_err(|e| SdkError::TicketInvalid(format!("JWT header: {e}")))?;
    let key = load_decoding_key(header.kid.as_deref())?;

    let mut validation = Validation::new(Algorithm::ES256);
    validation.set_issuer(&[ISS]);
    validation.set_audience(&[AUD]);
    validation.leeway = CLOCK_SKEW_SECS as u64;

    let data = decode::<OwnershipTicketClaims>(jwt, &key, &validation).map_err(|e| {
        let msg = e.to_string();
        if msg.to_lowercase().contains("expired") {
            SdkError::TicketExpired(msg)
        } else {
            SdkError::TicketInvalid(msg)
        }
    })?;

    let claims = data.claims;
    let now = now_unix();
    if now + CLOCK_SKEW_SECS < claims.iat || now + CLOCK_SKEW_SECS < claims.nbf {
        return Err(SdkError::ClockRollback(
            "System clock appears earlier than ticket issuance.".into(),
        ));
    }
    if claims.gid != expected_game {
        return Err(SdkError::TicketInvalid(format!(
            "ticket game `{}` does not match `{expected_game}`",
            claims.gid
        )));
    }
    if !claims.own {
        return Err(SdkError::TicketInvalid("ticket own claim is false".into()));
    }
    if claims.dev != expected_device_hash {
        return Err(SdkError::DeviceMismatch(
            "Ownership ticket was issued for a different machine.".into(),
        ));
    }
    Ok(claims)
}

fn check_clock_rollback(file: &CachedTicketFile) -> Result<(), SdkError> {
    let now = now_unix();
    if let Some(last) = file.last_seen_wall_time {
        if now + CLOCK_SKEW_SECS < last {
            return Err(SdkError::ClockRollback(
                "System clock moved backwards relative to the cached ticket.".into(),
            ));
        }
    }
    Ok(())
}

pub fn check_ownership_offline(game_id: &str) -> Result<OwnershipStatus, SdkError> {
    let file = load_ticket_file(game_id)?;
    check_clock_rollback(&file)?;

    if !file.drm_enabled {
        return Ok(OwnershipStatus::DrmDisabled);
    }
    if file.ticket.trim().is_empty() {
        return Err(SdkError::TicketMissing(
            "DRM is enabled but the cached ticket is empty. Reconnect online.".into(),
        ));
    }

    let local_dev = device_hash()?;
    if file.device_hash != local_dev {
        return Err(SdkError::DeviceMismatch(
            "Cached ticket device fingerprint does not match this machine.".into(),
        ));
    }

    let _claims = verify_ticket(&file.ticket, game_id, &local_dev)?;
    Ok(OwnershipStatus::Owned)
}
