//! Account state deserialization for OnRe protocol

use crate::constants::{
    ANCHOR_DISCRIMINATOR_LEN, CIRCULATING_SUPPLY_EXCLUDED_BALANCE_ACCOUNT_DISCRIMINATOR,
    OFFER_ACCOUNT_DISCRIMINATOR, STATE_ACCOUNT_DISCRIMINATOR,
};
use crate::errors::OnreError;
use crate::util::{read_i64, read_pubkey, read_u64};
use bytemuck::{Pod, Zeroable};
use onre_pricing::MAX_VECTORS;
use solana_pubkey::Pubkey;

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Offer {
    pub token_in_mint: Pubkey,
    pub token_out_mint: Pubkey,
    pub vectors: [OfferVector; MAX_VECTORS],
    pub fee_basis_points: u16,
    pub bump: u8,
    needs_approval: u8,
    allow_permissionless: u8,
    disabled: u8,
    pub fee_basis_points_permissionless: u16,
    _reserved: [u8; 128],
}

impl Offer {
    pub fn load(data: &[u8]) -> Result<Self, OnreError> {
        let expected_size = ANCHOR_DISCRIMINATOR_LEN + std::mem::size_of::<Offer>();
        if data.len() < expected_size {
            return Err(OnreError::DeserializationFailed(Pubkey::default()));
        }
        if data[..ANCHOR_DISCRIMINATOR_LEN] != OFFER_ACCOUNT_DISCRIMINATOR {
            return Err(OnreError::DeserializationFailed(Pubkey::default()));
        }

        let offer_data = &data
            [ANCHOR_DISCRIMINATOR_LEN..ANCHOR_DISCRIMINATOR_LEN + std::mem::size_of::<Offer>()];
        bytemuck::try_from_bytes::<Offer>(offer_data)
            .copied()
            .map_err(|_| OnreError::DeserializationFailed(Pubkey::default()))
    }

    pub fn needs_approval(&self) -> bool {
        self.needs_approval != 0
    }

    pub fn allow_permissionless(&self) -> bool {
        self.allow_permissionless != 0
    }

    pub fn permissionless_fee_basis_points(&self) -> u16 {
        self.fee_basis_points_permissionless
    }

    /// Whether the offer is disabled by emergency controls (v5 `require_enabled`)
    pub fn is_disabled(&self) -> bool {
        self.disabled != 0
    }
}

/// Pricing vector for time-based price evolution
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Pod, Zeroable)]
pub struct OfferVector {
    pub start_time: u64,
    pub base_time: u64,
    pub base_price: u64,
    pub apr: u64,
    pub price_fix_duration: u64,
}

impl From<OfferVector> for onre_pricing::PriceVector {
    fn from(value: OfferVector) -> Self {
        onre_pricing::PriceVector {
            start_time: value.start_time,
            base_time: value.base_time,
            base_price: value.base_price,
            apr: value.apr,
            price_fix_duration: value.price_fix_duration,
        }
    }
}

const MAX_ADMINS: usize = 20;

/// OnRe v5 State account structure.
///
/// The v5 program serializes State with borsh (`#[account]`, not zero-copy),
/// so the layout is packed with no alignment padding. Parsed manually to stay
/// defensive (no panics on malformed data).
#[derive(Copy, Clone, Debug)]
pub struct State {
    pub boss: Pubkey,
    pub proposed_boss: Pubkey,
    is_killed: u8,
    pub onyc_mint: Pubkey,
    pub admins: [Pubkey; MAX_ADMINS],
    pub approver1: Pubkey,
    pub approver2: Pubkey,
    pub bump: u8,
    pub max_supply: u64,
    pub redemption_admin: Pubkey,
    pub max_mint_amount: u64,
    pub main_offer: Pubkey,
}

/// Serialized size of the v5 State payload (without the 8-byte discriminator).
const STATE_SERIALIZED_LEN: usize =
    32 + 32 + 1 + 32 + MAX_ADMINS * 32 + 32 + 32 + 1 + 8 + 32 + 8 + 32 + 56;

impl State {
    pub fn load(data: &[u8]) -> Result<Self, OnreError> {
        let expected_size = ANCHOR_DISCRIMINATOR_LEN + STATE_SERIALIZED_LEN;
        if data.len() < expected_size {
            return Err(OnreError::DeserializationFailed(Pubkey::default()));
        }
        if data[..ANCHOR_DISCRIMINATOR_LEN] != STATE_ACCOUNT_DISCRIMINATOR {
            return Err(OnreError::DeserializationFailed(Pubkey::default()));
        }
        let d = &data[ANCHOR_DISCRIMINATOR_LEN..];

        let mut admins = [Pubkey::default(); MAX_ADMINS];
        for (i, admin) in admins.iter_mut().enumerate() {
            *admin = read_pubkey(d, 97 + i * 32);
        }

        Ok(State {
            boss: read_pubkey(d, 0),
            proposed_boss: read_pubkey(d, 32),
            is_killed: d[64],
            onyc_mint: read_pubkey(d, 65),
            admins,
            approver1: read_pubkey(d, 737),
            approver2: read_pubkey(d, 769),
            bump: d[801],
            max_supply: read_u64(d, 802),
            redemption_admin: read_pubkey(d, 810),
            max_mint_amount: read_u64(d, 842),
            main_offer: read_pubkey(d, 850),
        })
    }

    pub fn is_killed(&self) -> bool {
        self.is_killed != 0
    }
}

#[derive(Copy, Clone, Debug)]
pub struct CirculatingSupplyExcludedBalance {
    pub amount: u64,
    pub last_updated_at: i64,
    pub last_updated_slot: u64,
    pub bump: u8,
    pub reserved: [u8; 31],
}

const CIRCULATING_SUPPLY_EXCLUDED_BALANCE_SERIALIZED_LEN: usize = 8 + 8 + 8 + 1 + 31;

impl CirculatingSupplyExcludedBalance {
    pub fn load(data: &[u8]) -> Result<Self, OnreError> {
        let expected_size =
            ANCHOR_DISCRIMINATOR_LEN + CIRCULATING_SUPPLY_EXCLUDED_BALANCE_SERIALIZED_LEN;
        if data.len() < expected_size {
            return Err(OnreError::DeserializationFailed(Pubkey::default()));
        }
        if data[..ANCHOR_DISCRIMINATOR_LEN]
            != CIRCULATING_SUPPLY_EXCLUDED_BALANCE_ACCOUNT_DISCRIMINATOR
        {
            return Err(OnreError::DeserializationFailed(Pubkey::default()));
        }
        let d = &data[ANCHOR_DISCRIMINATOR_LEN..];

        let mut reserved = [0u8; 31];
        reserved.copy_from_slice(&d[25..56]);

        Ok(Self {
            amount: read_u64(d, 0),
            last_updated_at: read_i64(d, 8),
            last_updated_slot: read_u64(d, 16),
            bump: d[24],
            reserved,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds raw account data for a v5 Offer (zero-copy layout).
    /// Layout after the 8-byte discriminator:
    /// token_in_mint(32) token_out_mint(32) vectors(10*40) fee_basis_points(2)
    /// bump(1) needs_approval(1) allow_permissionless(1) disabled(1)
    /// fee_basis_points_permissionless(2) reserved(128)
    fn offer_account_data(allow_permissionless: u8, disabled: u8) -> Vec<u8> {
        let mut data = vec![0u8; 8 + 600];
        data[..8].copy_from_slice(&OFFER_ACCOUNT_DISCRIMINATOR);
        data[8 + 466] = 1; // bump
        data[8 + 468] = allow_permissionless;
        data[8 + 469] = disabled;
        data
    }

    #[test]
    fn test_offer_load_reads_v5_disabled_flag() {
        let data = offer_account_data(1, 1);
        let offer = Offer::load(&data).unwrap();
        assert!(offer.is_disabled());
        assert!(offer.allow_permissionless());

        let data = offer_account_data(1, 0);
        let offer = Offer::load(&data).unwrap();
        assert!(!offer.is_disabled());
    }

    /// Builds raw account data for the v5 State account (borsh layout, no padding):
    /// boss(32) proposed_boss(32) is_killed(1) onyc_mint(32) admins(20*32)
    /// approver1(32) approver2(32) bump(1) max_supply(8) redemption_admin(32)
    /// max_mint_amount(8) main_offer(32) reserved(56)
    fn state_account_data(
        boss: Pubkey,
        is_killed: bool,
        onyc_mint: Pubkey,
        max_supply: u64,
        redemption_admin: Pubkey,
        main_offer: Pubkey,
    ) -> Vec<u8> {
        let mut data = vec![0u8; 8 + 938];
        data[..8].copy_from_slice(&STATE_ACCOUNT_DISCRIMINATOR);
        data[8..40].copy_from_slice(boss.as_ref());
        data[72] = is_killed as u8;
        data[73..105].copy_from_slice(onyc_mint.as_ref());
        data[8 + 801] = 255; // bump
        data[8 + 802..8 + 810].copy_from_slice(&max_supply.to_le_bytes());
        data[8 + 810..8 + 842].copy_from_slice(redemption_admin.as_ref());
        data[8 + 850..8 + 882].copy_from_slice(main_offer.as_ref());
        data
    }

    #[test]
    fn test_state_load_parses_v5_borsh_layout() {
        let boss = Pubkey::new_unique();
        let onyc_mint = Pubkey::new_unique();
        let redemption_admin = Pubkey::new_unique();
        let main_offer = Pubkey::new_unique();

        let data = state_account_data(boss, true, onyc_mint, 42, redemption_admin, main_offer);
        let state = State::load(&data).unwrap();

        assert_eq!(state.boss, boss);
        assert!(state.is_killed());
        assert_eq!(state.onyc_mint, onyc_mint);
        assert_eq!(state.bump, 255);
        assert_eq!(state.max_supply, 42);
        assert_eq!(state.redemption_admin, redemption_admin);
        assert_eq!(state.main_offer, main_offer);
    }

    #[test]
    fn test_state_load_rejects_truncated_data() {
        assert!(State::load(&[0u8; 100]).is_err());
    }

    #[test]
    fn test_offer_size() {
        let size = std::mem::size_of::<Offer>();
        println!("Offer struct size: {} bytes", size);

        let vector_size = std::mem::size_of::<OfferVector>();
        println!("OfferVector size: {} bytes", vector_size);

        // OfferVector should be 5 * 8 = 40 bytes
        assert_eq!(vector_size, 40);
    }

    #[test]
    fn test_state_size() {
        let size = std::mem::size_of::<State>();
        println!("State struct size: {} bytes", size);
    }

    #[test]
    fn test_offer_vector_default() {
        let v = OfferVector::default();
        assert_eq!(v.start_time, 0);
        assert_eq!(v.base_time, 0);
        assert_eq!(v.base_price, 0);
        assert_eq!(v.apr, 0);
        assert_eq!(v.price_fix_duration, 0);
    }
}
