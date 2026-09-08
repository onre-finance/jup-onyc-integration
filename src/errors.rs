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

    #[error("Zero price not allowed")]
    ZeroPriceNotAllowed,

    #[error("Zero price fix duration not allowed")]
    ZeroPriceFixDurationNotAllowed,

    #[error("Decimals exceed max {max}, was {was}")]
    DecimalsExceedMax { max: u8, was: u8 },

    #[error(transparent)]
    CalculationError(onre_pricing::PricingError),

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

    #[error("Unsupported token program: {0}")]
    UnsupportedTokenProgram(Pubkey),

    #[error("Invalid account owner for {account}: expected {expected}, got {actual}")]
    InvalidAccountOwner {
        account: Pubkey,
        expected: Pubkey,
        actual: Pubkey,
    },

    #[error("Invalid PDA: expected {expected}, got {actual}")]
    InvalidPda { expected: Pubkey, actual: Pubkey },

    #[error("Invalid quote return-data program: expected {expected}, got {actual}")]
    InvalidQuoteProgram { expected: Pubkey, actual: Pubkey },

    #[error("Invalid quote return-data length: expected {expected}, got {actual}")]
    InvalidQuoteLength { expected: usize, actual: usize },

    #[error("Quote does not match the requested offer, mints, or input amount")]
    QuoteMismatch,

    #[error("A positive input quote must have a non-zero minimum output")]
    InvalidMinimumOut,
}

/// Expected on-chain failure states of the v5 program.
///
/// These are deliberate protocol controls, not transient errors: integrators
/// should surface them as explicit venue/offer states and stop routing,
/// rather than retrying.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ExpectedFailure {
    /// Error 6024: global kill switch is on
    KillSwitchActivated,
    /// Error 6025: offer does not allow permissionless takes
    PermissionlessNotAllowed,
    /// Error 6112: the offer is disabled (`require_enabled`)
    OfferDisabled,
    /// Error 6113: the redemption offer is disabled
    RedemptionOfferDisabled,
}

/// Maps a v5 program custom error code to an expected, integrator-visible
/// state. Returns `None` for codes that should be treated as generic failures.
pub fn classify_program_error(code: u32) -> Option<ExpectedFailure> {
    match code {
        6024 => Some(ExpectedFailure::KillSwitchActivated),
        6025 => Some(ExpectedFailure::PermissionlessNotAllowed),
        6112 => Some(ExpectedFailure::OfferDisabled),
        6113 => Some(ExpectedFailure::RedemptionOfferDisabled),
        _ => None,
    }
}

impl From<onre_pricing::PricingError> for OnreError {
    fn from(e: onre_pricing::PricingError) -> Self {
        match e {
            onre_pricing::PricingError::MathOverflow => OnreError::MathOverflow,
            onre_pricing::PricingError::NoActiveVector => OnreError::NoActiveVector,
            onre_pricing::PricingError::ZeroPriceNotAllowed => OnreError::ZeroPriceNotAllowed,
            onre_pricing::PricingError::ZeroPriceFixDurationNotAllowed => {
                OnreError::ZeroPriceFixDurationNotAllowed
            }
            onre_pricing::PricingError::DecimalsExceedMax { max, was } => {
                OnreError::DecimalsExceedMax { max, was }
            }
            // Any other current or future non_exhaustive variant surfaces.
            _ => OnreError::CalculationError(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classifies_v5_emergency_error_codes() {
        assert_eq!(
            classify_program_error(6024),
            Some(ExpectedFailure::KillSwitchActivated)
        );
        assert_eq!(
            classify_program_error(6112),
            Some(ExpectedFailure::OfferDisabled)
        );
        assert_eq!(
            classify_program_error(6113),
            Some(ExpectedFailure::RedemptionOfferDisabled)
        );
        assert_eq!(
            classify_program_error(6025),
            Some(ExpectedFailure::PermissionlessNotAllowed)
        );
        assert_eq!(classify_program_error(1), None);
    }

    #[test]
    fn test_unmapped_pricing_error_surfaces_faithfully() {
        // Previously-unmapped variants must no longer collapse to MathOverflow.
        let err: OnreError = onre_pricing::PricingError::InsufficientLiquidity.into();
        assert!(matches!(err, OnreError::CalculationError(_)));
        assert_eq!(
            err.to_string(),
            "token out amount exceeds available liquidity"
        );
    }
}
