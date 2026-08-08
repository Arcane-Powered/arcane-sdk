//! Talk to the Arcane desktop loopback server (127.0.0.1:39284).
//!
//! When no valid offline ticket is available, the SDK asks the desktop to
//! refresh ownership online. If the loopback is down, it opens the
//! `arcane-powered://` deep link and waits for the health endpoint.

use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::error::SdkError;

pub(crate) const SDK_HTTP_PORT: u16 = 39284;
const HEALTH_PATH: &str = "/v1/health";
const REFRESH_PATH_PREFIX: &str = "/v1/games";
const POLL_INTERVAL: Duration = Duration::from_millis(400);
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Debug, Deserialize)]
pub(crate) struct HealthResponse {
    pub ok: bool,
    #[serde(default)]
    pub authenticated: bool,
}

#[derive(Debug, Deserialize)]
struct OwnershipRefreshOk {
    ok: bool,
    #[serde(default)]
    drm_enabled: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SdkErrorBody {
    pub error: String,
    #[serde(default)]
    pub message: String,
}

fn base_url() -> String {
    format!("http://127.0.0.1:{SDK_HTTP_PORT}")
}

fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(2))
        .timeout_read(Duration::from_secs(15))
        .build()
}

/// Probe whether the Arcane desktop SDK server is listening.
pub(crate) fn probe_health() -> Result<HealthResponse, SdkError> {
    let url = format!("{}{HEALTH_PATH}", base_url());
    let resp = http_agent()
        .get(&url)
        .call()
        .map_err(|e| SdkError::ArcaneUnavailable(format!("Arcane desktop not reachable: {e}")))?;

    let status = resp.status();
    let body = resp
        .into_string()
        .map_err(|e| SdkError::ArcaneUnavailable(format!("health body: {e}")))?;

    if !(200..300).contains(&status) {
        return Err(SdkError::ArcaneUnavailable(format!(
            "health HTTP {status}: {body}"
        )));
    }

    serde_json::from_str(&body).map_err(|e| {
        SdkError::ArcaneUnavailable(format!("invalid health payload: {e}; body={body}"))
    })
}

/// Open the Arcane desktop deep link so the app starts (or focuses) and serves loopback.
pub(crate) fn open_arcane_deep_link(game_id: &str) -> Result<(), SdkError> {
    let url = format!("arcane-powered://sdk/ownership?game_id={game_id}");
    open::that(&url).map_err(|e| {
        SdkError::ArcaneUnavailable(format!(
            "Could not open Arcane Powered (`{url}`): {e}. Install or launch the desktop app."
        ))
    })
}

/// Ensure the desktop loopback is up, launching Arcane via deep link if needed.
pub(crate) fn ensure_arcane_desktop(game_id: &str) -> Result<HealthResponse, SdkError> {
    if let Ok(health) = probe_health() {
        return Ok(health);
    }

    open_arcane_deep_link(game_id)?;

    let deadline = Instant::now() + LAUNCH_TIMEOUT;
    while Instant::now() < deadline {
        if let Ok(health) = probe_health() {
            return Ok(health);
        }
        thread::sleep(POLL_INTERVAL);
    }

    Err(SdkError::ArcaneUnavailable(
        "Could not open or reach Arcane Powered. Install or launch the desktop app, then retry."
            .into(),
    ))
}

/// Map a loopback JSON error body to a stable [`SdkError`].
pub(crate) fn map_refresh_error(body: &SdkErrorBody) -> SdkError {
    let msg = if body.message.trim().is_empty() {
        body.error.clone()
    } else {
        body.message.clone()
    };

    match body.error.as_str() {
        "not_owned" => SdkError::NotOwned(
            "You do not own this game. Buy it on the Arcane Store or marketplace.".into(),
        ),
        "offline" => SdkError::NetworkRequired(
            "Connect to the internet once via Arcane to obtain an ownership ticket.".into(),
        ),
        "not_authenticated" => SdkError::NotAuthenticated(
            "Sign in to the Arcane desktop app, then retry.".into(),
        ),
        "game_not_found" => SdkError::TicketInvalid(msg),
        "cloud_unreachable" | "cloud_error" => SdkError::NetworkRequired(msg),
        "internal" => SdkError::Io(msg),
        other => SdkError::TicketInvalid(format!("{other}: {msg}")),
    }
}

/// Ask the desktop to refresh (or issue) an ownership ticket for `game_id`.
pub(crate) fn refresh_ownership_via_desktop(game_id: &str) -> Result<(), SdkError> {
    let health = ensure_arcane_desktop(game_id)?;

    if !health.ok {
        return Err(SdkError::ArcaneUnavailable(
            "Arcane desktop reported an unhealthy SDK server.".into(),
        ));
    }

    if !health.authenticated {
        return Err(SdkError::NotAuthenticated(
            "Sign in to the Arcane desktop app, then retry.".into(),
        ));
    }

    let url = format!(
        "{}{REFRESH_PATH_PREFIX}/{}/ownership/refresh",
        base_url(),
        game_id
    );

    let resp = match http_agent().post(&url).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(code, response)) => {
            let body_text = response
                .into_string()
                .unwrap_or_else(|_| String::new());
            if let Ok(err_body) = serde_json::from_str::<SdkErrorBody>(&body_text) {
                return Err(map_refresh_error(&err_body));
            }
            return Err(SdkError::TicketInvalid(format!(
                "ownership refresh HTTP {code}: {body_text}"
            )));
        }
        Err(e) => {
            return Err(SdkError::ArcaneUnavailable(format!(
                "ownership refresh failed: {e}"
            )));
        }
    };

    let status = resp.status();
    let body_text = resp
        .into_string()
        .map_err(|e| SdkError::Io(format!("refresh body: {e}")))?;

    if !(200..300).contains(&status) {
        if let Ok(err_body) = serde_json::from_str::<SdkErrorBody>(&body_text) {
            return Err(map_refresh_error(&err_body));
        }
        return Err(SdkError::TicketInvalid(format!(
            "ownership refresh HTTP {status}: {body_text}"
        )));
    }

    let parsed: OwnershipRefreshOk = serde_json::from_str(&body_text).map_err(|e| {
        SdkError::TicketInvalid(format!("unexpected refresh payload: {e}; body={body_text}"))
    })?;

    if !parsed.ok && parsed.drm_enabled {
        return Err(SdkError::TicketInvalid(
            "ownership refresh returned ok=false".into(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_not_owned() {
        let err = map_refresh_error(&SdkErrorBody {
            error: "not_owned".into(),
            message: "This account does not own the requested game.".into(),
        });
        assert_eq!(err.code(), "not_owned");
        assert!(err.to_string().contains("Arcane Store"));
    }

    #[test]
    fn maps_offline_to_network_required() {
        let err = map_refresh_error(&SdkErrorBody {
            error: "offline".into(),
            message: "Cannot refresh while offline.".into(),
        });
        assert_eq!(err.code(), "network_required");
    }

    #[test]
    fn maps_not_authenticated() {
        let err = map_refresh_error(&SdkErrorBody {
            error: "not_authenticated".into(),
            message: "No session.".into(),
        });
        assert_eq!(err.code(), "not_authenticated");
    }

    #[test]
    fn maps_cloud_unreachable() {
        let err = map_refresh_error(&SdkErrorBody {
            error: "cloud_unreachable".into(),
            message: "backend down".into(),
        });
        assert_eq!(err.code(), "network_required");
    }
}
