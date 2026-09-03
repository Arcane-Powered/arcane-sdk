//! Structured SDK errors.
//!
//! Every failure carries a stable machine-readable [`ErrorCode`], a player-facing
//! `message`, a developer-facing `hint` (what to actually do about it), and a
//! `context` key/value list naming the values that were compared or the paths
//! that were read. The goal is that a single log line is enough to debug.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorCode {
    /// No ticket cache, or the cached ticket is empty while DRM is on.
    TicketMissing,
    /// The ticket JWT is past `exp` (beyond clock skew).
    TicketExpired,
    /// Bad JWT, missing/invalid JWKS, wrong title, or `own=false`.
    TicketInvalid,
    /// The ticket is bound to a different machine.
    DeviceMismatch,
    /// The system clock is earlier than the ticket or the last-seen time.
    ClockRollback,
    /// Cloud confirmed the signed-in account does not own this title.
    NotOwned,
    /// No usable offline ticket, and a refresh needs the network.
    NetworkRequired,
    /// Arcane desktop is reachable but nobody is signed in.
    NotAuthenticated,
    /// Could not reach or launch the Arcane desktop loopback server.
    ArcaneUnavailable,
    /// The Arcane desktop app does not know this route yet — it predates the
    /// feature the SDK asked for.
    FeatureUnavailable,
    /// The public key argument is empty, oversized, or has forbidden characters.
    InvalidPublicKey,
    /// Several accounts hold a ticket for this title and Arcane has not recorded
    /// which one is signed in.
    AmbiguousSession,
    /// A client accessor was used before a successful init (C ABI only).
    NotInitialized,
    /// Filesystem / path failure.
    Internal,
}

impl ErrorCode {
    /// Stable wire string. This is the public contract — never change these.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TicketMissing => "ticket_missing",
            Self::TicketExpired => "ticket_expired",
            Self::TicketInvalid => "ticket_invalid",
            Self::DeviceMismatch => "device_mismatch",
            Self::ClockRollback => "clock_rollback",
            Self::NotOwned => "not_owned",
            Self::NetworkRequired => "network_required",
            Self::NotAuthenticated => "not_authenticated",
            Self::ArcaneUnavailable => "arcane_unavailable",
            Self::FeatureUnavailable => "feature_unavailable",
            Self::InvalidPublicKey => "invalid_public_key",
            Self::AmbiguousSession => "ambiguous_session",
            Self::NotInitialized => "not_initialized",
            Self::Internal => "internal",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnershipStatus {
    Owned,
    DrmDisabled,
}

impl OwnershipStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Owned => "owned",
            Self::DrmDisabled => "drm_disabled",
        }
    }
}

impl fmt::Display for OwnershipStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single SDK failure.
///
/// Match on [`SdkError::code`] — it is the stable contract. `message` is safe to
/// show a player; `hint` and `context` are for the developer's logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkError {
    code: ErrorCode,
    message: String,
    hint: Option<String>,
    context: Vec<(String, String)>,
}

impl SdkError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
            context: Vec::new(),
        }
    }

    /// Attach the developer-facing "what to do about it" line.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Attach a named value that was compared, read, or resolved.
    #[must_use]
    pub fn with_context(mut self, key: impl Into<String>, value: impl fmt::Display) -> Self {
        self.context.push((key.into(), value.to_string()));
        self
    }

    /// Stable machine-readable code, e.g. `"not_owned"`.
    pub fn code(&self) -> &'static str {
        self.code.as_str()
    }

    /// Typed form of [`SdkError::code`], for exhaustive matching in Rust.
    pub fn error_code(&self) -> ErrorCode {
        self.code
    }

    /// Player-facing explanation, without developer detail.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Developer-facing next step, when one exists.
    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }

    /// Named values involved in the failure (compared ids, resolved paths, ports).
    pub fn context(&self) -> &[(String, String)] {
        &self.context
    }

    /// Whether retrying the same call could succeed once an external condition
    /// changes (network comes back, Arcane opens, the player signs in) without
    /// the developer changing any code.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self.code,
            ErrorCode::TicketMissing
                | ErrorCode::TicketExpired
                | ErrorCode::NetworkRequired
                | ErrorCode::NotAuthenticated
                | ErrorCode::ArcaneUnavailable
        )
    }

    /// Machine-readable rendering for engines that want to surface full detail.
    pub fn to_json(&self) -> String {
        let context: serde_json::Map<String, serde_json::Value> = self
            .context
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();
        let value = serde_json::json!({
            "code": self.code.as_str(),
            "message": self.message,
            "hint": self.hint,
            "retryable": self.is_retryable(),
            "context": context,
        });
        value.to_string()
    }

    /// Whether init should attempt a desktop ownership refresh for this error.
    pub(crate) fn should_refresh_via_desktop(&self) -> bool {
        matches!(
            self.code,
            ErrorCode::TicketMissing | ErrorCode::TicketExpired
        )
    }

    pub(crate) fn ticket_missing(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::TicketMissing, message)
    }

    pub(crate) fn ticket_expired(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::TicketExpired, message)
    }

    pub(crate) fn ticket_invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::TicketInvalid, message)
    }

    pub(crate) fn device_mismatch(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::DeviceMismatch, message)
    }

    pub(crate) fn clock_rollback(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ClockRollback, message)
    }

    pub(crate) fn not_owned(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotOwned, message)
    }

    pub(crate) fn network_required(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NetworkRequired, message)
    }

    pub(crate) fn not_authenticated(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotAuthenticated, message)
    }

    pub(crate) fn arcane_unavailable(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ArcaneUnavailable, message)
    }

    pub(crate) fn feature_unavailable(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::FeatureUnavailable, message)
    }

    pub(crate) fn invalid_public_key(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidPublicKey, message)
    }

    pub(crate) fn ambiguous_session(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::AmbiguousSession, message)
    }

    pub(crate) fn not_initialized(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotInitialized, message)
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }
}

impl fmt::Display for SdkError {
    /// `code: message — hint (key=value, key=value)`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, " — {hint}")?;
        }
        if !self.context.is_empty() {
            f.write_str(" (")?;
            for (i, (key, value)) in self.context.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{key}={value}")?;
            }
            f.write_str(")")?;
        }
        Ok(())
    }
}

impl std::error::Error for SdkError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_includes_code_message_hint_and_context() {
        let err = SdkError::ticket_invalid("Ticket was issued for a different title.")
            .with_hint("Check the public key in your build.")
            .with_context("expected", "pk_abc")
            .with_context("ticket_gid", "pk_xyz");

        assert_eq!(
            err.to_string(),
            "ticket_invalid: Ticket was issued for a different title. \
             — Check the public key in your build. (expected=pk_abc, ticket_gid=pk_xyz)"
        );
    }

    #[test]
    fn display_without_hint_or_context_is_just_code_and_message() {
        let err = SdkError::not_owned("You do not own this game.");
        assert_eq!(err.to_string(), "not_owned: You do not own this game.");
    }

    #[test]
    fn json_carries_every_field() {
        let err = SdkError::network_required("Connect to the internet once.")
            .with_hint("Open Arcane while online.")
            .with_context("port", 39284);

        let parsed: serde_json::Value = serde_json::from_str(&err.to_json()).unwrap();
        assert_eq!(parsed["code"], "network_required");
        assert_eq!(parsed["message"], "Connect to the internet once.");
        assert_eq!(parsed["hint"], "Open Arcane while online.");
        assert_eq!(parsed["retryable"], true);
        assert_eq!(parsed["context"]["port"], "39284");
    }

    #[test]
    fn json_hint_is_null_when_absent() {
        let parsed: serde_json::Value =
            serde_json::from_str(&SdkError::internal("boom").to_json()).unwrap();
        assert_eq!(parsed["hint"], serde_json::Value::Null);
        assert_eq!(parsed["retryable"], false);
    }

    #[test]
    fn only_externally_resolvable_failures_are_retryable() {
        assert!(SdkError::network_required("x").is_retryable());
        assert!(SdkError::arcane_unavailable("x").is_retryable());
        assert!(SdkError::not_authenticated("x").is_retryable());
        assert!(!SdkError::not_owned("x").is_retryable());
        assert!(!SdkError::device_mismatch("x").is_retryable());
        assert!(!SdkError::invalid_public_key("x").is_retryable());
        assert!(!SdkError::feature_unavailable("x").is_retryable());
    }

    #[test]
    fn feature_unavailable_is_a_stable_code() {
        assert_eq!(
            SdkError::feature_unavailable("x").code(),
            "feature_unavailable"
        );
    }

    #[test]
    fn only_ticket_gaps_trigger_a_desktop_refresh() {
        assert!(SdkError::ticket_missing("x").should_refresh_via_desktop());
        assert!(SdkError::ticket_expired("x").should_refresh_via_desktop());
        assert!(!SdkError::ticket_invalid("x").should_refresh_via_desktop());
        assert!(!SdkError::not_owned("x").should_refresh_via_desktop());
    }
}
