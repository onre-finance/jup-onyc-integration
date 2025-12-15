//! Account state deserialization for OnRe protocol

use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use solana_sdk::pubkey::Pubkey;

use crate::constants::{ANCHOR_DISCRIMINATOR_LEN, MAX_VECTORS};
use crate::errors::OnreAmmError;

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
    _reserved1: [u8; 128],
    _reserved2: [u8; 3],
}

impl Offer {
    pub fn load(data: &[u8]) -> Result<Self> {
        let expected_size = ANCHOR_DISCRIMINATOR_LEN + std::mem::size_of::<Offer>();
        if data.len() < expected_size {
            return Err(OnreAmmError::DeserializationError(format!(
                "Offer data too short: {} < {}",
                data.len(),
                expected_size
            ))
            .into());
        }

        let offer_data = &data
            [ANCHOR_DISCRIMINATOR_LEN..ANCHOR_DISCRIMINATOR_LEN + std::mem::size_of::<Offer>()];
        bytemuck::try_from_bytes::<Offer>(offer_data)
            .map(|o| *o)
            .map_err(|e| OnreAmmError::DeserializationError(format!("Offer: {:?}", e)).into())
    }

    pub fn needs_approval(&self) -> bool {
        self.needs_approval != 0
    }

    pub fn allow_permissionless(&self) -> bool {
        self.allow_permissionless != 0
    }
}

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
    pub fn load(data: &[u8]) -> Result<Self> {
        let expected_size = ANCHOR_DISCRIMINATOR_LEN + std::mem::size_of::<State>();
        if data.len() < expected_size {
            return Err(OnreAmmError::DeserializationError(format!(
                "State data too short: {} < {}",
                data.len(),
                expected_size
            ))
            .into());
        }

        let state_data = &data
            [ANCHOR_DISCRIMINATOR_LEN..ANCHOR_DISCRIMINATOR_LEN + std::mem::size_of::<State>()];
        bytemuck::try_from_bytes::<State>(state_data)
            .map(|s| *s)
            .map_err(|e| OnreAmmError::DeserializationError(format!("State: {:?}", e)).into())
    }

    pub fn is_killed(&self) -> bool {
        self.is_killed != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
