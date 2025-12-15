//! ## APR Scale
//! APR values use APR_SCALE (1,000,000) = 100%:
//! - 1% APR = 10,000
//! - 3.65% APR = 36,500  (used in OnRe tests)
//! - 5% APR = 50,000
//! - 36.5% APR = 365,000

use anyhow::Result;

use crate::constants::{APR_SCALE, PRICE_DECIMALS, SECONDS_IN_YEAR};
use crate::errors::OnreAmmError;
use crate::state::{Offer, OfferVector};

pub fn find_active_vector_at(offer: &Offer, time: u64) -> Result<OfferVector> {
    offer
        .vectors
        .iter()
        .filter(|v| v.start_time != 0 && v.start_time <= time)
        .max_by_key(|v| v.start_time)
        .copied()
        .ok_or_else(|| OnreAmmError::NoActiveVector.into())
}

/// Calculates the price for a specific time using discrete interval pricing
///
/// Formula:
///   interval = floor((time - base_time) / price_fix_duration)
///   step_end_time = (interval + 1) * price_fix_duration
///   price = base_price * (1 + apr * step_end_time / SECONDS_IN_YEAR)
pub fn calculate_step_price_at(
    apr: u64,
    base_price: u64,
    base_time: u64,
    price_fix_duration: u64,
    time: u64,
) -> Result<u64> {
    if base_time > time {
        return Err(OnreAmmError::NoActiveVector.into());
    }

    let elapsed_since_start = time.saturating_sub(base_time);
    let current_step = elapsed_since_start / price_fix_duration;

    let step_end_time = current_step
        .checked_add(1)
        .ok_or(OnreAmmError::MathOverflow)?
        .checked_mul(price_fix_duration)
        .ok_or(OnreAmmError::MathOverflow)?;

    calculate_vector_price(apr, base_price, step_end_time)
}

/// Calculates continuous price growth using APR-based linear interest
///
/// Formula: P(t) = P0 * (1 + apr * elapsed_time / SECONDS_IN_YEAR)
pub fn calculate_vector_price(apr: u64, base_price: u64, elapsed_time: u64) -> Result<u64> {
    let factor_den = APR_SCALE
        .checked_mul(SECONDS_IN_YEAR)
        .expect("APR_SCALE * SECONDS_IN_YEAR overflow");

    let y_part = (apr as u128)
        .checked_mul(elapsed_time as u128)
        .ok_or(OnreAmmError::MathOverflow)?;

    let factor_num = factor_den
        .checked_add(y_part)
        .ok_or(OnreAmmError::MathOverflow)?;

    let price_u128 = (base_price as u128)
        .checked_mul(factor_num)
        .ok_or(OnreAmmError::MathOverflow)?
        .checked_div(factor_den)
        .ok_or(OnreAmmError::MathOverflow)?;

    if price_u128 > u64::MAX as u128 {
        return Err(OnreAmmError::MathOverflow.into());
    }

    Ok(price_u128 as u64)
}

/// Calculates output token amount based on input amount and price
///
/// Formula:
///   token_out = (token_in_net * 10^(token_out_decimals + 9)) / (price * 10^token_in_decimals)
pub fn calculate_token_out_amount(
    token_in_amount: u64,
    price: u64,
    token_in_decimals: u8,
    token_out_decimals: u8,
) -> Result<u64> {
    let token_in_u128 = token_in_amount as u128;
    let price_u128 = price as u128;

    let numerator = token_in_u128
        .checked_mul(10_u128.pow((token_out_decimals + PRICE_DECIMALS) as u32))
        .ok_or(OnreAmmError::MathOverflow)?;

    let denominator = price_u128
        .checked_mul(10_u128.pow(token_in_decimals as u32))
        .ok_or(OnreAmmError::MathOverflow)?;

    let result = numerator
        .checked_div(denominator)
        .ok_or(OnreAmmError::MathOverflow)?;

    Ok(result as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_vector_price_no_time() {
        // At time=0, any APR gives base_price since no time has elapsed
        let price = calculate_vector_price(50_000, 1_000_000_000, 0).unwrap();
        assert_eq!(price, 1_000_000_000);
    }

    #[test]
    fn test_calculate_vector_price_one_year() {
        // APR scale: 1_000_000 = 100%, so 5% = 50_000
        // base_price = 1.0 (1_000_000_000), apr = 5% (50_000), time = 1 year
        let price = calculate_vector_price(50_000, 1_000_000_000, SECONDS_IN_YEAR as u64).unwrap();
        // Expected: 1.0 * (1 + 0.05 * 1) = 1.05 = 1_050_000_000
        assert_eq!(price, 1_050_000_000);
    }

    #[test]
    fn test_calculate_token_out_amount() {
        // 100 USDC (6 decimals) at price 1.0 (9 decimals) = 100 ONyc (9 decimals)
        let out = calculate_token_out_amount(
            100_000_000,   // 100 USDC
            1_000_000_000, // price = 1.0
            6,             // USDC decimals
            9,             // ONyc decimals
        )
        .unwrap();

        // Expected: (100_000_000 * 10^18) / (1_000_000_000 * 10^6) = 100 * 10^9
        assert_eq!(out, 100_000_000_000);
    }

    #[test]
    fn test_calculate_fee() {
        // 5% fee on 1000 = 50
        let amount: u64 = 1000;
        let fee_bps: u16 = 500;
        let fee = (amount as u128) * (fee_bps as u128) / 10000;
        assert_eq!(fee, 50);
    }

    #[test]
    fn test_onre_test_values() {
        // apr = 3.65% (36_500), duration = 1 day (86400s)
        // Price in first interval: 1.0 * (1 + 0.0365 * 86400 / 31536000) = 1.0001
        let price = calculate_vector_price(36_500, 1_000_000_000, 86400).unwrap();
        // Expected: 1.0001 (1_000_100_000 with 9 decimals)
        assert_eq!(price, 1_000_100_000);
    }

    #[test]
    fn test_365_percent_apr_one_year() {
        // apr = 365_000 (36.5%), after 1 year = 1.365 price
        let price = calculate_vector_price(365_000, 1_000_000_000, SECONDS_IN_YEAR as u64).unwrap();
        // Expected: 1.0 * (1 + 0.365) = 1.365 = 1_365_000_000
        assert_eq!(price, 1_365_000_000);
    }
}
