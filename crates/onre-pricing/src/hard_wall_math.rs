use crate::{PricingError, MAX_BASIS_POINTS};
use core::num::{NonZeroU128, NonZeroU64};

const WALL_SENSITIVITY_SCALE: u128 = 10_000;

pub const HARD_WALL_SCALE_NZ: NonZeroU64 = NonZeroU64::new(1_000_000_000_000).unwrap();
pub const HARD_WALL_SCALE: u128 = HARD_WALL_SCALE_NZ.get() as u128;

const CURVE_EXPONENT_SCALE: u32 = 10_000;
const CURVE_EXPONENT_STEP: u32 = 1_000;

// Prop AMM sell dampening needs `utilization^e`, where:
//   utilization = raw_sell_value / effective_liquidity
//   e           = curve_exponent_scaled / CURVE_EXPONENT_SCALE
//
// Both `utilization` and the return value are scaled by HARD_WALL_SCALE.
// Integer exponents use repeated fixed-point multiplication with saturation
// at extreme values. Fractional exponents use:
//   utilization^e = 2^(e * log2(utilization))
//
// The approximation is intentionally table-free:
// - `log2_integer_q` normalizes the input to mantissa `m` in [1, 2), then uses
//   ln(m) = 2 * (z + z^3/3 + z^5/5 + ...), z = (m - 1) / (m + 1), converted by
//   log2_e. Seven odd terms total are used: z through z^13/13.
// - `exp2_hard_wall_scaled_q` splits the exponent into integer and fractional
//   parts, computes exp(frac * ln(2)) with ten Taylor terms, then applies the
//   integer power-of-two shift.
//
// Q40 is used inside the approximation to keep enough precision while staying
// cheap in compute units compared with generic nth-root or table interpolation.
const POW_APPROX_Q_SHIFT: u32 = 40;
const POW_APPROX_Q: u128 = 1_u128 << POW_APPROX_Q_SHIFT;
const POW_APPROX_LN2_Q: u128 = 762_123_384_786;
const POW_APPROX_LOG2_E_Q: u128 = 1_586_259_972_792;
const LOG2_HARD_WALL_SCALE_Q: i128 = 43_829_982_801_540;

// W = L / (1 + s * V / L)
pub(crate) fn dynamic_wall_position(
    actual_liquidity: NonZeroU64,
    effective_sell_volume: u64,
    wall_sensitivity_scaled: u32,
) -> Result<NonZeroU64, PricingError> {
    if effective_sell_volume == 0 {
        return Ok(actual_liquidity);
    }

    // Safe: u32 × u64 < u128::MAX
    let scaled_pressure_ratio = wall_sensitivity_scaled as u128 * effective_sell_volume as u128
        / actual_liquidity.get() as u128;

    let denominator = WALL_SENSITIVITY_SCALE + scaled_pressure_ratio;
    let wall = actual_liquidity.get() as u128 * WALL_SENSITIVITY_SCALE / denominator;

    // Defensive: wall is always <= liquidity (u64) and >= 1
    let wall: u64 = wall
        .max(1)
        .try_into()
        .map_err(|_| PricingError::MathOverflow)?;

    // We coerced to 1 above
    NonZeroU64::new(wall).ok_or(PricingError::InvalidAmount)
}

// haircut = peg_haircut × utilization^exponent
pub(crate) fn redemption_haircut_scaled(
    utilization: NonZeroU128,
    curve_peg_haircut_bps: u16,
    curve_exponent_scaled: u32,
) -> Result<u128, PricingError> {
    // the maximum haircut at full utilization
    let peg_haircut = bps_to_hard_wall_scale(curve_peg_haircut_bps);
    if peg_haircut == 0 {
        validate_curve_exponent_scaled(curve_exponent_scaled)?;
        return Ok(0);
    }

    let utilization_power = utilization_power_scaled(utilization, curve_exponent_scaled)?;

    let curve_haircut = match utilization_power.checked_mul(peg_haircut) {
        Some(scaled) => scaled / HARD_WALL_SCALE,
        // saturate to U128::MAX to signal maximum haircut (happens when utilization is far above 1)
        None => u128::MAX,
    };

    Ok(curve_haircut)
}

fn bps_to_hard_wall_scale(bps: u16) -> u128 {
    HARD_WALL_SCALE * bps as u128 / MAX_BASIS_POINTS
}

fn utilization_power_scaled(
    utilization: NonZeroU128,
    exponent_scaled: u32,
) -> Result<u128, PricingError> {
    validate_curve_exponent_scaled(exponent_scaled)?;

    if exponent_scaled == 0 || utilization == HARD_WALL_SCALE_NZ.into() {
        return Ok(HARD_WALL_SCALE);
    }

    if exponent_scaled.is_multiple_of(CURVE_EXPONENT_SCALE) {
        return Ok(integer_utilization_power_scaled(
            utilization.get(),
            exponent_scaled / CURVE_EXPONENT_SCALE,
        ));
    }

    let log2_u_q = log2_hard_wall_scaled_q(utilization.get());
    let exponentiated_log_q = log2_u_q
        .checked_mul(exponent_scaled as i128)
        .ok_or(PricingError::MathOverflow)?
        / CURVE_EXPONENT_SCALE as i128;

    Ok(exp2_hard_wall_scaled_q(exponentiated_log_q))
}

fn validate_curve_exponent_scaled(exponent_scaled: u32) -> Result<(), PricingError> {
    if exponent_scaled > CURVE_EXPONENT_SCALE.saturating_mul(10) {
        return Err(PricingError::InvalidCurveExponent);
    }

    if !exponent_scaled.is_multiple_of(CURVE_EXPONENT_STEP) {
        return Err(PricingError::InvalidCurveExponent);
    }

    Ok(())
}

fn integer_utilization_power_scaled(utilization: u128, exponent: u32) -> u128 {
    let mut value = HARD_WALL_SCALE;
    for _ in 0..exponent {
        value = value.saturating_mul(utilization) / HARD_WALL_SCALE;
    }
    value
}

fn log2_hard_wall_scaled_q(value: u128) -> i128 {
    log2_integer_q(value) - LOG2_HARD_WALL_SCALE_Q
}

fn log2_integer_q(value: u128) -> i128 {
    let msb = (u128::BITS - 1 - value.leading_zeros()) as i128;
    let shift = msb - POW_APPROX_Q_SHIFT as i128;
    let mantissa_q = if shift >= 0 {
        value >> shift as u32
    } else {
        value << (-shift as u32)
    };
    let z = mantissa_q
        .saturating_sub(POW_APPROX_Q)
        .saturating_mul(POW_APPROX_Q)
        / mantissa_q.saturating_add(POW_APPROX_Q);
    let z2 = (z.saturating_mul(z)) >> POW_APPROX_Q_SHIFT;

    let mut term = z;
    let mut sum = term;
    for divisor in [3_u128, 5, 7, 9, 11, 13] {
        term = (term.saturating_mul(z2)) >> POW_APPROX_Q_SHIFT;
        sum = sum.saturating_add(term / divisor);
    }

    let ln_mantissa_q = sum.saturating_mul(2);
    let fractional_q = (ln_mantissa_q.saturating_mul(POW_APPROX_LOG2_E_Q)) >> POW_APPROX_Q_SHIFT;
    msb.saturating_mul(POW_APPROX_Q as i128)
        .saturating_add(fractional_q as i128)
}

fn exp2_hard_wall_scaled_q(log2_value_q: i128) -> u128 {
    let q = POW_APPROX_Q as i128;
    let mut integer_part = log2_value_q / q;
    let mut fractional_part = log2_value_q % q;
    if fractional_part < 0 {
        fractional_part += q;
        integer_part -= 1;
    }

    let x_q = ((fractional_part as u128).saturating_mul(POW_APPROX_LN2_Q)) >> POW_APPROX_Q_SHIFT;
    let mut term = POW_APPROX_Q;
    let mut exp_fraction_q = POW_APPROX_Q;
    for divisor in [1_u128, 2, 3, 4, 5, 6, 7, 8, 9, 10] {
        term = (term.saturating_mul(x_q)) >> POW_APPROX_Q_SHIFT;
        term /= divisor;
        exp_fraction_q = exp_fraction_q.saturating_add(term);
    }

    let scaled = (exp_fraction_q.saturating_mul(HARD_WALL_SCALE)) >> POW_APPROX_Q_SHIFT;
    if integer_part >= 0 {
        let shift = integer_part as u32;
        if shift >= u128::BITS {
            return u128::MAX;
        }
        if scaled > (u128::MAX >> shift) {
            return u128::MAX;
        }
        return scaled << shift;
    }

    let shift = (-integer_part) as u32;
    if shift >= u128::BITS {
        0
    } else {
        scaled >> shift
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- bps_to_hard_wall_scale ---

    #[test]
    fn bps_zero() {
        let out = bps_to_hard_wall_scale(0);

        assert_eq!(out, 0);
    }

    #[test]
    fn bps_max() {
        let out = bps_to_hard_wall_scale(10_000);

        assert_eq!(out, HARD_WALL_SCALE);
    }

    #[test]
    fn bps_half() {
        let out = bps_to_hard_wall_scale(5_000);

        assert_eq!(out, HARD_WALL_SCALE / 2);
    }

    // --- validate_curve_exponent_scaled ---

    #[test]
    fn validate_exponent_valid() {
        assert!(validate_curve_exponent_scaled(0).is_ok());
        assert!(validate_curve_exponent_scaled(10_000).is_ok());
        assert!(validate_curve_exponent_scaled(100_000).is_ok());
        assert!(validate_curve_exponent_scaled(5_000).is_ok());
    }

    #[test]
    fn validate_exponent_too_high() {
        let result = validate_curve_exponent_scaled(100_001);

        assert_eq!(result, Err(PricingError::InvalidCurveExponent));
    }

    #[test]
    fn validate_exponent_not_step_multiple() {
        let result = validate_curve_exponent_scaled(1_500);

        assert_eq!(result, Err(PricingError::InvalidCurveExponent));

        let result = validate_curve_exponent_scaled(999);

        assert_eq!(result, Err(PricingError::InvalidCurveExponent));
    }

    // --- utilization_power_scaled ---

    #[test]
    fn power_exponent_zero_returns_scale() {
        let utilization = NonZeroU128::new(HARD_WALL_SCALE / 2).unwrap();

        let out = utilization_power_scaled(utilization, 0).unwrap();

        assert_eq!(out, HARD_WALL_SCALE);
    }

    #[test]
    fn power_full_utilization_returns_scale() {
        let utilization = NonZeroU128::new(HARD_WALL_SCALE).unwrap();

        let out = utilization_power_scaled(utilization, 30_000).unwrap();

        assert_eq!(out, HARD_WALL_SCALE);
    }

    #[test]
    fn power_integer_exponent_half_squared() {
        let utilization = NonZeroU128::new(HARD_WALL_SCALE / 2).unwrap();

        let out = utilization_power_scaled(utilization, 20_000).unwrap();

        // (0.5)^2 = 0.25
        assert_eq!(out, HARD_WALL_SCALE / 4);
    }

    #[test]
    fn power_fractional_exponent_sqrt() {
        let utilization = NonZeroU128::new(HARD_WALL_SCALE / 4).unwrap();

        let result = utilization_power_scaled(utilization, 5_000).unwrap();

        // (0.25)^0.5 = 0.5 — exercises the log2/exp2 path
        let expected = HARD_WALL_SCALE / 2;
        let tolerance = HARD_WALL_SCALE / 10_000; // 0.01%
        assert!(
            result.abs_diff(expected) < tolerance,
            "sqrt(0.25): expected ~{expected}, got {result}"
        );
    }

    #[test]
    fn power_fractional_exponent_1_5() {
        let utilization = NonZeroU128::new(HARD_WALL_SCALE / 2).unwrap();

        let result = utilization_power_scaled(utilization, 15_000).unwrap();

        // (0.5)^1.5 ≈ 0.353553
        let expected_f64 = 0.5_f64.powf(1.5) * HARD_WALL_SCALE as f64;
        let expected = expected_f64 as u128;
        let tolerance = HARD_WALL_SCALE / 1_000; // 0.1%
        assert!(
            result.abs_diff(expected) < tolerance,
            "0.5^1.5: expected ~{expected}, got {result}"
        );
    }

    #[test]
    fn power_fractional_exponent_small_utilization() {
        let utilization = NonZeroU128::new(HARD_WALL_SCALE / 10).unwrap();

        let result = utilization_power_scaled(utilization, 5_000).unwrap();

        // (0.1)^0.5 ≈ 0.316228
        let expected_f64 = 0.1_f64.powf(0.5) * HARD_WALL_SCALE as f64;
        let expected = expected_f64 as u128;
        let tolerance = HARD_WALL_SCALE / 1_000; // 0.1%
        assert!(
            result.abs_diff(expected) < tolerance,
            "0.1^0.5: expected ~{expected}, got {result}"
        );
    }

    // --- dynamic_wall_position ---

    #[test]
    fn zero_sell_volume_returns_liquidity() {
        let liquidity = NonZeroU64::new(1_000_000).unwrap();

        let result = dynamic_wall_position(liquidity, 0, 5_000);

        assert_eq!(result, Ok(liquidity));
    }

    #[test]
    fn zero_sensitivity_returns_liquidity() {
        let liquidity = NonZeroU64::new(1_000_000).unwrap();

        let result = dynamic_wall_position(liquidity, 500_000, 0);

        assert_eq!(result, Ok(liquidity));
    }

    #[test]
    fn moderate_sell_pressure_reduces_wall() {
        let liquidity = NonZeroU64::new(1_000_000).unwrap();

        let result = dynamic_wall_position(liquidity, 500_000, 5_000);

        // sensitivity_component = 5000 * 500_000 / 1_000_000 = 2_500
        // denominator = 10_000 + 2_500 = 12_500
        // wall = 1_000_000 * 10_000 / 12_500 = 800_000
        assert_eq!(result, Ok(NonZeroU64::new(800_000).unwrap()));
    }

    #[test]
    fn equal_volume_and_liquidity_halves_wall() {
        let liquidity = NonZeroU64::new(1_000_000).unwrap();

        let result = dynamic_wall_position(liquidity, 1_000_000, 10_000);

        // sensitivity_component = 10_000 * 1_000_000 / 1_000_000 = 10_000
        // denominator = 10_000 + 10_000 = 20_000
        // wall = 1_000_000 * 10_000 / 20_000 = 500_000
        assert_eq!(result, Ok(NonZeroU64::new(500_000).unwrap()));
    }

    #[test]
    fn high_pressure_pushes_wall_to_floor() {
        let liquidity = NonZeroU64::new(1_000).unwrap();

        let result = dynamic_wall_position(liquidity, 1_000_000, 10_000);

        // sensitivity_component = 10_000 * 1_000_000 / 1_000 = 10_000_000
        // denominator = 10_000 + 10_000_000 = 10_010_000
        // wall = 1_000 * 10_000 / 10_010_000 = 0 → .max(1) = 1
        assert_eq!(result, Ok(NonZeroU64::new(1).unwrap()));
    }

    #[test]
    fn minimal_sensitivity_barely_moves_wall() {
        let liquidity = NonZeroU64::new(10_000).unwrap();

        let result = dynamic_wall_position(liquidity, 10_000, 1);

        // sensitivity_component = 1 * 10_000 / 10_000 = 1
        // denominator = 10_000 + 1 = 10_001
        // wall = 10_000 * 10_000 / 10_001 = 9_999
        assert_eq!(result, Ok(NonZeroU64::new(9_999).unwrap()));
    }

    // --- redemption_haircut_scaled ---

    #[test]
    fn zero_peg_haircut_with_invalid_exponent_errors() {
        let utilization = NonZeroU128::new(HARD_WALL_SCALE / 2).unwrap();

        let result = redemption_haircut_scaled(utilization, 0, 999);

        assert_eq!(result, Err(PricingError::InvalidCurveExponent));
    }

    #[test]
    fn exponent_out_of_range_errors() {
        let utilization = NonZeroU128::new(HARD_WALL_SCALE / 2).unwrap();

        let result = redemption_haircut_scaled(utilization, 5_000, 110_000);

        assert_eq!(result, Err(PricingError::InvalidCurveExponent));
    }

    #[test]
    fn exponent_not_step_multiple_errors() {
        let utilization = NonZeroU128::new(HARD_WALL_SCALE / 2).unwrap();

        let result = redemption_haircut_scaled(utilization, 5_000, 1_500);

        assert_eq!(result, Err(PricingError::InvalidCurveExponent));
    }

    #[test]
    fn zero_peg_haircut_returns_zero() {
        let utilization = NonZeroU128::new(HARD_WALL_SCALE / 2).unwrap();

        let result = redemption_haircut_scaled(utilization, 0, 10_000);

        assert_eq!(result, Ok(0));
    }

    #[test]
    fn full_utilization_returns_peg_haircut() {
        let utilization = NonZeroU128::new(HARD_WALL_SCALE).unwrap();

        let result = redemption_haircut_scaled(utilization, 10_000, 30_000);

        // 1.0 ^ 3.0 = 1.0, haircut = bps_to_hard_wall_scale(10_000) = HARD_WALL_SCALE
        assert_eq!(result, Ok(HARD_WALL_SCALE));
    }

    #[test]
    fn exponent_zero_returns_peg_haircut() {
        let utilization = NonZeroU128::new(HARD_WALL_SCALE / 4).unwrap();

        let result = redemption_haircut_scaled(utilization, 10_000, 0);

        // exponent=0 → utilization_power = HARD_WALL_SCALE
        // haircut = peg_haircut * HARD_WALL_SCALE / HARD_WALL_SCALE = peg_haircut
        assert_eq!(result, Ok(HARD_WALL_SCALE));
    }

    #[test]
    fn half_utilization_linear_exponent() {
        let utilization = NonZeroU128::new(HARD_WALL_SCALE / 2).unwrap();

        let result = redemption_haircut_scaled(utilization, 10_000, 10_000);

        // 0.5 ^ 1.0 = 0.5, peg_haircut = HARD_WALL_SCALE (bps=10000)
        // haircut = HARD_WALL_SCALE * (HARD_WALL_SCALE/2) / HARD_WALL_SCALE = HARD_WALL_SCALE/2
        assert_eq!(result, Ok(HARD_WALL_SCALE / 2));
    }

    #[test]
    fn half_utilization_squared() {
        let utilization = NonZeroU128::new(HARD_WALL_SCALE / 2).unwrap();

        let result = redemption_haircut_scaled(utilization, 10_000, 20_000);

        // 0.5 ^ 2.0 = 0.25, peg_haircut = HARD_WALL_SCALE
        // haircut = HARD_WALL_SCALE * (HARD_WALL_SCALE/4) / HARD_WALL_SCALE = HARD_WALL_SCALE/4
        assert_eq!(result, Ok(HARD_WALL_SCALE / 4));
    }

    #[test]
    fn partial_peg_haircut_at_full_utilization() {
        let utilization = NonZeroU128::new(HARD_WALL_SCALE).unwrap();

        let result = redemption_haircut_scaled(utilization, 5_000, 10_000);

        // 1.0 ^ 1.0 = 1.0, peg_haircut = bps_to_hard_wall_scale(5000) = HARD_WALL_SCALE/2
        // haircut = (HARD_WALL_SCALE/2) * HARD_WALL_SCALE / HARD_WALL_SCALE = HARD_WALL_SCALE/2
        assert_eq!(result, Ok(HARD_WALL_SCALE / 2));
    }

    #[test]
    fn haircut_extreme_utilization_saturates() {
        // ratio 1e18
        let utilization = NonZeroU128::new(HARD_WALL_SCALE * 1_000_000_000_000_000_000).unwrap();

        let out = redemption_haircut_scaled(utilization, 10_000, 15_000).unwrap();

        // Utilization far above 1.0 with a fractional exponent saturates to u128::MAX
        assert_eq!(out, u128::MAX);
    }
}
