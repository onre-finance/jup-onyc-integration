//! Account state deserialization for OnRe protocol

use bytemuck::{Pod, Zeroable};
use solana_pubkey::Pubkey;

use crate::constants::{ANCHOR_DISCRIMINATOR_LEN, MAX_VECTORS};
use crate::errors::OnreError;

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
    _reserved: [u8; 130],
}

impl Offer {
    pub fn load(data: &[u8]) -> Result<Self, OnreError> {
        let expected_size = ANCHOR_DISCRIMINATOR_LEN + std::mem::size_of::<Offer>();
        if data.len() < expected_size {
            return Err(OnreError::DeserializationFailed(Pubkey::default()));
        }

        let offer_data = &data
            [ANCHOR_DISCRIMINATOR_LEN..ANCHOR_DISCRIMINATOR_LEN + std::mem::size_of::<Offer>()];
        bytemuck::try_from_bytes::<Offer>(offer_data)
            .map(|o| *o)
            .map_err(|_| OnreError::DeserializationFailed(Pubkey::default()))
    }

    pub fn needs_approval(&self) -> bool {
        self.needs_approval != 0
    }

    pub fn allow_permissionless(&self) -> bool {
        self.allow_permissionless != 0
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

const MAX_ADMINS: usize = 20;

/// OnRe State account structure
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct State {
    pub boss: Pubkey,
    pub proposed_boss: Pubkey,
    is_killed: u8,
    _pad1: [u8; 7],
    pub onyc_mint: Pubkey,
    pub admins: [Pubkey; MAX_ADMINS],
    pub approver1: Pubkey,
    pub approver2: Pubkey,
    pub bump: u8,
    _pad2: [u8; 7],
    pub max_supply: u64,
    _reserved: [u8; 128],
}

impl State {
    pub fn load(data: &[u8]) -> Result<Self, OnreError> {
        let expected_size = ANCHOR_DISCRIMINATOR_LEN + std::mem::size_of::<State>();
        if data.len() < expected_size {
            return Err(OnreError::DeserializationFailed(Pubkey::default()));
        }

        let state_data = &data
            [ANCHOR_DISCRIMINATOR_LEN..ANCHOR_DISCRIMINATOR_LEN + std::mem::size_of::<State>()];
        bytemuck::try_from_bytes::<State>(state_data)
            .map(|s| *s)
            .map_err(|_| OnreError::DeserializationFailed(Pubkey::default()))
    }

    pub fn is_killed(&self) -> bool {
        self.is_killed != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds raw account data for a v5 Offer (zero-copy layout).
    /// Layout after the 8-byte discriminator:
    /// token_in_mint(32) token_out_mint(32) vectors(10*40) fee_basis_points(2)
    /// bump(1) needs_approval(1) allow_permissionless(1) disabled(1) reserved(130)
    fn offer_account_data(allow_permissionless: u8, disabled: u8) -> Vec<u8> {
        let mut data = vec![0u8; 8 + 600];
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
