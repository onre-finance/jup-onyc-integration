#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct PriceVector {
    /// Unix timestamp when this vector becomes active.
    pub start_time: u64,
    /// Reference time from which price growth is calculated.
    pub base_time: u64,
    /// Starting price (9-decimal precision, see [`PRICE_DECIMALS`](crate::PRICE_DECIMALS)).
    pub base_price: u64,
    /// Annual percentage rate, scale=6 (see [`APR_SCALE`](crate::APR_SCALE)).
    pub apr: u64,
    /// Seconds per discrete pricing step.
    pub price_fix_duration: u64,
}

/// Sell-side dampening (hard-wall) parameters.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct DampeningState {
    /// Maximum haircut at full utilization.
    pub max_haircut_bps: u16,
    /// Curve exponent, scale=10_000 (e.g. 10_000 = linear, 20_000 = quadratic).
    pub exponent_scaled: u32,

    /// Sell trade count at which cadence wave reaches full ramp.
    pub cadence_threshold: u32,
    /// Max cadence wave amplitude, scale=10_000.
    pub cadence_wave_scaled: u32,

    /// How aggressively sell volume contracts the dynamic wall.
    pub wall_sensitivity_scaled: u32,

    pub epoch_start: u64,
    pub epoch_duration_seconds: u64,

    pub sell_volume: u64,
    pub buy_volume: u64,
    pub sell_trade_count: u32,
    /// Net sell volume from the previous epoch, linearly decayed into the current one.
    pub prev_net_sell_volume: u64,
}

/// Liquidity state passed into sell-side pricing.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct LiquidityParams {
    pub liquidity: u64,
    /// Effective liquidity ceiling - may be lower than `liquidity` when a TVL target is configured.
    pub liquidity_cap: u64,
    pub dampening: DampeningState,
}
