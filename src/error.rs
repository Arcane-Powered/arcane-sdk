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
            Self::Io(_) => "internal",
        }
    }
}
