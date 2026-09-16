//! v5 redemption flow: create redemption requests and track their status.
//!
//! Fulfillment is protocol-side (redemption_admin). The integrator surface is:
//! create a request (locks ONyc in the redemption vault) + read back its state.

use solana_pubkey::Pubkey;

use crate::constants::{ANCHOR_DISCRIMINATOR_LEN, REDEMPTION_OFFER_ACCOUNT_DISCRIMINATOR};
use crate::util::{read_pubkey, read_u128, read_u16, read_u64};

/// v5 RedemptionOffer account (borsh layout):
/// offer(32) token_in_mint(32) token_out_mint(32) executed_redemptions(16)
/// requested_redemptions(16) fee_basis_points(2) request_counter(8) bump(1)
/// vault_target_bps(2) disabled(1) fee_basis_points_prop_amm_sell(2) reserved(104)
#[derive(Copy, Clone, Debug)]
pub struct RedemptionOffer {
    pub offer: Pubkey,
    pub token_in_mint: Pubkey,
    pub token_out_mint: Pubkey,
    pub executed_redemptions: u128,
    pub requested_redemptions: u128,
    pub fee_basis_points: u16,
    pub request_counter: u64,
    pub bump: u8,
    pub vault_target_bps: u16,
    disabled: u8,
    pub fee_basis_points_prop_amm_sell: u16,
}

const REDEMPTION_OFFER_SERIALIZED_LEN: usize = 32 + 32 + 32 + 16 + 16 + 2 + 8 + 1 + 2 + 1 + 2 + 104;

impl RedemptionOffer {
    pub fn load(data: &[u8]) -> Option<Self> {
        if data.len() < ANCHOR_DISCRIMINATOR_LEN + REDEMPTION_OFFER_SERIALIZED_LEN {
            return None;
        }
        if data[..ANCHOR_DISCRIMINATOR_LEN] != REDEMPTION_OFFER_ACCOUNT_DISCRIMINATOR {
            return None;
        }
        let offer_data = &data[ANCHOR_DISCRIMINATOR_LEN..];

        Some(RedemptionOffer {
            offer: read_pubkey(offer_data, 0),
            token_in_mint: read_pubkey(offer_data, 32),
            token_out_mint: read_pubkey(offer_data, 64),
            executed_redemptions: read_u128(offer_data, 96),
            requested_redemptions: read_u128(offer_data, 112),
            fee_basis_points: read_u16(offer_data, 128),
            request_counter: read_u64(offer_data, 130),
            bump: offer_data[138],
            vault_target_bps: read_u16(offer_data, 139),
            disabled: offer_data[141],
            fee_basis_points_prop_amm_sell: read_u16(offer_data, 142),
        })
    }

    /// Whether the redemption offer is disabled (v5 `require_enabled`)
    pub fn is_disabled(&self) -> bool {
        self.disabled != 0
    }
}
