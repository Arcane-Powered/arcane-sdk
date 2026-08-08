use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnershipStatus {
    Owned,
    DrmDisabled,
}

#[derive(Debug, Error)]
pub enum SdkError {
    #[error("ticket_missing: {0}")]
    TicketMissing(String),
    #[error("ticket_expired: {0}")]
    TicketExpired(String),
    #[error("ticket_invalid: {0}")]
    TicketInvalid(String),
    #[error("device_mismatch: {0}")]
    DeviceMismatch(String),
    #[error("clock_rollback: {0}")]
    ClockRollback(String),
    /// Cloud confirmed the signed-in account does not own this title.
    #[error("not_owned: {0}")]
    NotOwned(String),
    /// No usable offline ticket and Arcane/cloud cannot refresh while offline.
    #[error("network_required: {0}")]
    NetworkRequired(String),
    /// Arcane desktop is reachable but the user is not signed in.
    #[error("not_authenticated: {0}")]
    NotAuthenticated(String),
    /// Could not reach or launch the Arcane desktop loopback server.
    #[error("arcane_unavailable: {0}")]
    ArcaneUnavailable(String),
    #[error("io: {0}")]
    Io(String),
}

impl SdkError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::TicketMissing(_) => "ticket_missing",
            Self::TicketExpired(_) => "ticket_expired",
            Self::TicketInvalid(_) => "ticket_invalid",
            Self::DeviceMismatch(_) => "device_mismatch",
            Self::ClockRollback(_) => "clock_rollback",
            Self::NotOwned(_) => "not_owned",
            Self::NetworkRequired(_) => "network_required",
            Self::NotAuthenticated(_) => "not_authenticated",
            Self::ArcaneUnavailable(_) => "arcane_unavailable",
            Self::Io(_) => "internal",
        }
    }

    /// Whether init should attempt a desktop ownership refresh for this error.
    pub(crate) fn should_refresh_via_desktop(&self) -> bool {
        matches!(self, Self::TicketMissing(_) | Self::TicketExpired(_))
    }
}
