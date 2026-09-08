use crate::{
    calculate_step_price_at, deduct_fee, PriceVector, PricingError, MAX_TOKEN_DECIMALS,
    PRICE_DECIMALS,
};
use core::cmp::max;

/// Shortcut for calling [`calculate_step_price_at`] and [`calculate_token_out_amount`].
pub fn calculate_amount_out(
    vector: &PriceVector,
    time: u64,
    amount_in: u64,
    fee_bps: u16,
    in_decimals: u8,
    out_decimals: u8,
) -> Result<u64, PricingError> {
    let price = calculate_step_price_at(
        vector.apr,
        vector.base_price,
        vector.base_time,
        vector.price_fix_duration,
        time,
    )?;

    calculate_token_out_amount(amount_in, price, fee_bps, in_decimals, out_decimals)
}

/// Converts a token_in amount to token_out using offer pricing where `price` is token_in per ONyc.
///
/// The fee is ceiling-rounded in favor of the protocol.
///
/// # Arguments
/// * `price` - Price with 9 decimal precision
pub fn calculate_token_out_amount(
    amount_in: u64,
    price: u64,
    fee_bps: u16,
    in_decimals: u8,
    out_decimals: u8,
) -> Result<u64, PricingError> {
    if price == 0 {
        return Err(PricingError::ZeroPriceNotAllowed);
    }

    if in_decimals > MAX_TOKEN_DECIMALS || out_decimals > MAX_TOKEN_DECIMALS {
        return Err(PricingError::DecimalsExceedMax {
            max: MAX_TOKEN_DECIMALS,
            was: max(in_decimals, out_decimals),
        });
    }

    let amount_in_net = deduct_fee(amount_in, fee_bps)?;

    if amount_in_net == 0 {
        return Ok(0);
    }

    let numerator = (amount_in_net as u128)
        .checked_mul(10_u128.pow((out_decimals + PRICE_DECIMALS) as u32))
        .ok_or(PricingError::MathOverflow)?;

    // Safe: u64 * 10^(≤18) ≤ 1.8e37 < u128::MAX
    let denominator = price as u128 * 10_u128.pow(in_decimals as u32);

    let result = numerator / denominator;

    result.try_into().map_err(|_| PricingError::MathOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SECONDS_IN_DAY;

    // --- calculate_token_out_amount ---

    #[test]
    fn token_out_with_price_floors() {
        let out = calculate_token_out_amount(
            100_000_000,   // 100 USDC
            1_500_000_000, // price = 1.5
            0,
            6, // USDC decimals
            9, // ONyc decimals
        )
        .unwrap();

        // 100 / 1.5 = 66.666... floored (in native)
        assert_eq!(out, 66_666_666_666);
    }

    #[test]
    fn token_out_with_price_below_one() {
        let out = calculate_token_out_amount(
            100_000_000, // 100 USDC
            500_000_000, // price = 0.5
            0,
            6,
            9,
        )
        .unwrap();

        // 100 / 0.5 -> 200 ONyc
        assert_eq!(out, 200_000_000_000);
    }

    #[test]
    fn token_out_with_equal_decimals() {
        let out = calculate_token_out_amount(
            100_000_000_000, // 100 tokens
            1_000_000_000,   // price = 1.0
            0,
            9,
            9,
        )
        .unwrap();

        // 100 / 1 = 100 (in native)
        assert_eq!(out, 100_000_000_000);
    }

    #[test]
    fn token_out_with_fewer_out_decimals() {
        let out = calculate_token_out_amount(
            100_000_000_000, // 100 tokens
            1_000_000_000,   // price = 1.0
            0,
            9, // in decimals
            6, // out decimals
        )
        .unwrap();

        // 100 / 1 = 100 (in native)
        assert_eq!(out, 100_000_000);
    }

    #[test]
    fn token_out_zero_input_returns_zero() {
        let out = calculate_token_out_amount(0, 1_000_000_000, 0, 6, 9).unwrap();

        assert_eq!(out, 0);
    }

    #[test]
    fn token_out_rejects_zero_price() {
        let result = calculate_token_out_amount(100_000_000, 0, 0, 6, 9);

        assert_eq!(result, Err(PricingError::ZeroPriceNotAllowed));
    }

    #[test]
    fn token_out_rejects_decimals_above_max() {
        let result = calculate_token_out_amount(100, 1_000_000_000, 0, 19, 9);
        assert_eq!(
            result,
            Err(PricingError::DecimalsExceedMax { max: 18, was: 19 })
        );

        let result = calculate_token_out_amount(100, 1_000_000_000, 0, 9, 20);
        assert_eq!(
            result,
            Err(PricingError::DecimalsExceedMax { max: 18, was: 20 })
        );
    }

    #[test]
    fn token_out_numerator_overflow() {
        let result = calculate_token_out_amount(u64::MAX, 1_000_000_000, 0, 0, 18);
        assert_eq!(result, Err(PricingError::MathOverflow));
    }

    #[test]
    fn token_out_with_max_decimals() {
        let out = calculate_token_out_amount(
            100_000_000_000, // small amount at 18 decimals
            2_000_000_000,   // price = 2.0
            0,
            18,
            18,
        )
        .unwrap();

        assert_eq!(out, 50_000_000_000);
    }

    #[test]
    fn token_out_full_fee_returns_zero() {
        let out = calculate_token_out_amount(
            100,
            1_000_000_000, // price = 1.0
            10_000,        // 100% fee
            6,
            9,
        )
        .unwrap();

        assert_eq!(out, 0);
    }

    // --- calculate_amount_out ---

    #[test]
    fn calculate_amount_out_end_to_end() {
        let vector = PriceVector {
            start_time: 1_000,
            base_time: 1_000,
            base_price: 1_000_000_000, // 1.0
            apr: 36_500,               // 3.65 %
            price_fix_duration: SECONDS_IN_DAY,
        };

        let out = calculate_amount_out(
            &vector,
            1_000 + 100 * SECONDS_IN_DAY, // base + 100 days
            100_000_000,                  // 100 USDC
            100,                          // 1% fee
            6,                            // USDC decimals
            9,                            // ONyc decimals
        )
        .unwrap();

        let out_1 = calculate_amount_out(
            &vector,
            1_000 + 101 * SECONDS_IN_DAY, // base + 101 days
            100_000_000,                  // 100 USDC
            100,                          // 1% fee
            6,                            // USDC decimals
            9,                            // ONyc decimals
        )
        .unwrap();

        assert!(out_1 < out, "output must decrease as price grows");

        // 1% fee -> 99 USDC net in

        // price_0 = 1_010_150_667
        // 99 / 1.010150667 = ~98,005182032 (in native)
        assert_eq!(out, 98_005_182_032);

        // price_1 = 1010251682
        // 99 / 1.010251682 = ~97,995382501 (in native)
        assert_eq!(out_1, 97_995_382_501);
    }
}
