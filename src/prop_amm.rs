//! PropAmmPairState deserialization for OnRe's v5 proprietary AMM.

use solana_pubkey::Pubkey;

use crate::constants::*;
use crate::errors::OnreError;
use crate::util::{read_i64, read_pubkey, read_u16, read_u32, read_u64};

pub const PROP_AMM_PAIR_STATE_RESERVED_BYTES: usize = 284;
// 3*32 + 1 + 2 + 4 + 4 + 4 + 8 + 4 + 8 + 8 + 8 + 8 + 4 + 8 + 1 + 284
const PROP_AMM_PAIR_STATE_SERIALIZED_LEN: usize = 452;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PropAmmPairState {
    pub offer: Pubkey,
    pub asset_mint: Pubkey,
    pub onyc_mint: Pubkey,
    pub enabled: bool,
    pub curve_peg_haircut_bps: u16,
    pub curve_exponent_scaled: u32,
    pub cadence_threshold: u32,
    pub cadence_wave_scaled: u32,
    pub epoch_duration_seconds: i64,
    pub wall_sensitivity_scaled: u32,
    pub minimum_sell_haircut_onyc: u64,
    pub curr_sell_value_stable: u64,
    pub curr_buy_value_stable: u64,
    pub prev_net_sell_value_stable: u64,
    pub curr_sell_trade_count: u32,
    pub epoch_start: i64,
    pub bump: u8,
    pub reserved: [u8; PROP_AMM_PAIR_STATE_RESERVED_BYTES],
}

impl PropAmmPairState {
    pub fn load(data: &[u8]) -> Result<Self, OnreError> {
        if data.len() < ANCHOR_DISCRIMINATOR_LEN + PROP_AMM_PAIR_STATE_SERIALIZED_LEN {
            return Err(OnreError::DeserializationFailed(Pubkey::default()));
        }
        if data[..ANCHOR_DISCRIMINATOR_LEN] != PROP_AMM_PAIR_STATE_ACCOUNT_DISCRIMINATOR {
            return Err(OnreError::DeserializationFailed(Pubkey::default()));
        }
        let d = &data[ANCHOR_DISCRIMINATOR_LEN..];

        let mut reserved = [0u8; PROP_AMM_PAIR_STATE_RESERVED_BYTES];
        reserved.copy_from_slice(&d[168..168 + PROP_AMM_PAIR_STATE_RESERVED_BYTES]);

        Ok(PropAmmPairState {
            offer: read_pubkey(d, 0),
            asset_mint: read_pubkey(d, 32),
            onyc_mint: read_pubkey(d, 64),
            enabled: read_bool(d, 96)?,
            curve_peg_haircut_bps: read_u16(d, 97),
            curve_exponent_scaled: read_u32(d, 99),
            cadence_threshold: read_u32(d, 103),
            cadence_wave_scaled: read_u32(d, 107),
            epoch_duration_seconds: read_i64(d, 111),
            wall_sensitivity_scaled: read_u32(d, 119),
            minimum_sell_haircut_onyc: read_u64(d, 123),
            curr_sell_value_stable: read_u64(d, 131),
            curr_buy_value_stable: read_u64(d, 139),
            prev_net_sell_value_stable: read_u64(d, 147),
            curr_sell_trade_count: read_u32(d, 155),
            epoch_start: read_i64(d, 159),
            bump: d[167],
            reserved,
        })
    }

    pub fn to_dampening_state(&self) -> onre_pricing::types::DampeningState {
        onre_pricing::types::DampeningState {
            max_haircut_bps: self.curve_peg_haircut_bps,
            exponent_scaled: self.curve_exponent_scaled,
            cadence_threshold: self.cadence_threshold,
            cadence_wave_scaled: self.cadence_wave_scaled,
            wall_sensitivity_scaled: self.wall_sensitivity_scaled,
            epoch_start: self.epoch_start as u64,
            epoch_duration_seconds: self.epoch_duration_seconds as u64,
            sell_volume: self.curr_sell_value_stable,
            buy_volume: self.curr_buy_value_stable,
            sell_trade_count: self.curr_sell_trade_count,
            prev_net_sell_volume: self.prev_net_sell_value_stable,
        }
    }

    /// `min_fee` argument for `onre_pricing::sell::calculate_amount_out`.
    pub fn min_sell_fee(&self) -> u64 {
        self.minimum_sell_haircut_onyc
    }

    pub fn is_disabled(&self) -> bool {
        !self.enabled
    }
}

fn read_bool(data: &[u8], offset: usize) -> Result<bool, OnreError> {
    match data.get(offset) {
        Some(&0) => Ok(false),
        Some(&1) => Ok(true),
        _ => Err(OnreError::DeserializationFailed(Pubkey::default())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serialized_pair_state(offer: Pubkey, asset: Pubkey, onyc: Pubkey) -> Vec<u8> {
        let mut d = PROP_AMM_PAIR_STATE_ACCOUNT_DISCRIMINATOR.to_vec();
        d.extend_from_slice(offer.as_ref());
        d.extend_from_slice(asset.as_ref());
        d.extend_from_slice(onyc.as_ref());
        d.push(1); // enabled
        d.extend_from_slice(&700u16.to_le_bytes()); // curve_peg_haircut_bps
        d.extend_from_slice(&25_000u32.to_le_bytes()); // curve_exponent_scaled
        d.extend_from_slice(&20u32.to_le_bytes()); // cadence_threshold
        d.extend_from_slice(&10_000u32.to_le_bytes()); // cadence_wave_scaled
        d.extend_from_slice(&86_400i64.to_le_bytes()); // epoch_duration_seconds
        d.extend_from_slice(&20_000u32.to_le_bytes()); // wall_sensitivity_scaled
        d.extend_from_slice(&5_000_000_000u64.to_le_bytes()); // minimum_sell_haircut_onyc
        d.extend_from_slice(&11u64.to_le_bytes()); // curr_sell_value_stable
        d.extend_from_slice(&22u64.to_le_bytes()); // curr_buy_value_stable
        d.extend_from_slice(&33u64.to_le_bytes()); // prev_net_sell_value_stable
        d.extend_from_slice(&4u32.to_le_bytes()); // curr_sell_trade_count
        d.extend_from_slice(&123i64.to_le_bytes()); // epoch_start
        d.push(254); // bump
        d.extend_from_slice(&[0u8; PROP_AMM_PAIR_STATE_RESERVED_BYTES]); // reserved
        assert_eq!(
            d.len(),
            ANCHOR_DISCRIMINATOR_LEN + PROP_AMM_PAIR_STATE_SERIALIZED_LEN
        );
        d
    }

    #[test]
    fn pair_state_load_matches_v5_layout() {
        let offer = Pubkey::new_unique();
        let asset = Pubkey::new_unique();
        let onyc = Pubkey::new_unique();
        let state = PropAmmPairState::load(&serialized_pair_state(offer, asset, onyc)).unwrap();

        assert_eq!(state.offer, offer);
        assert_eq!(state.asset_mint, asset);
        assert_eq!(state.onyc_mint, onyc);
        assert!(state.enabled);
        assert_eq!(state.curve_peg_haircut_bps, 700);
        assert_eq!(state.curve_exponent_scaled, 25_000);
        assert_eq!(state.cadence_threshold, 20);
        assert_eq!(state.cadence_wave_scaled, 10_000);
        assert_eq!(state.epoch_duration_seconds, 86_400);
        assert_eq!(state.wall_sensitivity_scaled, 20_000);
        assert_eq!(state.minimum_sell_haircut_onyc, 5_000_000_000);
        assert_eq!(state.curr_sell_value_stable, 11);
        assert_eq!(state.curr_buy_value_stable, 22);
        assert_eq!(state.prev_net_sell_value_stable, 33);
        assert_eq!(state.curr_sell_trade_count, 4);
        assert_eq!(state.epoch_start, 123);
        assert_eq!(state.bump, 254);
    }

    #[test]
    fn dampening_state_maps_from_pair_state() {
        let state = PropAmmPairState::load(&serialized_pair_state(
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        ))
        .unwrap();

        let dampening: onre_pricing::types::DampeningState = state.to_dampening_state();
        assert_eq!(dampening.max_haircut_bps, 700);
        assert_eq!(dampening.exponent_scaled, 25_000);
        assert_eq!(dampening.cadence_threshold, 20);
        assert_eq!(dampening.cadence_wave_scaled, 10_000);
        assert_eq!(dampening.wall_sensitivity_scaled, 20_000);
        assert_eq!(dampening.epoch_start, 123); // i64 -> u64
        assert_eq!(dampening.epoch_duration_seconds, 86_400); // i64 -> u64
        assert_eq!(dampening.sell_volume, 11);
        assert_eq!(dampening.buy_volume, 22);
        assert_eq!(dampening.sell_trade_count, 4);
        assert_eq!(dampening.prev_net_sell_volume, 33);

        assert_eq!(state.min_sell_fee(), 5_000_000_000);
    }

    #[test]
    fn pair_state_load_rejects_bad_input() {
        let data = serialized_pair_state(
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        );

        assert!(matches!(
            PropAmmPairState::load(&data[..data.len() - 1]),
            Err(OnreError::DeserializationFailed(_))
        ));

        let mut wrong_disc = data.clone();
        wrong_disc[0] ^= 0xff;
        assert!(matches!(
            PropAmmPairState::load(&wrong_disc),
            Err(OnreError::DeserializationFailed(_))
        ));
    }
}
