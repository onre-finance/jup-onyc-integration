use thiserror::Error;

#[derive(Error, Debug)]
pub enum OnreAmmError {
    #[error("Permissionless not allowed for this offer")]
    PermissionlessNotAllowed,

    #[error("Invalid swap direction - only token_in → ONyc is supported")]
    InvalidDirection,

    #[error("Kill switch is activated")]
    KillSwitchActivated,

    #[error("State account data is missing")]
    StateMissing,

    #[error("No active pricing vector")]
    NoActiveVector,

    #[error("Math overflow")]
    MathOverflow,

    #[error("Failed to deserialize account: {0}")]
    DeserializationError(String),

    #[error("Invalid mint account data")]
    InvalidMintData,

    #[error("Failed to get system time")]
    TimeError,

    #[error("Max supply exceeded: minting {requested} would exceed max supply of {max_supply} (current: {current_supply})")]
    MaxSupplyExceeded {
        requested: u64,
        current_supply: u64,
        max_supply: u64,
    },
}
