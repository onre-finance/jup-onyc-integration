use core::fmt;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PricingError {
    MathOverflow,
    NoActiveVector,
    TimeBeforeBase,
    ZeroPriceNotAllowed,
    ZeroPriceFixDurationNotAllowed,
    ZeroLiquidity,
    DecimalsExceedMax { max: u8, was: u8 },
    InsufficientLiquidity,
    InvalidAmount,
    InvalidEpochDuration,
    InvalidCadenceConfig,
    InvalidCurveExponent,
    InvalidBasisPoints,
}

impl fmt::Display for PricingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PricingError::MathOverflow => write!(f, "math overflow"),
            PricingError::NoActiveVector => write!(f, "no active pricing vector"),
            PricingError::TimeBeforeBase => write!(f, "time is before vector base time"),
            PricingError::ZeroPriceNotAllowed => write!(f, "zero price not allowed"),
            PricingError::ZeroPriceFixDurationNotAllowed => {
                write!(f, "zero price fix duration not allowed")
            }
            PricingError::DecimalsExceedMax { max, was } => {
                write!(f, "decimals exceed max. Max: {}, was: {}", max, was)
            }
            PricingError::InvalidAmount => write!(f, "invalid amount"),
            PricingError::ZeroLiquidity => write!(f, "liquidity or reserve is zero"),
            PricingError::InsufficientLiquidity => {
                write!(f, "token out amount exceeds available liquidity")
            }
            PricingError::InvalidEpochDuration => write!(f, "epoch duration must be positive"),
            PricingError::InvalidCadenceConfig => write!(f, "invalid cadence configuration"),
            PricingError::InvalidCurveExponent => write!(f, "invalid curve exponent"),
            PricingError::InvalidBasisPoints => write!(f, "invalid basis points"),
        }
    }
}

impl core::error::Error for PricingError {}
