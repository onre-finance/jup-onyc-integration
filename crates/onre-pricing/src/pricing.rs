use crate::math_utils::{ceil_div_u128, mul_div_round, pow_fixed};
use crate::{
    constants::{APR_SCALE, MAX_BASIS_POINTS},
    error::PricingError,
    types::PriceVector,
    SECONDS_IN_DAY,
};

// 1e18
const SCALE: u128 = 1000000000000000000;

/// Finds the currently active pricing vector at a specific time
///
/// Searches through the pricing vectors to find the one that should be
/// active at the given time. Returns the vector with the latest start_time that
/// is still before or equal to the specified time.
///
/// # Arguments
/// * `vectors` - Pricing vectors to search
/// * `time` - Unix timestamp to check for active vector
///
/// # Returns
/// * `Ok(PriceVector)` - The active pricing vector at the specified time
/// * `Err(PricingError::NoActiveVector)` - If no vector is active at that time
pub fn find_active_vector_at(
    vectors: &[PriceVector],
    time: u64,
) -> Result<&PriceVector, PricingError> {
    vectors
        .iter()
        .filter(|vector| vector.start_time != 0 && vector.start_time <= time)
        .max_by_key(|vector| vector.start_time)
        .ok_or(PricingError::NoActiveVector)
}

/// Calculates the price for a specific time using discrete interval pricing
///
/// Snaps `time` to the end of its current interval, then delegates to
/// [`calculate_vector_price`] for daily-compounded pricing at that point.
///
/// Formula:
///   interval = floor((time - base_time) / price_fix_duration)
///   step_end_time = (interval + 1) * price_fix_duration
///   price = calculate_vector_price(apr, base_price, step_end_time)
pub fn calculate_step_price_at(
    apr: u64,
    base_price: u64,
    base_time: u64,
    price_fix_duration: u64,
    time: u64,
) -> Result<u64, PricingError> {
    if base_time > time {
        return Err(PricingError::TimeBeforeBase);
    }
    if price_fix_duration == 0 {
        return Err(PricingError::ZeroPriceFixDurationNotAllowed);
    }

    let elapsed_since_start = time - base_time;
    let current_step = elapsed_since_start / price_fix_duration;

    let step_end_time = current_step
        .checked_add(1)
        .ok_or(PricingError::MathOverflow)?
        .checked_mul(price_fix_duration)
        .ok_or(PricingError::MathOverflow)?;

    calculate_vector_price(apr, base_price, step_end_time)
}

/// Calculates price growth using daily compounding with linear intraday interpolation
///
/// The APR is compounded daily.
/// For elapsed times that are not an exact number of days,
/// the remaining fractional day is linearly interpolated between the current
/// daily-compounded price and the next daily-compounded price.
///
/// Formula:
///   daily_factor = 1 + apr / (APR_SCALE * 365)
///   full_day_price = base_price * daily_factor^(elapsed_seconds / 86400)
///   price = full_day_price + daily_delta * (elapsed_seconds % 86400) / 86400
///
/// # Arguments
/// * `apr` - Annual Percentage Rate with scale=6 (10_000 = 1%, 1_000_000 = 100%)
/// * `base_price` - Starting price with scale=9
/// * `elapsed_seconds` - Time elapsed since base_time in seconds
fn calculate_vector_price(
    apr: u64,
    base_price: u64,
    elapsed_seconds: u64,
) -> Result<u64, PricingError> {
    let full_days = elapsed_seconds / SECONDS_IN_DAY;
    let remaining_seconds = elapsed_seconds % SECONDS_IN_DAY;

    // 1 + (r * n)
    let daily_factor = SCALE + (apr as u128 * SCALE / (365 * APR_SCALE));

    // (base^n)
    let compound_factor =
        pow_fixed(daily_factor, full_days, SCALE).ok_or(PricingError::MathOverflow)?;
    let full_day_price = mul_div_round(base_price as u128, compound_factor, SCALE)
        .ok_or(PricingError::MathOverflow)?;

    if remaining_seconds == 0 {
        return full_day_price
            .try_into()
            .map_err(|_| PricingError::MathOverflow);
    }

    let next_day_price =
        mul_div_round(full_day_price, daily_factor, SCALE).ok_or(PricingError::MathOverflow)?;

    let daily_delta = next_day_price - full_day_price;

    let partial_day_delta = mul_div_round(
        daily_delta,
        remaining_seconds as u128,
        SECONDS_IN_DAY as u128,
    )
    .ok_or(PricingError::MathOverflow)?;

    let price = full_day_price
        .checked_add(partial_day_delta)
        .ok_or(PricingError::MathOverflow)?;

    price.try_into().map_err(|_| PricingError::MathOverflow)
}

/// Computes the fee for a given token amount and fee rate in basis points.
///
/// The fee is ceiling-rounded in favor of the protocol: any fractional fee
/// rounds up to the next whole token.
///
/// Returns `Err(InvalidBasisPoints)` if `fee_basis_points` > `MAX_BASIS_POINTS`
///
/// # Example
/// ```text
/// // 5% fee on 1000 tokens → 50
/// let fee = calculate_fee(1000, 500)?;
/// assert_eq!(fee, 50);
/// ```
pub fn calculate_fee(token_amount: u64, fee_basis_points: u16) -> Result<u64, PricingError> {
    if fee_basis_points > MAX_BASIS_POINTS as u16 {
        return Err(PricingError::InvalidBasisPoints);
    }

    let fee_numerator = token_amount as u128 * fee_basis_points as u128;

    let fee_amount: u64 = ceil_div_u128(fee_numerator, MAX_BASIS_POINTS)
        .ok_or(PricingError::MathOverflow)?
        .try_into()
        .map_err(|_| PricingError::MathOverflow)?;

    Ok(fee_amount)
}

/// Deducts a [`calculate_fee`] fee and returns the net amount.
///
/// # Example
/// ```text
/// // 5% fee on 1000 tokens → 50 fee, 950 remaining
/// let net = deduct_fee(1000, 500)?;
/// assert_eq!(net, 950);
/// ```
pub fn deduct_fee(token_amount: u64, fee_basis_points: u16) -> Result<u64, PricingError> {
    let fee_amount = calculate_fee(token_amount, fee_basis_points)?;

    // Safe: fee_basis_points ≤ 10_000 is validated, so ceil(amount * bp / 10_000) ≤ amount
    Ok(token_amount - fee_amount)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_price_no_time_returns_base() {
        let price = calculate_vector_price(50_000, 1_000_000_000, 0).unwrap();
        assert_eq!(price, 1_000_000_000);
    }

    #[test]
    fn vector_price_half_year_growth() {
        let price =
            calculate_vector_price(127_000, 1_085_708_975, SECONDS_IN_DAY * (365 / 2)).unwrap();

        assert_eq!(price, 1_156_673_508);
    }

    #[test]
    fn vector_price_matches_daily_compounding_on_day_boundaries() {
        let price = calculate_vector_price(97_600, 1_000_000_000, SECONDS_IN_DAY).unwrap();
        assert_eq!(price, 1_000_267_397);

        let price = calculate_vector_price(97_600, 1_000_000_000, SECONDS_IN_DAY * 2).unwrap();
        assert_eq!(price, 1_000_534_866);
    }

    #[test]
    fn vector_price_grows_with_subday_elapsed_time() {
        let one_hour = calculate_vector_price(97_600, 1_000_000_000, 3_600).unwrap();
        let six_hours = calculate_vector_price(97_600, 1_000_000_000, 21_600).unwrap();
        let one_day = calculate_vector_price(97_600, 1_000_000_000, SECONDS_IN_DAY).unwrap();

        assert_eq!(one_hour, 1_000_011_142);
        assert_eq!(six_hours, 1_000_066_849);
        assert_eq!(one_day, 1_000_267_397);
    }

    #[test]
    fn vector_price_matches_multi_day_compounding() {
        let three_days = calculate_vector_price(97_600, 1_000_000_000, SECONDS_IN_DAY * 3).unwrap();
        let three_days_six_hours =
            calculate_vector_price(97_600, 1_000_000_000, SECONDS_IN_DAY * 3 + 21_600).unwrap();

        assert_eq!(three_days, 1_000_802_406);
        assert_eq!(three_days_six_hours, 1_000_869_309);
    }

    #[test]
    fn vector_price_rejects_price_overflow() {
        let result = calculate_vector_price(u64::MAX, u64::MAX, SECONDS_IN_DAY);

        assert!(result.is_err());
    }

    #[test]
    fn step_price_rejects_time_before_base_time() {
        let result = calculate_step_price_at(0, 1_000_000_000, 2, SECONDS_IN_DAY, 1);

        assert!(result.is_err());
    }

    #[test]
    fn step_price_rejects_step_end_overflow() {
        let result = calculate_step_price_at(0, 1_000_000_000, 0, 1, u64::MAX);

        assert!(result.is_err());
    }

    #[test]
    fn vector_price_365_percent_apr_one_year() {
        let seconds_in_year = SECONDS_IN_DAY * 365;

        let price = calculate_vector_price(365_000, 1_000_000_000, seconds_in_year).unwrap();
        // 36.5% APR compounded daily for 1 year ≈ 44.03% effective yield
        assert_eq!(price, 1_440_251_313);
    }

    // ── calculate_step_price_at ─────────────────────────────────────

    #[test]
    fn step_price_is_constant_within_a_step() {
        let base_time = 1_000;
        let duration = SECONDS_IN_DAY;
        let apr = 36_500;
        let base_price = 1_000_000_000;

        let at_start =
            calculate_step_price_at(apr, base_price, base_time, duration, base_time).unwrap();
        let at_end = calculate_step_price_at(
            apr,
            base_price,
            base_time,
            duration,
            base_time + duration - 1,
        )
        .unwrap();

        assert_eq!(at_start, at_end);
        assert_eq!(at_start, 1_000_100_000);
    }

    #[test]
    fn step_price_jumps_at_boundary() {
        let base_time = 1_000;
        let duration = SECONDS_IN_DAY;
        let apr = 36_500;
        let base_price = 1_000_000_000;

        let step_0 =
            calculate_step_price_at(apr, base_price, base_time, duration, base_time).unwrap();
        let step_1 =
            calculate_step_price_at(apr, base_price, base_time, duration, base_time + duration)
                .unwrap();

        assert!(step_1 > step_0, "price must increase at step boundary");
        assert_eq!(step_0, 1_000_100_000);
        assert_eq!(step_1, 1_000_200_010);
    }

    #[test]
    fn step_price_rejects_zero_duration() {
        let result = calculate_step_price_at(36_500, 1_000_000_000, 0, 0, 0);
        assert_eq!(result, Err(PricingError::ZeroPriceFixDurationNotAllowed));
    }

    #[test]
    fn step_price_at_base_time_equals_one_step_growth() {
        let price =
            calculate_step_price_at(36_500, 1_000_000_000, 1_000, SECONDS_IN_DAY, 1_000).unwrap();
        // At time == base_time, step 0 → step_end = 1 * duration
        // Price is one step of growth, never the raw base_price
        assert!(price > 1_000_000_000);
        assert_eq!(price, 1_000_100_000);
    }

    // ── calculate_fee ────────────────────────────────────────────────

    #[test]
    fn calculate_fee_basic() {
        assert_eq!(calculate_fee(1_000, 500).unwrap(), 50);
    }

    #[test]
    fn calculate_fee_zero_bp() {
        assert_eq!(calculate_fee(1_000, 0).unwrap(), 0);
    }

    #[test]
    fn calculate_fee_max_bp() {
        assert_eq!(calculate_fee(1_000, 10_000).unwrap(), 1_000);
    }

    #[test]
    fn calculate_fee_invalid_bp() {
        assert_eq!(
            calculate_fee(1_000, 10_001),
            Err(PricingError::InvalidBasisPoints)
        );
    }

    #[test]
    fn calculate_fee_ceiling_rounds_up() {
        // 1001 * 500 / 10000 = 50.05 → ceil = 51
        assert_eq!(calculate_fee(1_001, 500).unwrap(), 51);
    }

    #[test]
    fn calculate_fee_minimum_amount() {
        // 1 * 1 / 10000 = 0.0001 → ceil = 1
        assert_eq!(calculate_fee(1, 1).unwrap(), 1);
    }

    // ── deduct_fee ──────────────────────────────────────────────────

    #[test]
    fn fee_zero_returns_full_amount() {
        assert_eq!(deduct_fee(1_000, 0).unwrap(), 1_000);
    }

    #[test]
    fn fee_full_returns_zero() {
        assert_eq!(deduct_fee(1_000, 10_000).unwrap(), 0);
    }

    #[test]
    fn fee_basis_points_above_max_returns_error() {
        let result = deduct_fee(1_000, 10_001);
        assert_eq!(result, Err(PricingError::InvalidBasisPoints));
    }

    #[test]
    fn fee_ceiling_rounds_up_in_favor_of_protocol() {
        // 1001 * 500 / 10000 = 50.05 → ceil = 51, net = 950
        let net = deduct_fee(1_001, 500).unwrap();
        assert_eq!(net, 950);

        let actual_fee = 1_001 - net;
        assert_eq!(actual_fee, 51);

        // Floor division would give fee=50, net=951 — ceiling costs user 1 extra
        let floor_fee = 1_001u64 * 500 / 10_000;
        assert_eq!(floor_fee, 50);
        assert!(actual_fee > floor_fee);
    }

    #[test]
    fn fee_on_minimum_amount_rounds_up_to_full_token() {
        // 1 token at 1bp: 1*1/10000 = 0.0001 → ceil = 1 → net = 0
        let net = deduct_fee(1, 1).unwrap();
        assert_eq!(net, 0);
    }

    // ── find_active_vector_at ───────────────────────────────────────

    #[test]
    fn find_vector_single_active() {
        let vectors = [PriceVector {
            start_time: 100,
            base_time: 100,
            base_price: 1_000_000_000,
            apr: 50_000,
            price_fix_duration: 86400,
        }];

        let v = find_active_vector_at(&vectors, 100).unwrap();
        assert_eq!(v.start_time, 100);
    }

    #[test]
    fn find_vector_exact_start_time_boundary() {
        let vectors = [
            PriceVector {
                start_time: 100,
                base_time: 100,
                base_price: 1_000_000_000,
                apr: 50_000,
                price_fix_duration: 86400,
            },
            PriceVector {
                start_time: 200,
                base_time: 200,
                base_price: 1_050_000_000,
                apr: 60_000,
                price_fix_duration: 86400,
            },
        ];

        // time == start_time of second vector: should select it (filter is <=)
        let v = find_active_vector_at(&vectors, 200).unwrap();
        assert_eq!(v.start_time, 200);
    }

    #[test]
    fn find_vector_all_empty_returns_error() {
        let vectors = [PriceVector::default(); 3];
        assert_eq!(
            find_active_vector_at(&vectors, 100),
            Err(PricingError::NoActiveVector)
        );
    }

    // ── calculate_vector_price ──────────────────────────────────────

    #[test]
    fn vector_price_zero_apr_never_changes() {
        let base = 1_000_000_000;

        let at_one_day = calculate_vector_price(0, base, SECONDS_IN_DAY).unwrap();
        assert_eq!(at_one_day, base);

        let at_half_day = calculate_vector_price(0, base, SECONDS_IN_DAY / 2).unwrap();
        assert_eq!(at_half_day, base);
    }
}
