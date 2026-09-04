pub const MAX_TOKEN_DECIMALS: u8 = 18;
/// Fixed-point decimal precision for all prices in this crate.
pub const PRICE_DECIMALS: u8 = 9;

/// 10_000 basis points = 100%.
pub const MAX_BASIS_POINTS: u128 = 10000;

pub const SECONDS_IN_DAY: u64 = 86_400;

/// APR scale factor: 1_000_000 = 100%, so 1% = 10,000, 5% = 50,000
pub const APR_SCALE: u128 = 1_000_000;

/// Maximum number of offer vectors in a pricing schedule.
pub const MAX_VECTORS: usize = 10;
