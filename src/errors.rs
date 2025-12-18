use solana_pubkey::Pubkey;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum OnreError {
    #[error("Permissionless not allowed for this offer")]
    PermissionlessNotAllowed,

    #[error("No account found for pubkey: {0}")]
    NoAccountFound(Pubkey),

    #[error("Failed to fetch multiple accounts")]
    FailedToFetchMultipleAccounts,

    #[error("Failed to deserialize account data: {0}")]
    DeserializationFailed(Pubkey),

    #[error("Invalid mint: {0}")]
    InvalidMint(Pubkey),

    #[error("State account data is missing")]
    StateMissing,

    #[error("No active pricing vector")]
    NoActiveVector,

    #[error("Math overflow")]
    MathOverflow,

    #[error("Exact Out swap type is not supported")]
    ExactOutNotSupported,

    #[error("Token info does not extend to index {0}")]
    TokenInfoIndexError(usize),

    #[error("Failed to get system time")]
    TimeError,

    #[error("Kill switch is activated")]
    KillSwitchActivated,

    #[error("Venue not initialized - call update_state first")]
    NotInitialized,
}
