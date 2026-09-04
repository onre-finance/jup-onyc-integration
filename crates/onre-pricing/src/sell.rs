use crate::hard_wall_math::{
    dynamic_wall_position, redemption_haircut_scaled, HARD_WALL_SCALE, HARD_WALL_SCALE_NZ,
};
use crate::types::{DampeningState, LiquidityParams};
use crate::{
    calculate_fee, calculate_step_price_at, PriceVector, PricingError, MAX_TOKEN_DECIMALS,
    PRICE_DECIMALS,
};
use core::cmp::max;
use core::num::{NonZeroU128, NonZeroU32, NonZeroU64};

const MAX_CADENCE_WAVE_SCALED: u32 = 50_000;

const CADENCE_WAVE_SCALE: u16 = 10_000;
const CADENCE_WAVE_STEP: u32 = 1_000;
const CADENCE_WAVE_EASE: u8 = 8;
const CADENCE_WAVE_CAP_DIVISOR: u8 = 3;

const CADENCE_WAVE_DIVISOR: u64 = CADENCE_WAVE_SCALE as u64 * CADENCE_WAVE_CAP_DIVISOR as u64;

/// Shortcut for calling [`calculate_step_price_at`] and [`calculate_token_out_amount`].
pub fn calculate_amount_out(
    vector: &PriceVector,
    time: u64,
    amount_in: u64,
    fee_bps: u16,
    in_decimals: u8,
    out_decimals: u8,
    min_fee: u64,
    liquidity: &LiquidityParams,
) -> Result<u64, PricingError> {
    let price = calculate_step_price_at(
        vector.apr,
        vector.base_price,
        vector.base_time,
        vector.price_fix_duration,
        time,
    )?;

    calculate_token_out_amount(
        amount_in,
        price,
        time,
        fee_bps,
        in_decimals,
        out_decimals,
        min_fee,
        liquidity,
    )
}

/// Converts `amount_in` (token_in) to token_out at `price` (token_out per token_in),
/// applying fees (with `min_fee` floor) and liquidity dampening.
///
/// # Arguments
/// * `price` - Price with 9 decimal precision
pub fn calculate_token_out_amount(
    amount_in: u64,
    price: u64,
    time: u64,
    fee_bps: u16,
    in_decimals: u8,
    out_decimals: u8,
    min_fee: u64,
    liquidity: &LiquidityParams,
) -> Result<u64, PricingError> {
    let raw_amount_out = calculate_amount_out_raw(
        amount_in,
        price,
        fee_bps,
        min_fee,
        in_decimals,
        out_decimals,
    )?;

    let amount_out = apply_hard_wall_liquidity_factor_at_time(
        raw_amount_out,
        liquidity.liquidity,
        liquidity.liquidity_cap,
        &liquidity.dampening,
        time,
    )?;

    Ok(amount_out)
}

/// Converts `amount_in` to token_out at `price` (token_out per token_in), before liquidity dampening.
/// Applies the fee floored at `min_fee` and floors the result
/// in favor of the protocol.
fn calculate_amount_out_raw(
    amount_in: u64,
    price: u64,
    fee_bps: u16,
    min_fee: u64,
    in_decimals: u8,
    out_decimals: u8,
) -> Result<u64, PricingError> {
    if amount_in < min_fee {
        return Err(PricingError::InvalidAmount);
    }

    if price == 0 {
        return Err(PricingError::ZeroPriceNotAllowed);
    }

    if in_decimals > MAX_TOKEN_DECIMALS || out_decimals > MAX_TOKEN_DECIMALS {
        return Err(PricingError::DecimalsExceedMax {
            max: MAX_TOKEN_DECIMALS,
            was: max(in_decimals, out_decimals),
        });
    }

    let fee_amount = calculate_fee(amount_in, fee_bps)?.max(min_fee);
    let amount_in_net = amount_in - fee_amount;

    if amount_in_net == 0 {
        return Ok(0);
    }

    let numerator = (amount_in_net as u128 * price as u128)
        .checked_mul(10_u128.pow(out_decimals as u32))
        .ok_or(PricingError::MathOverflow)?;

    // Safe: 10^(≤27) fits trivially in u128
    let denominator = 10_u128.pow(in_decimals as u32 + PRICE_DECIMALS as u32);

    let result = numerator / denominator;

    result.try_into().map_err(|_| PricingError::MathOverflow)
}

fn apply_hard_wall_liquidity_factor_at_time(
    amount_out: u64,
    liquidity: u64,
    liquidity_cap: u64,
    dampening: &DampeningState,
    time: u64,
) -> Result<u64, PricingError> {
    let liquidity = NonZeroU64::new(liquidity).ok_or(PricingError::ZeroLiquidity)?;
    let liquidity_cap = NonZeroU64::new(liquidity_cap).ok_or(PricingError::ZeroLiquidity)?;

    let amount_out = match NonZeroU64::new(amount_out) {
        Some(amount_out) => amount_out,
        None => return Ok(0),
    };

    if amount_out > liquidity {
        return Err(PricingError::InsufficientLiquidity);
    }

    // Final sell output is:
    //   effective_liquidity = min(dynamic_wall(actual, sell_pressure), liquidity_cap)
    //   utilization = amount_out / effective_liquidity
    //   base_haircut = peg_haircut * utilization^curve_exponent
    //   cadence_target = cadence_wave(utilization, prior_sell_count)
    //   haircut = max(base_haircut, cadence_target)
    //   output = amount_out * max(0, 1 - haircut)
    //
    // `liquidity` is the solvency bound. `liquidity_cap` is either the
    // same actual balance or min(actual balance, TVL target), depending on
    // redemption-offer configuration. The base exponent power is approximated
    // in hard_wall_math.rs; the cadence target uses integer-only rational math.
    let effective_liquidity = dynamic_wall_liquidity_at_time(
        amount_out.get(),
        liquidity,
        liquidity_cap,
        dampening,
        time,
    )?;

    let utilization_scaled = if amount_out == effective_liquidity {
        HARD_WALL_SCALE_NZ.into()
    } else {
        let utilization_scaled =
            amount_out.get() as u128 * HARD_WALL_SCALE / effective_liquidity.get() as u128;

        match NonZeroU128::new(utilization_scaled) {
            Some(utilization_scaled) => utilization_scaled,
            // This can happen if the amount_out is very small in respect to the liquidity
            None => return Ok(amount_out.get()),
        }
    };

    let base_haircut = redemption_haircut_scaled(
        utilization_scaled,
        dampening.max_haircut_bps,
        dampening.exponent_scaled,
    )?;

    let cadence_wave_y_scaled = cadence_wave_y_scaled(dampening, time)?;
    let cadence_target_haircut =
        cadence_wave_target_haircut_scaled(utilization_scaled, cadence_wave_y_scaled)?;
    let haircut = base_haircut.max(cadence_target_haircut);

    let liquidity_factor = HARD_WALL_SCALE.saturating_sub(haircut);

    let dampened_amount = (amount_out.get() as u128 * liquidity_factor) / HARD_WALL_SCALE;

    dampened_amount
        .try_into()
        .map_err(|_| PricingError::MathOverflow)
}

fn dynamic_wall_liquidity_at_time(
    incoming_sell_value: u64,
    liquidity: NonZeroU64,
    liquidity_cap: NonZeroU64,
    dampening: &DampeningState,
    time: u64,
) -> Result<NonZeroU64, PricingError> {
    if dampening.wall_sensitivity_scaled == 0 {
        return Ok(liquidity.min(liquidity_cap));
    }

    let effective_sell_volume =
        preview_effective_sell_volume(dampening, incoming_sell_value, time)?;

    let wall_position = dynamic_wall_position(
        liquidity,
        effective_sell_volume,
        dampening.wall_sensitivity_scaled,
    )?;

    Ok(wall_position.min(liquidity_cap))
}

fn preview_effective_sell_volume(
    dampening: &DampeningState,
    incoming_sell_value: u64,
    time: u64,
) -> Result<u64, PricingError> {
    let epoch_duration = dampening.epoch_duration_seconds;
    if epoch_duration <= 0 {
        return Err(PricingError::InvalidEpochDuration);
    };

    let elapsed = time.saturating_sub(dampening.epoch_start);

    if dampening.epoch_start == 0 || elapsed <= 0 || elapsed >= epoch_duration.saturating_mul(2) {
        return Ok(incoming_sell_value);
    }

    let current_net = dampening.sell_volume.saturating_sub(dampening.buy_volume);

    let (prev_net, curr_net, remaining) = if elapsed < epoch_duration {
        let remaining = epoch_duration - elapsed;

        (dampening.prev_net_sell_volume, current_net, remaining)
    } else {
        let remaining = epoch_duration;

        (current_net, 0, remaining)
    };

    // Safe: u64 × positive-i64 < 2^127 < u128::MAX;
    let decayed_prev: u64 = (prev_net as u128 * remaining as u128 / epoch_duration as u128)
        .try_into()
        .map_err(|_| PricingError::MathOverflow)?;

    let effective = decayed_prev
        .checked_add(curr_net)
        .ok_or(PricingError::MathOverflow)?
        .checked_add(incoming_sell_value)
        .ok_or(PricingError::MathOverflow)?;

    Ok(effective)
}

fn cadence_wave_y_scaled(dampening: &DampeningState, time: u64) -> Result<u128, PricingError> {
    let threshold =
        NonZeroU32::new(dampening.cadence_threshold).ok_or(PricingError::InvalidCadenceConfig)?;

    if dampening.cadence_wave_scaled > MAX_CADENCE_WAVE_SCALED {
        return Err(PricingError::InvalidCadenceConfig);
    }

    if !dampening
        .cadence_wave_scaled
        .is_multiple_of(CADENCE_WAVE_STEP)
    {
        return Err(PricingError::InvalidCadenceConfig);
    }

    let max_wave_y = dampening.cadence_wave_scaled as u128;
    if max_wave_y == 0 {
        return Ok(0);
    }

    let sell_count = preview_current_sell_trade_count(dampening, time)?;
    if sell_count == 0 {
        return Ok(0);
    }

    let ramp = if sell_count >= threshold.get() {
        CADENCE_WAVE_SCALE.into()
    } else {
        sell_count as u128 * CADENCE_WAVE_SCALE as u128 / threshold.get() as u128
    };

    let wave_y = max_wave_y
        .checked_mul(ramp)
        .ok_or(PricingError::MathOverflow)?
        .checked_div(CADENCE_WAVE_SCALE.into())
        .ok_or(PricingError::MathOverflow)?;

    Ok(wave_y)
}

fn cadence_wave_target_haircut_scaled(
    utilization: NonZeroU128,
    wave_y_scaled: u128,
) -> Result<u128, PricingError> {
    if wave_y_scaled == 0 {
        return Ok(0);
    }

    // Clamp to [1, HARD_WALL_SCALE]
    let clamped_utilization = utilization.min(HARD_WALL_SCALE_NZ.into()).get();
    let gap = HARD_WALL_SCALE.saturating_sub(clamped_utilization);

    let eased_numerator = clamped_utilization
        .checked_mul(CADENCE_WAVE_EASE as u128)
        .ok_or(PricingError::MathOverflow)?;

    let eased_denominator = eased_numerator
        .checked_add(gap)
        .ok_or(PricingError::MathOverflow)?;

    let eased_utilization = eased_numerator
        .checked_mul(HARD_WALL_SCALE)
        .ok_or(PricingError::MathOverflow)?
        / eased_denominator;

    let target_haircut = eased_utilization
        .checked_mul(wave_y_scaled)
        .ok_or(PricingError::MathOverflow)?
        / CADENCE_WAVE_DIVISOR as u128;

    Ok(target_haircut.min(HARD_WALL_SCALE))
}

fn preview_current_sell_trade_count(
    dampening: &DampeningState,
    time: u64,
) -> Result<u32, PricingError> {
    let epoch_duration = dampening.epoch_duration_seconds;
    if epoch_duration <= 0 {
        return Err(PricingError::InvalidEpochDuration);
    }

    let elapsed = time.saturating_sub(dampening.epoch_start);
    if dampening.epoch_start == 0 || dampening.epoch_start > time || elapsed >= epoch_duration {
        return Ok(0);
    }

    Ok(dampening.sell_trade_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SECONDS_IN_DAY;

    fn default_state() -> DampeningState {
        DampeningState {
            max_haircut_bps: 0,
            exponent_scaled: 0,
            cadence_threshold: 0,
            cadence_wave_scaled: 0,
            wall_sensitivity_scaled: 0,
            epoch_start: 1_000,
            epoch_duration_seconds: 100,
            sell_volume: 0,
            buy_volume: 0,
            sell_trade_count: 0,
            prev_net_sell_volume: 0,
        }
    }

    // --- calculate_amount_out_raw ---

    #[test]
    fn amount_out_raw_with_price_above_one() {
        let out = calculate_amount_out_raw(
            100_000_000_000, // 100 ONyc
            1_500_000_000,   // price = 1.5
            0,
            0,
            9, // ONyc decimals
            6, // USDC decimals
        )
        .unwrap();

        // 100 ONyc at price 1.5 -> 150 USDC
        assert_eq!(out, 150_000_000);
    }

    #[test]
    fn amount_out_raw_with_equal_decimals() {
        let out = calculate_amount_out_raw(
            100_000_000_000, // 100 ONyc
            1_500_000_000,   // price = 1.5
            0,
            0,
            9, // ONyc decimals
            9, // Token decimals
        )
        .unwrap();

        // 100 ONyc at price 1.5 -> 150 Token
        assert_eq!(out, 150_000_000_000);
    }

    #[test]
    fn amount_out_raw_with_fewer_in_decimals() {
        let out = calculate_amount_out_raw(
            100_000_000_000, // 100_000 Token0
            1_500_000_000,   // price = 1.5
            0,
            0,
            6, // Token0 decimals
            9, // Token1 decimals
        )
        .unwrap();

        // 100_000 Token0 at price 1.5 -> 150_000 Token1
        assert_eq!(out, 150_000_000_000_000);
    }

    #[test]
    fn amount_out_raw_floors_in_favor_of_protocol() {
        let out = calculate_amount_out_raw(
            1_000_000_000, // 1 ONyc
            333_333_333,   // price = 0.333...
            0,
            0,
            9,
            6,
        )
        .unwrap();

        // result = 333_333.333 -> floor = 333_333
        assert_eq!(out, 333_333);
    }

    #[test]
    fn amount_out_raw_with_fee() {
        let out = calculate_amount_out_raw(
            1_000_000_000, // 1 ONyc
            1_000_000_000, // price = 1.0
            500,           // 5% fee
            0,
            9,
            6,
        )
        .unwrap();

        // 1 ONyc at price 1.0, 5% fee -> 0.95 USDC
        assert_eq!(out, 950_000);
    }

    #[test]
    fn amount_out_raw_min_fee_overrides_computed_fee() {
        let out = calculate_amount_out_raw(
            1_000_000_000, // 1 ONyc
            1_000_000_000, // price = 1.0
            100,           // 1% (computed fee = 10_000_000)
            100_000_000,   // min_fee = 100_000_000 (> computed fee)
            9,
            6,
        )
        .unwrap();

        // min_fee overrides: fee = 0.1 ONyc
        // 0.9 ONyc at price 1.0 -> 0.9 USDC
        assert_eq!(out, 900_000);
    }

    #[test]
    fn amount_out_raw_fee_consumes_entire_input_returns_zero() {
        let out = calculate_amount_out_raw(
            100,
            1_000_000_000,
            0,
            100, // min_fee == amount_in
            9,
            6,
        )
        .unwrap();

        // amount_in == min_fee -> net becomes 0
        assert_eq!(out, 0);
    }

    #[test]
    fn amount_out_raw_fee_ceiling_rounds_up() {
        let out = calculate_amount_out_raw(
            1_001,
            1_000_000_000, // price = 1.0
            500,
            0,
            9,
            9,
        )
        .unwrap();

        // fee = ceil(1001 * 500 / 10_000) = 51, net = 950
        assert_eq!(out, 950);
    }

    #[test]
    fn token_out_zero_input_returns_zero() {
        let out = calculate_amount_out_raw(0, 1_000_000_000, 0, 0, 9, 6).unwrap();

        assert_eq!(out, 0);
    }

    #[test]
    fn token_out_rejects_zero_price() {
        let result = calculate_amount_out_raw(100, 0, 0, 0, 9, 6);

        assert_eq!(result, Err(PricingError::ZeroPriceNotAllowed));
    }

    #[test]
    fn token_out_rejects_decimals_above_max() {
        let result = calculate_amount_out_raw(100, 1_000_000_000, 0, 0, 19, 9);
        assert_eq!(
            result,
            Err(PricingError::DecimalsExceedMax { max: 18, was: 19 })
        );

        let result = calculate_amount_out_raw(100, 1_000_000_000, 0, 0, 9, 19);
        assert_eq!(
            result,
            Err(PricingError::DecimalsExceedMax { max: 18, was: 19 })
        );
    }

    #[test]
    fn amount_out_raw_numerator_overflow() {
        let result = calculate_amount_out_raw(u64::MAX, u64::MAX, 0, 0, 0, 18);

        // u64::MAX * u64::MAX fits u128, but * 10^18 overflows checked_mul
        assert_eq!(result, Err(PricingError::MathOverflow));
    }

    #[test]
    fn amount_out_raw_result_exceeds_u64() {
        let result = calculate_amount_out_raw(u64::MAX, 1_000_000_000, 0, 0, 6, 9);

        // numerator fits u128 but result = 1.84e22 overflows u64 on try_into
        assert_eq!(result, Err(PricingError::MathOverflow));
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

        // No-op dampening: haircut resolves to 0, and the raw amount passes
        // through the hard wall unchanged, isolating the pricing path.
        let liquidity = LiquidityParams {
            liquidity: 1_000_000_000,
            liquidity_cap: 1_000_000_000,
            dampening: DampeningState {
                max_haircut_bps: 0,
                exponent_scaled: 0,
                cadence_threshold: 1, // must be ≥ 1: NonZeroU32 built before zero-wave short-circuit
                cadence_wave_scaled: 0,
                wall_sensitivity_scaled: 0,
                epoch_start: 1_000,
                epoch_duration_seconds: 100,
                sell_volume: 0,
                buy_volume: 0,
                sell_trade_count: 0,
                prev_net_sell_volume: 0,
            },
        };

        let out = calculate_amount_out(
            &vector,
            1_000 + 100 * SECONDS_IN_DAY, // base + 100 days
            100_000_000_000,              // 100 ONyc
            100,                          // 1% fee
            9,                            // ONyc decimals
            6,                            // USDC decimals
            0,                            // min_fee
            &liquidity,
        )
        .unwrap();

        let out_1 = calculate_amount_out(
            &vector,
            1_000 + 101 * SECONDS_IN_DAY, // base + 101 days
            100_000_000_000,              // 100 ONyc
            100,                          // 1% fee
            9,                            // ONyc decimals
            6,                            // USDC decimals
            0,                            // min_fee
            &liquidity,
        )
        .unwrap();

        assert!(out_1 > out, "output must increase as price grows");

        // 1% fee -> 99 ONyc net in

        // price_0 = 1_010_150_667
        // 99 * 1.010150667 = 100.004916 USDC
        assert_eq!(out, 100_004_916);

        // price_1 = 1_010_251_682
        // 99 * 1.010251682 = 100.014916 USDC
        assert_eq!(out_1, 100_014_916);
    }

    #[test]
    fn calculate_amount_out_dampening_end_to_end() {
        let vector = PriceVector {
            start_time: 1_000,
            base_time: 1_000,
            base_price: 1_000_000_000, // 1.0
            apr: 36_500,               // 3.65 %
            price_fix_duration: SECONDS_IN_DAY,
        };

        // Active dampening: 100% peg haircut, linear curve. The raw output is
        // 100_004_916 (see `calculate_amount_out_end_to_end`); a liquidity_cap of
        // exactly 2x that puts utilization at 0.5, so a linear 100% haircut halves
        // the output. This verifies the raw amount is threaded through the hard
        // wall by the public entry point, not just by the helper's unit tests.
        let raw_out = 100_004_916;
        let liquidity = LiquidityParams {
            liquidity: 1_000_000_000,
            liquidity_cap: 2 * raw_out, // utilization = raw_out / (2 × raw_out) = 0.5
            dampening: DampeningState {
                max_haircut_bps: 10_000, // 100% peg haircut
                exponent_scaled: 10_000, // linear
                cadence_threshold: 1,
                cadence_wave_scaled: 0,
                wall_sensitivity_scaled: 0,
                epoch_start: 1_000,
                epoch_duration_seconds: 100,
                sell_volume: 0,
                buy_volume: 0,
                sell_trade_count: 0,
                prev_net_sell_volume: 0,
            },
        };

        let out = calculate_amount_out(
            &vector,
            1_000 + 100 * SECONDS_IN_DAY, // base + 100 days
            100_000_000_000,              // 100 ONyc
            100,                          // 1% fee
            9,                            // ONyc decimals
            6,                            // USDC decimals
            0,                            // min_fee
            &liquidity,
        )
        .unwrap();

        assert!(out < raw_out, "dampening must reduce the undampened output");

        // utilization 0.5, linear 100% haircut -> output = raw_out × 0.5
        assert_eq!(out, 50_002_458);
    }

    #[test]
    fn token_in_below_minimum_fee_errors() {
        let vector = PriceVector {
            start_time: 1_000,
            base_time: 1_000,
            base_price: 1_000_000_000,
            apr: 36_500,
            price_fix_duration: 86_400,
        };
        let state = DampeningState {
            max_haircut_bps: 0,
            exponent_scaled: 0,
            cadence_threshold: 1,
            cadence_wave_scaled: 0,
            wall_sensitivity_scaled: 0,
            epoch_start: 1_000,
            epoch_duration_seconds: 100,
            sell_volume: 0,
            buy_volume: 0,
            sell_trade_count: 0,
            prev_net_sell_volume: 0,
        };

        let result = calculate_amount_out(
            &vector,
            1_000,
            99, // amount_in < min_fee
            9,
            6,
            100,
            100, // min_fee = 100
            &LiquidityParams {
                liquidity: 1_000_000,
                liquidity_cap: 1_000_000,
                dampening: state,
            },
        );

        assert_eq!(result, Err(PricingError::InvalidAmount));
    }

    #[test]
    fn zero_epoch_duration_errors() {
        let state = DampeningState {
            epoch_duration_seconds: 0,
            ..default_state()
        };

        let result = preview_effective_sell_volume(&state, 500, 1_050);

        assert_eq!(result, Err(PricingError::InvalidEpochDuration));
    }

    #[test]
    fn epoch_start_zero_returns_incoming_value() {
        let state = DampeningState {
            epoch_start: 0,
            ..default_state()
        };

        let out = preview_effective_sell_volume(&state, 500, 1_050).unwrap();

        assert_eq!(out, 500);
    }

    #[test]
    fn now_before_epoch_start_returns_incoming_value() {
        let out = preview_effective_sell_volume(&default_state(), 500, 999).unwrap();

        assert_eq!(out, 500);
    }

    #[test]
    fn now_equals_epoch_start_returns_incoming_value() {
        let out = preview_effective_sell_volume(&default_state(), 500, 1_000).unwrap();

        assert_eq!(out, 500);
    }

    #[test]
    fn elapsed_equals_two_epochs_returns_incoming_value() {
        let out = preview_effective_sell_volume(&default_state(), 500, 1_200).unwrap();

        assert_eq!(out, 500);
    }

    #[test]
    fn elapsed_exceeds_two_epochs_returns_incoming_value() {
        let out = preview_effective_sell_volume(&default_state(), 500, 1_300).unwrap();

        assert_eq!(out, 500);
    }

    // --- first epoch: decay of prev_net ---

    #[test]
    fn first_epoch_no_history_returns_incoming_value() {
        let out = preview_effective_sell_volume(&default_state(), 500, 1_050).unwrap();

        assert_eq!(out, 500);
    }

    #[test]
    fn first_epoch_prev_net_decays_linearly_at_halfway() {
        let state = DampeningState {
            prev_net_sell_volume: 1_000,
            ..default_state()
        };

        let out = preview_effective_sell_volume(&state, 0, 1_050).unwrap();

        // elapsed=50, remaining=50 -> decayed = 1000*50/100 = 500
        assert_eq!(out, 500);
    }

    #[test]
    fn first_epoch_prev_net_decays_near_start() {
        let state = DampeningState {
            prev_net_sell_volume: 1_000,
            ..default_state()
        };

        let out = preview_effective_sell_volume(&state, 0, 1_001).unwrap();

        // elapsed=1, remaining=99 -> decayed = 1000*99/100 = 990
        assert_eq!(out, 990);
    }

    #[test]
    fn first_epoch_prev_net_decays_near_end() {
        let state = DampeningState {
            prev_net_sell_volume: 1_000,
            ..default_state()
        };

        let out = preview_effective_sell_volume(&state, 0, 1_099).unwrap();

        // elapsed=99, remaining=1 -> decayed = 1000*1/100 = 10
        assert_eq!(out, 10);
    }

    // --- first epoch: current net sell pressure ---

    #[test]
    fn first_epoch_current_net_added_when_sells_exceed_buys() {
        let state = DampeningState {
            sell_volume: 800,
            buy_volume: 300,
            ..default_state()
        };

        let out = preview_effective_sell_volume(&state, 200, 1_050).unwrap();

        // current_net = 500 -> effective = 0 + 500 + 200 = 700
        assert_eq!(out, 700);
    }

    #[test]
    fn first_epoch_current_net_saturates_when_buys_exceed_sells() {
        let state = DampeningState {
            sell_volume: 300,
            buy_volume: 800,
            ..default_state()
        };

        let out = preview_effective_sell_volume(&state, 200, 1_050).unwrap();

        // current_net saturates to 0 -> effective = 0 + 0 + 200 = 200
        assert_eq!(out, 200);
    }

    #[test]
    fn first_epoch_prev_and_current_combined() {
        let state = DampeningState {
            prev_net_sell_volume: 1_000,
            sell_volume: 600,
            buy_volume: 100,
            ..default_state()
        };

        let out = preview_effective_sell_volume(&state, 200, 1_025).unwrap();

        // decayed = 1000*75/100 = 750, current_net = 500
        // effective = 750 + 500 + 200 = 1450
        assert_eq!(out, 1_450);
    }

    // --- second epoch (elapsed in [epoch_duration, 2×epoch_duration)) ---

    #[test]
    fn second_epoch_current_net_becomes_prev_with_no_decay() {
        let state = DampeningState {
            sell_volume: 800,
            buy_volume: 300,
            ..default_state()
        };

        // current_net=500 becomes prev_net, remaining=epoch_duration -> no decay
        // effective = 500 + 0 + 200 = 700
        let out = preview_effective_sell_volume(&state, 200, 1_100).unwrap();
        assert_eq!(out, 700);

        // elapsed=150 (mid-second-epoch): remaining is always epoch_duration, so same result
        let out = preview_effective_sell_volume(&state, 200, 1_150).unwrap();
        assert_eq!(out, 700);
    }

    #[test]
    fn second_epoch_no_net_sell_returns_incoming_value() {
        let state = DampeningState {
            sell_volume: 100,
            buy_volume: 500,
            ..default_state()
        };

        let out = preview_effective_sell_volume(&state, 300, 1_100).unwrap();

        // current_net saturates to 0 -> effective = 0 + 0 + 300 = 300
        assert_eq!(out, 300);
    }

    // --- overflow ---

    #[test]
    fn overflow_on_effective_sum() {
        let state = DampeningState {
            prev_net_sell_volume: u64::MAX,
            sell_volume: u64::MAX,
            buy_volume: 0,
            ..default_state()
        };

        let result = preview_effective_sell_volume(&state, 1, 1_050);

        // decayed + curr_net overflows checked_add
        assert_eq!(result, Err(PricingError::MathOverflow));
    }

    // --- cadence_wave_target_haircut_scaled ---

    #[test]
    fn wave_y_zero_returns_zero() {
        let utilization = NonZeroU128::new(HARD_WALL_SCALE).unwrap();

        let out = cadence_wave_target_haircut_scaled(utilization, 0).unwrap();

        assert_eq!(out, 0);
    }

    #[test]
    fn minimum_utilization() {
        let utilization = NonZeroU128::new(1).unwrap();

        let out = cadence_wave_target_haircut_scaled(utilization, 30_000).unwrap();

        // eased_num = 1 * 8 = 8
        // eased_den = 8 + (1_000_000_000_000 - 1) = 1_000_000_000_007
        // eased_util = 8 * 1_000_000_000_000 / 1_000_000_000_007 = 7
        // target = 7 * 30_000 / 30_000 = 7
        assert_eq!(out, 7);
    }

    #[test]
    fn half_utilization() {
        let utilization = NonZeroU128::new(HARD_WALL_SCALE / 2).unwrap();

        let out = cadence_wave_target_haircut_scaled(utilization, 30_000).unwrap();

        // eased_num = 500_000_000_000 * 8 = 4_000_000_000_000
        // eased_den = 4_000_000_000_000 + 500_000_000_000 = 4_500_000_000_000
        // eased_util = 4_000_000_000_000 * 1_000_000_000_000 / 4_500_000_000_000 = 888_888_888_888
        // target = 888_888_888_888 * 30_000 / 30_000 = 888_888_888_888
        assert_eq!(out, 888_888_888_888);
    }

    #[test]
    fn full_utilization() {
        let utilization = NonZeroU128::new(HARD_WALL_SCALE).unwrap();

        let out = cadence_wave_target_haircut_scaled(utilization, 30_000).unwrap();

        // gap=0 -> eased_util = HARD_WALL_SCALE -> target = HARD_WALL_SCALE
        assert_eq!(out, HARD_WALL_SCALE);
    }

    #[test]
    fn utilization_above_scale_is_clamped() {
        let utilization = NonZeroU128::new(2 * HARD_WALL_SCALE).unwrap();

        let out = cadence_wave_target_haircut_scaled(utilization, 30_000).unwrap();

        // clamped to HARD_WALL_SCALE, same as full_utilization
        assert_eq!(out, HARD_WALL_SCALE);
    }

    #[test]
    fn partial_wave_y() {
        let utilization = NonZeroU128::new(HARD_WALL_SCALE).unwrap();

        let out = cadence_wave_target_haircut_scaled(utilization, 15_000).unwrap();

        // gap=0 -> target = HARD_WALL_SCALE * 15_000 / 30_000
        assert_eq!(out, HARD_WALL_SCALE / 2);
    }

    #[test]
    fn result_clamped_to_hard_wall_scale() {
        let utilization = NonZeroU128::new(HARD_WALL_SCALE).unwrap();

        let out = cadence_wave_target_haircut_scaled(utilization, 60_000).unwrap();

        // unclamped = 2 * HARD_WALL_SCALE -> clamped
        assert_eq!(out, HARD_WALL_SCALE);
    }

    // --- preview_current_sell_trade_count ---

    #[test]
    fn sell_count_invalid_epoch_duration_errors() {
        let state = DampeningState {
            epoch_duration_seconds: 0,
            ..default_state()
        };

        let result = preview_current_sell_trade_count(&state, 1_050);

        assert_eq!(result, Err(PricingError::InvalidEpochDuration));
    }

    #[test]
    fn sell_count_epoch_start_zero_returns_zero() {
        let state = DampeningState {
            epoch_start: 0,
            sell_trade_count: 5,
            ..default_state()
        };

        let out = preview_current_sell_trade_count(&state, 1_050).unwrap();

        assert_eq!(out, 0);
    }

    #[test]
    fn sell_count_before_epoch_returns_zero() {
        let state = DampeningState {
            sell_trade_count: 5,
            epoch_start: 1_000,
            ..default_state()
        };

        let out = preview_current_sell_trade_count(&state, 999).unwrap();

        assert_eq!(out, 0);
    }

    #[test]
    fn sell_count_past_epoch_returns_zero() {
        let state = DampeningState {
            sell_trade_count: 5,
            ..default_state()
        };

        let out = preview_current_sell_trade_count(&state, 1_100).unwrap();

        // elapsed = 100 = epoch_duration -> outside [0, epoch_duration)
        assert_eq!(out, 0);
    }

    #[test]
    fn sell_count_within_epoch_returns_count() {
        let state = DampeningState {
            sell_trade_count: 7,
            ..default_state()
        };

        let out = preview_current_sell_trade_count(&state, 1_050).unwrap();

        assert_eq!(out, 7);
    }

    #[test]
    fn sell_count_at_epoch_start_returns_count() {
        let state = DampeningState {
            sell_trade_count: 7,
            ..default_state()
        };

        let out = preview_current_sell_trade_count(&state, 1_000).unwrap();

        // time == epoch_start -> elapsed=0, within [0, epoch_duration)
        assert_eq!(out, 7);
    }

    // --- cadence_wave_y_scaled ---

    fn cadence_state(threshold: u32, wave_scaled: u32, sell_count: u32) -> DampeningState {
        DampeningState {
            cadence_threshold: threshold,
            cadence_wave_scaled: wave_scaled,
            sell_trade_count: sell_count,
            ..default_state()
        }
    }

    #[test]
    fn cadence_wave_zero_threshold_errors() {
        let state = cadence_state(0, 10_000, 5);

        let result = cadence_wave_y_scaled(&state, 1_050);

        assert_eq!(result, Err(PricingError::InvalidCadenceConfig));
    }

    #[test]
    fn cadence_wave_scaled_above_max_errors() {
        let state = cadence_state(10, 50_001, 5);

        let result = cadence_wave_y_scaled(&state, 1_050);

        assert_eq!(result, Err(PricingError::InvalidCadenceConfig));
    }

    #[test]
    fn cadence_wave_scaled_not_step_multiple_errors() {
        let state = cadence_state(10, 1_500, 5);

        let result = cadence_wave_y_scaled(&state, 1_050);

        assert_eq!(result, Err(PricingError::InvalidCadenceConfig));
    }

    #[test]
    fn cadence_wave_zero_max_returns_zero() {
        let state = cadence_state(10, 0, 5);

        let out = cadence_wave_y_scaled(&state, 1_050).unwrap();

        assert_eq!(out, 0);
    }

    #[test]
    fn cadence_wave_zero_sell_count_returns_zero() {
        let state = cadence_state(10, 10_000, 0);

        let out = cadence_wave_y_scaled(&state, 1_050).unwrap();

        assert_eq!(out, 0);
    }

    #[test]
    fn cadence_wave_count_below_threshold_ramps_proportionally() {
        let state = cadence_state(10, 20_000, 5);

        let out = cadence_wave_y_scaled(&state, 1_050).unwrap();

        // ramp = 5/10 * SCALE = 5_000 -> wave_y = 20_000 * 5_000 / 10_000 = 10_000
        assert_eq!(out, 10_000);
    }

    #[test]
    fn cadence_wave_count_at_threshold_saturates() {
        let state = cadence_state(10, 20_000, 10);

        let out = cadence_wave_y_scaled(&state, 1_050).unwrap();

        // ramp saturates at 10_000 -> wave_y = 20_000
        assert_eq!(out, 20_000);
    }

    #[test]
    fn cadence_wave_count_above_threshold_saturates() {
        let state = cadence_state(10, 20_000, 50);

        let out = cadence_wave_y_scaled(&state, 1_050).unwrap();

        assert_eq!(out, 20_000);
    }

    #[test]
    fn cadence_wave_at_max_scaled_succeeds() {
        // cadence_wave_scaled == MAX_CADENCE_WAVE_SCALED should pass the > check
        let state = cadence_state(10, 50_000, 10);

        let out = cadence_wave_y_scaled(&state, 1_050).unwrap();

        assert_eq!(out, 50_000);
    }

    // --- dynamic_wall_liquidity_at_time ---

    #[test]
    fn wall_liq_zero_sensitivity_returns_min_of_actual_and_reserve() {
        let state = DampeningState {
            wall_sensitivity_scaled: 0,
            ..default_state()
        };
        let actual = NonZeroU64::new(1_000_000).unwrap();
        let cap = NonZeroU64::new(500_000).unwrap();

        let out = dynamic_wall_liquidity_at_time(100, actual, cap, &state, 1_050).unwrap();
        assert_eq!(out, cap);

        // When actual < cap, returns actual
        let small_actual = NonZeroU64::new(200_000).unwrap();
        let big_cap = NonZeroU64::new(500_000).unwrap();

        let out =
            dynamic_wall_liquidity_at_time(100, small_actual, big_cap, &state, 1_050).unwrap();
        assert_eq!(out, small_actual);
    }

    #[test]
    fn wall_liq_with_sensitivity_capped_by_reserve() {
        let state = DampeningState {
            wall_sensitivity_scaled: 10_000,
            ..default_state()
        };
        let actual = NonZeroU64::new(1_000_000).unwrap();
        let cap = NonZeroU64::new(100).unwrap();

        let out = dynamic_wall_liquidity_at_time(0, actual, cap, &state, 1_050).unwrap();

        assert_eq!(out, cap);
    }

    #[test]
    fn wall_liq_sensitivity_reduces_wall_with_sell_pressure() {
        let state = DampeningState {
            wall_sensitivity_scaled: 10_000,
            sell_volume: 500_000,
            buy_volume: 0,
            ..default_state()
        };
        let actual = NonZeroU64::new(1_000_000).unwrap();
        let cap = NonZeroU64::new(1_000_000).unwrap();

        // wall = 1M * 10k / 15k = 666_666
        let out = dynamic_wall_liquidity_at_time(0, actual, cap, &state, 1_050).unwrap();

        assert_eq!(out, NonZeroU64::new(666_666).unwrap());
    }

    // --- hard wall ---

    fn dampening_state() -> DampeningState {
        DampeningState {
            max_haircut_bps: 10_000, // 100% peg haircut
            exponent_scaled: 10_000, // linear exponent
            cadence_threshold: 1,
            cadence_wave_scaled: 0,
            wall_sensitivity_scaled: 0,
            epoch_start: 1_000,
            epoch_duration_seconds: 100,
            sell_volume: 0,
            buy_volume: 0,
            sell_trade_count: 0,
            prev_net_sell_volume: 0,
        }
    }

    #[test]
    fn hard_wall_zero_liquidity_errors() {
        let result =
            apply_hard_wall_liquidity_factor_at_time(100, 0, 1_000, &dampening_state(), 1_050);

        assert_eq!(result, Err(PricingError::ZeroLiquidity));
    }

    #[test]
    fn hard_wall_zero_reserve_errors() {
        let result =
            apply_hard_wall_liquidity_factor_at_time(100, 1_000, 0, &dampening_state(), 1_050);

        assert_eq!(result, Err(PricingError::ZeroLiquidity));
    }

    #[test]
    fn hard_wall_zero_amount_returns_zero() {
        let out =
            apply_hard_wall_liquidity_factor_at_time(0, 1_000, 1_000, &dampening_state(), 1_050)
                .unwrap();

        assert_eq!(out, 0);
    }

    #[test]
    fn hard_wall_amount_exceeds_liquidity_errors() {
        let result = apply_hard_wall_liquidity_factor_at_time(
            1_001,
            1_000,
            1_000,
            &dampening_state(),
            1_050,
        );

        assert_eq!(result, Err(PricingError::InsufficientLiquidity));
    }

    #[test]
    fn hard_wall_no_haircut_passes_through() {
        let state = DampeningState {
            max_haircut_bps: 0,
            ..dampening_state()
        };

        let out =
            apply_hard_wall_liquidity_factor_at_time(500, 1_000, 1_000, &state, 1_050).unwrap();

        // peg_haircut=0 -> haircut=0 -> output=input
        assert_eq!(out, 500);
    }

    #[test]
    fn hard_wall_full_utilization_max_haircut() {
        let state = dampening_state();

        let out =
            apply_hard_wall_liquidity_factor_at_time(1_000, 1_000, 1_000, &state, 1_050).unwrap();

        // utilization=1.0 -> haircut=100% -> output=0
        assert_eq!(out, 0);
    }

    #[test]
    fn hard_wall_half_utilization_linear_haircut() {
        let state = dampening_state();

        let out =
            apply_hard_wall_liquidity_factor_at_time(500, 1_000, 1_000, &state, 1_050).unwrap();

        // utilization=0.5 -> haircut=0.5 -> output=250
        assert_eq!(out, 250);
    }

    #[test]
    fn hard_wall_utilization_above_one_via_cap_zeroes_output() {
        let state = dampening_state();

        let out = apply_hard_wall_liquidity_factor_at_time(900, 1_000, 100, &state, 1_050).unwrap();

        // liquidity_cap << amount_out < liquidity -> utilization > 1.0, haircut saturates
        assert_eq!(out, 0);
    }

    #[test]
    fn hard_wall_quadratic_exponent() {
        let state = DampeningState {
            exponent_scaled: 20_000,
            ..dampening_state()
        };

        let out =
            apply_hard_wall_liquidity_factor_at_time(500, 1_000, 1_000, &state, 1_050).unwrap();

        // exponent=2.0: 0.5^2 = 0.25, haircut=25%, output = 500 × 0.75 = 375
        assert_eq!(out, 375);
    }

    #[test]
    fn hard_wall_partial_peg_at_full_utilization() {
        let state = DampeningState {
            max_haircut_bps: 5_000,
            ..dampening_state()
        };

        let out =
            apply_hard_wall_liquidity_factor_at_time(1_000, 1_000, 1_000, &state, 1_050).unwrap();

        // max_haircut=50% at full utilization -> output = 1000 × 0.5 = 500
        assert_eq!(out, 500);
    }

    #[test]
    fn hard_wall_tiny_amount_relative_to_liquidity_returns_amount_out() {
        let state = dampening_state();

        let out = apply_hard_wall_liquidity_factor_at_time(
            1_000,
            1_000_000_000_000_000_000,
            u64::MAX,
            &state,
            1_050,
        )
        .unwrap();

        assert_eq!(out, 1_000);
    }

    #[test]
    fn hard_wall_fractional_exponent_extreme_utilization_saturates_to_zero() {
        // Fractional curve exponent with a liquidity_cap far below the trade size drives
        // utilization far above 1.0. The haircut must saturate (>= 100%) to output 0,
        // not error out (regression guard: this path used to return MathOverflow).
        let state = DampeningState {
            exponent_scaled: 15_000, // 1.5, fractional
            ..dampening_state()
        };

        // cap=1 (wall_sensitivity 0 -> eff_liq=1); utilization = 1e18 * 1e12 / 1 >> 1
        let out = apply_hard_wall_liquidity_factor_at_time(
            1_000_000_000_000_000_000,
            u64::MAX,
            1,
            &state,
            1_050,
        )
        .unwrap();

        assert_eq!(out, 0);
    }

    #[test]
    fn hard_wall_cadence_beats_base_when_higher() {
        // peg_haircut=0 (base=0), wave=30_000, threshold=1, sell_count=1
        let state = DampeningState {
            max_haircut_bps: 0,
            exponent_scaled: 10_000,
            cadence_threshold: 1,
            cadence_wave_scaled: 30_000,
            wall_sensitivity_scaled: 0,
            epoch_start: 1_000,
            epoch_duration_seconds: 100,
            sell_volume: 0,
            buy_volume: 0,
            sell_trade_count: 1,
            prev_net_sell_volume: 0,
        };

        let out =
            apply_hard_wall_liquidity_factor_at_time(500, 1_000, 1_000, &state, 1_050).unwrap();

        // base haircut = 0, cadence target > 0 -> max picks cadence
        assert_eq!(out, 55);
    }
}
