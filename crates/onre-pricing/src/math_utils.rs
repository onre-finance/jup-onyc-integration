pub fn mul_div_round(a: u128, b: u128, denom: u128) -> Option<u128> {
    if denom == 0 {
        return None;
    }

    let prod = a.checked_mul(b)?;
    let adj = prod.checked_add(denom / 2)?;

    Some(adj / denom)
}

pub fn ceil_div_u128(numerator: u128, denominator: u128) -> Option<u128> {
    if denominator == 0 {
        return None;
    }

    Some((numerator.checked_add(denominator - 1)?) / denominator)
}

/// Fixed-point exponentiation: `(x / base)^n * base`
///
/// # Arguments
///
/// * `x` - base-scaled value (e.g. 1e18 + rate_per_second)
/// * `n` - exponent (e.g. seconds elapsed)
/// * `base` - scaling factor (e.g. 1e18)
pub fn pow_fixed(x: u128, n: u64, base: u128) -> Option<u128> {
    let mut result = base;
    let mut x = x;
    let mut n = n;

    loop {
        if n % 2 == 1 {
            result = mul_div_round(result, x, base)?;
        }
        n /= 2;
        if n == 0 {
            break;
        }

        x = mul_div_round(x, x, base)?;
    }

    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::APR_SCALE;

    const SCALE: u128 = 1_000_000_000_000_000_000;

    // ── ceil_div_u128 ──────────────────────────────────────────────

    #[test]
    fn ceil_div_exact() {
        assert_eq!(ceil_div_u128(10, 5), Some(2));
    }

    #[test]
    fn ceil_div_rounds_up() {
        assert_eq!(ceil_div_u128(11, 5), Some(3));
    }

    #[test]
    fn ceil_div_zero_denominator() {
        assert_eq!(ceil_div_u128(10, 0), None);
    }

    #[test]
    fn ceil_div_zero_numerator() {
        assert_eq!(ceil_div_u128(0, 5), Some(0));
    }

    #[test]
    fn ceil_div_max_numerator() {
        assert_eq!(ceil_div_u128(u128::MAX, 1), Some(u128::MAX));
        // u128::MAX + (2-1) overflows checked_add → None
        assert_eq!(ceil_div_u128(u128::MAX, 2), None);
    }

    // ── mul_div_round ──────────────────────────────────────────────

    #[test]
    fn mul_div_round_exact() {
        assert_eq!(mul_div_round(10, 20, 10), Some(20));
    }

    #[test]
    fn mul_div_round_rounds_to_nearest() {
        // 7 * 3 / 10 = 2.1 → rounds to 2
        assert_eq!(mul_div_round(7, 3, 10), Some(2));
        // 8 * 3 / 10 = 2.4 → rounds to 2
        assert_eq!(mul_div_round(8, 3, 10), Some(2));
        // 9 * 3 / 10 = 2.7 → rounds to 3
        assert_eq!(mul_div_round(9, 3, 10), Some(3));
    }

    #[test]
    fn mul_div_round_overflow_returns_none() {
        assert_eq!(mul_div_round(u128::MAX, u128::MAX, 1), None);
    }

    // ── rpow ───────────────────────────────────────────────────────

    #[test]
    fn rpow_zero_exponent() {
        assert_eq!(pow_fixed(2 * SCALE, 0, SCALE).unwrap(), SCALE);
    }

    #[test]
    fn rpow_identity() {
        assert_eq!(pow_fixed(SCALE, 100, SCALE).unwrap(), SCALE);
    }

    #[test]
    fn rpow_small_base() {
        // 1.001^1 = 1.001
        let b = SCALE + SCALE / 1000;
        assert_eq!(pow_fixed(b, 1, SCALE).unwrap(), b);

        // 1.001^2 = 1.002001
        let result = pow_fixed(b, 2, SCALE).unwrap();
        let expected = SCALE * 1_002_001 / 1_000_000;
        assert_eq!(result, expected);
    }

    #[test]
    fn rpow_fractional_base() {
        // 1.05^1 = 1.05
        let base_1_05 = SCALE + SCALE / 20;
        let result = pow_fixed(base_1_05, 1, SCALE).unwrap();
        assert_eq!(result, base_1_05);

        // 1.05^2 = 1.1025
        let result = pow_fixed(base_1_05, 2, SCALE).unwrap();
        let expected = SCALE * 11025 / 10000;
        assert_eq!(result, expected);
    }

    #[test]
    fn rpow_matches_f64() {
        // 5% APR daily rate compounded 365 days
        let daily_rate = SCALE + (50_000u128 * SCALE / (365 * APR_SCALE));
        let result = pow_fixed(daily_rate, 365, SCALE).unwrap();

        let f64_result = (1.0 + 0.05 / 365.0_f64).powi(365);
        let f64_scaled = (f64_result * SCALE as f64) as u128;

        let diff = result.abs_diff(f64_scaled);
        assert!(diff < 100_000, "rpow vs f64 diff={diff}");
    }
}
