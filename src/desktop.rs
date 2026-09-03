//! Talk to the Arcane desktop loopback server (default `127.0.0.1:39284`).
//!
//! When no valid offline ticket is available, the SDK asks the desktop to refresh
//! ownership online. If the loopback is down, it opens the `arcane-powered://`
//! deep link and waits for the health endpoint.

use std::env;
use std::thread;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;

use crate::error::SdkError;

pub(crate) const DEFAULT_SDK_HTTP_PORT: u16 = 39284;

/// Overrides the loopback port. Intended for tests and QA — a game should never
/// set it.
pub(crate) const SDK_PORT_ENV: &str = "ARCANE_SDK_PORT";

/// When set to `1` / `true`, the SDK never contacts or launches Arcane desktop:
/// a missing or expired ticket surfaces directly instead of triggering a refresh.
///
/// Intended for developers exercising their offline and error handling. It can
/// only make a check fail earlier — it never lets one pass.
pub(crate) const OFFLINE_ONLY_ENV: &str = "ARCANE_OFFLINE_ONLY";

const HEALTH_PATH: &str = "/v1/health";
const POLL_INTERVAL: Duration = Duration::from_millis(400);
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(25);

pub(crate) const GAMES_PATH_PREFIX: &str = "/v1/games";
pub(crate) const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Deserialize)]
pub(crate) struct HealthResponse {
    pub ok: bool,
    #[serde(default)]
    pub authenticated: bool,
    /// Added by newer Arcane desktop builds; absent on older ones.
    #[serde(default)]
    pub user_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OwnershipRefreshOk {
    ok: bool,
    #[serde(default)]
    drm_enabled: bool,
    /// Added by newer Arcane desktop builds; absent on older ones.
    #[serde(default)]
    user_id: Option<String>,
    /// Canonical title id, added by newer Arcane desktop builds.
    #[serde(default)]
    game_id: Option<String>,
}

/// What a successful refresh told us. Every field beyond `drm_enabled` is
/// best-effort — an older desktop leaves them `None` and the SDK still works.
#[derive(Debug, Clone, Default)]
pub(crate) struct RefreshOutcome {
    pub drm_enabled: bool,
    pub user_id: Option<String>,
    pub game_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SdkErrorBody {
    pub error: String,
    #[serde(default)]
    pub message: String,
}

pub(crate) fn offline_only() -> bool {
    matches!(
        env::var(OFFLINE_ONLY_ENV).ok().as_deref().map(str::trim),
        Some("1") | Some("true")
    )
}

pub(crate) fn sdk_http_port() -> u16 {
    env::var(SDK_PORT_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<u16>().ok())
        .filter(|port| *port != 0)
        .unwrap_or(DEFAULT_SDK_HTTP_PORT)
}

fn base_url() -> String {
    format!("http://127.0.0.1:{}", sdk_http_port())
}

fn http_agent(read_timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(2))
        .timeout_read(read_timeout)
        .build()
}

#[derive(Debug)]
pub(crate) enum CallFailure {
    Transport(String),
    Status {
        code: u16,
        body: String,
        error: Option<SdkErrorBody>,
    },
    Decode {
        detail: String,
        body: String,
    },
}

#[derive(Debug)]
pub(crate) struct DesktopCall {
    pub url: String,
    pub failure: CallFailure,
}

impl DesktopCall {
    pub(crate) fn desktop_error(&self) -> Option<&str> {
        match &self.failure {
            CallFailure::Status {
                error: Some(body), ..
            } => Some(body.error.as_str()),
            _ => None,
        }
    }

    pub(crate) fn into_sdk_error(self) -> SdkError {
        match self.failure {
            CallFailure::Transport(detail) => {
                SdkError::arcane_unavailable(format!("Arcane desktop is not reachable: {detail}"))
                    .with_hint("Launch the Arcane Powered desktop app, then retry.")
                    .with_context("url", self.url)
            }
            CallFailure::Status {
                error: Some(body), ..
            } => map_desktop_error(&body),
            CallFailure::Status {
                code: 404, body, ..
            } => SdkError::feature_unavailable(
                "This Arcane desktop build does not support that feature yet.",
            )
            .with_hint("Update the Arcane Powered desktop app.")
            .with_context("url", self.url)
            .with_context("body", truncate(&body, 200)),
            CallFailure::Status { code, body, .. } => {
                SdkError::arcane_unavailable("Arcane desktop returned an unexpected status.")
                    .with_hint("Restart the Arcane Powered desktop app, then retry.")
                    .with_context("url", self.url)
                    .with_context("http_status", code)
                    .with_context("body", truncate(&body, 200))
            }
            CallFailure::Decode { detail, body } => {
                SdkError::arcane_unavailable(format!("Unexpected Arcane desktop payload: {detail}"))
                    .with_hint("The Arcane desktop app may be older than this SDK — update it.")
                    .with_context("url", self.url)
                    .with_context("body", truncate(&body, 200))
            }
        }
    }
}

fn call_json<T: DeserializeOwned>(
    request: ureq::Request,
    body: Option<Value>,
    url: String,
) -> Result<T, DesktopCall> {
    let sent = match body {
        Some(value) => request
            .set("Content-Type", "application/json")
            .send_string(&value.to_string()),
        None => request.call(),
    };

    let response = match sent {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            let error = serde_json::from_str::<SdkErrorBody>(&body).ok();
            return Err(DesktopCall {
                url,
                failure: CallFailure::Status { code, body, error },
            });
        }
        Err(e) => {
            return Err(DesktopCall {
                url,
                failure: CallFailure::Transport(e.to_string()),
            })
        }
    };

    let status = response.status();
    let body = match response.into_string() {
        Ok(body) => body,
        Err(e) => {
            return Err(DesktopCall {
                url,
                failure: CallFailure::Transport(e.to_string()),
            })
        }
    };

    if !(200..300).contains(&status) {
        let error = serde_json::from_str::<SdkErrorBody>(&body).ok();
        return Err(DesktopCall {
            url,
            failure: CallFailure::Status {
                code: status,
                body,
                error,
            },
        });
    }

    serde_json::from_str::<T>(&body).map_err(|e| DesktopCall {
        url,
        failure: CallFailure::Decode {
            detail: e.to_string(),
            body,
        },
    })
}

pub(crate) fn get_json<T: DeserializeOwned>(
    path: &str,
    read_timeout: Duration,
) -> Result<T, DesktopCall> {
    let url = format!("{}{path}", base_url());
    let request = http_agent(read_timeout).get(&url);
    call_json(request, None, url)
}

pub(crate) fn post_json<T: DeserializeOwned>(
    path: &str,
    body: Option<Value>,
    read_timeout: Duration,
) -> Result<T, DesktopCall> {
    let url = format!("{}{path}", base_url());
    let request = http_agent(read_timeout).post(&url);
    call_json(request, body, url)
}

/// Probe whether the Arcane desktop SDK server is listening.
pub(crate) fn probe_health() -> Result<HealthResponse, SdkError> {
    get_json::<HealthResponse>(HEALTH_PATH, DEFAULT_READ_TIMEOUT).map_err(health_error)
}

fn health_error(call: DesktopCall) -> SdkError {
    match call.failure {
        CallFailure::Transport(detail) => {
            SdkError::arcane_unavailable(format!("Arcane desktop is not reachable: {detail}"))
                .with_hint("Launch the Arcane Powered desktop app, then retry.")
                .with_context("url", call.url)
        }
        CallFailure::Status { code, body, .. } => {
            SdkError::arcane_unavailable("Arcane desktop returned an unhealthy status.")
                .with_hint("Restart the Arcane Powered desktop app, then retry.")
                .with_context("url", call.url)
                .with_context("http_status", code)
                .with_context("body", truncate(&body, 200))
        }
        CallFailure::Decode { detail, body } => {
            SdkError::arcane_unavailable(format!("Unexpected health payload: {detail}"))
                .with_hint("The Arcane desktop app may be older than this SDK — update it.")
                .with_context("url", call.url)
                .with_context("body", truncate(&body, 200))
        }
    }
}

/// Open the Arcane desktop deep link so the app starts (or focuses) and serves loopback.
///
/// `public_key` is interpolated raw; it is validated against a strict charset by
/// [`crate::client::validate_public_key`] before it can reach this function.
pub(crate) fn open_arcane_deep_link(public_key: &str) -> Result<(), SdkError> {
    let url = format!("arcane-powered://sdk/ownership?game_id={public_key}");
    open::that(&url).map_err(|e| {
        SdkError::arcane_unavailable(format!("Could not open Arcane Powered: {e}"))
            .with_hint("Install the Arcane Powered desktop app, or launch it manually and retry.")
            .with_context("deep_link", &url)
    })
}

/// Ensure the desktop loopback is up, launching Arcane via deep link if needed.
pub(crate) fn ensure_arcane_desktop(public_key: &str) -> Result<HealthResponse, SdkError> {
    if let Ok(health) = probe_health() {
        return Ok(health);
    }

    open_arcane_deep_link(public_key)?;

    let started = Instant::now();
    let deadline = started + LAUNCH_TIMEOUT;
    let mut last_error: Option<SdkError> = None;
    while Instant::now() < deadline {
        match probe_health() {
            Ok(health) => return Ok(health),
            Err(err) => last_error = Some(err),
        }
        thread::sleep(POLL_INTERVAL);
    }

    let mut err = SdkError::arcane_unavailable(
        "Arcane Powered did not start listening after the deep link was opened.",
    )
    .with_hint("Install or launch the Arcane Powered desktop app, then retry.")
    .with_context("port", sdk_http_port())
    .with_context("waited_secs", started.elapsed().as_secs());
    if let Some(last) = last_error {
        err = err.with_context("last_probe", last.message());
    }
    Err(err)
}

/// Map a loopback JSON error body to a stable [`SdkError`].
pub(crate) fn map_desktop_error(body: &SdkErrorBody) -> SdkError {
    let detail = if body.message.trim().is_empty() {
        body.error.clone()
    } else {
        body.message.clone()
    };

    match body.error.as_str() {
        "not_owned" => SdkError::not_owned("You do not own this game.")
            .with_hint("Buy it on the Arcane Store or marketplace, then retry.")
            .with_context("detail", detail),
        "offline" => {
            SdkError::network_required("Arcane could not reach the internet to confirm ownership.")
                .with_hint("Connect to the internet once via Arcane to obtain an ownership ticket.")
                .with_context("detail", detail)
        }
        "not_authenticated" => {
            SdkError::not_authenticated("Nobody is signed in to the Arcane desktop app.")
                .with_hint("Sign in to the Arcane desktop app, then retry.")
                .with_context("detail", detail)
        }
        "unknown_achievement" => {
            SdkError::unknown_achievement("Arcane does not know this achievement for this title.")
                .with_hint(
                    "Check the achievement key against the Arcane portal, and that the \
                     achievement is published for this title.",
                )
                .with_context("detail", detail)
        }
        "game_not_found" => SdkError::ticket_invalid("Arcane does not know this title.")
            .with_hint(
                "Confirm the public key compiled into your build matches the one in the \
                 Arcane portal, and that the title is published.",
            )
            .with_context("detail", detail),
        "cloud_unreachable" | "cloud_error" => {
            SdkError::network_required("The Arcane backend could not be reached.")
                .with_hint("Retry in a moment; if it persists, check the Arcane status page.")
                .with_context("detail", detail)
        }
        "internal" => SdkError::internal("The Arcane desktop app hit an internal error.")
            .with_hint("Restart the Arcane Powered desktop app, then retry.")
            .with_context("detail", detail),
        other => SdkError::ticket_invalid("Arcane desktop refused the ownership refresh.")
            .with_hint("This error code is newer than your SDK — update arcane-sdk.")
            .with_context("desktop_error", other)
            .with_context("detail", detail),
    }
}

/// Ask the desktop to refresh (or issue) an ownership ticket for `public_key`.
pub(crate) fn refresh_ownership_via_desktop(public_key: &str) -> Result<RefreshOutcome, SdkError> {
    let health = ensure_arcane_desktop(public_key)?;

    if !health.ok {
        return Err(SdkError::arcane_unavailable(
            "Arcane desktop reported an unhealthy SDK server.",
        )
        .with_hint("Restart the Arcane Powered desktop app, then retry.")
        .with_context("port", sdk_http_port()));
    }

    if !health.authenticated {
        return Err(
            SdkError::not_authenticated("Nobody is signed in to the Arcane desktop app.")
                .with_hint("Sign in to the Arcane desktop app, then retry.")
                .with_context("public_key", public_key),
        );
    }

    let parsed: OwnershipRefreshOk = post_json(
        &format!("{GAMES_PATH_PREFIX}/{public_key}/ownership/refresh"),
        None,
        DEFAULT_READ_TIMEOUT,
    )
    .map_err(refresh_error)?;

    // `ok: false` with DRM off is not a failure: there is simply no ticket to mint.
    if !parsed.ok && parsed.drm_enabled {
        return Err(SdkError::ticket_invalid(
            "Arcane desktop could not issue an ownership ticket.",
        )
        .with_hint("Retry; if it persists, sign out and back in to Arcane desktop.")
        .with_context("public_key", public_key));
    }

    Ok(RefreshOutcome {
        drm_enabled: parsed.drm_enabled,
        user_id: parsed.user_id.or(health.user_id),
        game_id: parsed.game_id,
    })
}

fn refresh_error(call: DesktopCall) -> SdkError {
    match call.failure {
        CallFailure::Transport(detail) => {
            SdkError::arcane_unavailable(format!("The ownership refresh call failed: {detail}"))
                .with_hint("Make sure the Arcane Powered desktop app stays open, then retry.")
                .with_context("url", call.url)
        }
        CallFailure::Status {
            error: Some(body), ..
        } => map_desktop_error(&body),
        CallFailure::Status { code, body, .. } => SdkError::ticket_invalid(
            "Arcane desktop rejected the ownership refresh with an unreadable body.",
        )
        .with_hint("Update the Arcane Powered desktop app, then retry.")
        .with_context("url", call.url)
        .with_context("http_status", code)
        .with_context("body", truncate(&body, 200)),
        CallFailure::Decode { detail, body } => {
            SdkError::ticket_invalid(format!("Unexpected ownership refresh payload: {detail}"))
                .with_hint("The Arcane desktop app may be older than this SDK — update it.")
                .with_context("url", call.url)
                .with_context("body", truncate(&body, 200))
        }
    }
}

fn truncate(value: &str, max: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(max).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(error: &str, message: &str) -> SdkErrorBody {
        SdkErrorBody {
            error: error.into(),
            message: message.into(),
        }
    }

    #[test]
    fn maps_not_owned() {
        let err = map_desktop_error(&body(
            "not_owned",
            "This account does not own the requested game.",
        ));
        assert_eq!(err.code(), "not_owned");
        assert!(err.hint().unwrap().contains("Arcane Store"));
        assert!(!err.is_retryable());
    }

    #[test]
    fn maps_offline_to_network_required() {
        let err = map_desktop_error(&body("offline", "Cannot refresh while offline."));
        assert_eq!(err.code(), "network_required");
        assert!(err.is_retryable());
    }

    #[test]
    fn maps_unknown_achievement() {
        let err = map_desktop_error(&body("unknown_achievement", "no such key for this game"));
        assert_eq!(err.code(), "unknown_achievement");
        assert!(err.hint().unwrap().contains("Arcane portal"));
        assert!(!err.is_retryable());
    }

    #[test]
    fn maps_not_authenticated() {
        let err = map_desktop_error(&body("not_authenticated", "No session."));
        assert_eq!(err.code(), "not_authenticated");
    }

    #[test]
    fn maps_cloud_unreachable() {
        let err = map_desktop_error(&body("cloud_unreachable", "backend down"));
        assert_eq!(err.code(), "network_required");
    }

    #[test]
    fn unknown_desktop_error_keeps_the_original_code_in_context() {
        let err = map_desktop_error(&body("region_locked", "not available here"));
        assert_eq!(err.code(), "ticket_invalid");
        let context: Vec<_> = err
            .context()
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert!(context.contains(&("desktop_error", "region_locked")));
        assert!(context.contains(&("detail", "not available here")));
    }

    #[test]
    fn empty_message_falls_back_to_the_error_code() {
        let err = map_desktop_error(&body("not_owned", "   "));
        let detail = err
            .context()
            .iter()
            .find(|(k, _)| k == "detail")
            .map(|(_, v)| v.as_str());
        assert_eq!(detail, Some("not_owned"));
    }

    #[test]
    fn port_falls_back_to_the_default_when_unset_or_invalid() {
        // The env var is process-global; only assert the parse rules here.
        assert_eq!(DEFAULT_SDK_HTTP_PORT, 39284);
        assert!("0".parse::<u16>().is_ok());
        assert!("not-a-port".parse::<u16>().is_err());
    }

    #[test]
    fn truncate_keeps_short_values_intact() {
        assert_eq!(truncate("  hello  ", 10), "hello");
        assert_eq!(truncate("abcdefghij", 5), "abcde…");
    }
}
