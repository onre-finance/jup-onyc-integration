//! v5 redemption flow: create redemption requests and track their status.
//!
//! Fulfillment is protocol-side (redemption_admin). The integrator surface is:
//! create a request (locks ONyc in the redemption vault) + read back its state.

use solana_pubkey::Pubkey;

use crate::constants::ANCHOR_DISCRIMINATOR_LEN;
use crate::errors::OnreError;

fn read_pubkey(data: &[u8], offset: usize) -> Pubkey {
    let mut buf = [0u8; 32];
    buf.copy_from_slice(&data[offset..offset + 32]);
    Pubkey::new_from_array(buf)
}

fn read_u64(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
}

fn read_u128(data: &[u8], offset: usize) -> u128 {
    u128::from_le_bytes(data[offset..offset + 16].try_into().unwrap())
}

fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap())
}

/// v5 RedemptionOffer account (borsh layout):
/// offer(32) token_in_mint(32) token_out_mint(32) executed_redemptions(16)
/// requested_redemptions(16) fee_basis_points(2) request_counter(8) bump(1)
/// vault_target_bps(2) disabled(1) reserved(106)
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
}

const REDEMPTION_OFFER_SERIALIZED_LEN: usize = 32 + 32 + 32 + 16 + 16 + 2 + 8 + 1 + 2 + 1 + 106;

impl RedemptionOffer {
    pub fn load(data: &[u8]) -> Result<Self, OnreError> {
        if data.len() < ANCHOR_DISCRIMINATOR_LEN + REDEMPTION_OFFER_SERIALIZED_LEN {
            return Err(OnreError::DeserializationFailed(Pubkey::default()));
        }
        let d = &data[ANCHOR_DISCRIMINATOR_LEN..];
        Ok(RedemptionOffer {
            offer: read_pubkey(d, 0),
            token_in_mint: read_pubkey(d, 32),
            token_out_mint: read_pubkey(d, 64),
            executed_redemptions: read_u128(d, 96),
            requested_redemptions: read_u128(d, 112),
            fee_basis_points: read_u16(d, 128),
            request_counter: read_u64(d, 130),
            bump: d[138],
            vault_target_bps: read_u16(d, 139),
            disabled: d[141],
        })
    }

    /// Whether the redemption offer is disabled (v5 `require_enabled`)
    pub fn is_disabled(&self) -> bool {
        self.disabled != 0
    }
}

/// v5 RedemptionRequest account (borsh layout):
/// offer(32) request_id(8) redeemer(32) amount(8) bump(1)
/// fulfilled_amount(8) reserved(119)
#[derive(Copy, Clone, Debug)]
pub struct RedemptionRequest {
    pub offer: Pubkey,
    pub request_id: u64,
    pub redeemer: Pubkey,
    pub amount: u64,
    pub bump: u8,
    pub fulfilled_amount: u64,
}

const REDEMPTION_REQUEST_SERIALIZED_LEN: usize = 32 + 8 + 32 + 8 + 1 + 8 + 119;

impl RedemptionRequest {
    pub fn load(data: &[u8]) -> Result<Self, OnreError> {
        if data.len() < ANCHOR_DISCRIMINATOR_LEN + REDEMPTION_REQUEST_SERIALIZED_LEN {
            return Err(OnreError::DeserializationFailed(Pubkey::default()));
        }
        let d = &data[ANCHOR_DISCRIMINATOR_LEN..];
        Ok(RedemptionRequest {
            offer: read_pubkey(d, 0),
            request_id: read_u64(d, 32),
            redeemer: read_pubkey(d, 40),
            amount: read_u64(d, 72),
            bump: d[80],
            fulfilled_amount: read_u64(d, 81),
        })
    }

    pub fn status(&self) -> RedemptionRequestStatus {
        if self.fulfilled_amount == 0 {
            RedemptionRequestStatus::Pending
        } else {
            RedemptionRequestStatus::PartiallyFulfilled {
                fulfilled: self.fulfilled_amount,
                total: self.amount,
            }
        }
    }
}

/// Integrator-facing status of a redemption request.
///
/// The program closes the request account when it is fully fulfilled or
/// cancelled, so a missing account for a known request PDA means Closed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RedemptionRequestStatus {
    /// Request exists, nothing fulfilled yet
    Pending,
    /// Request exists with a partial fill (account stays open until fully settled)
    PartiallyFulfilled { fulfilled: u64, total: u64 },
    /// Account no longer exists: fully fulfilled or cancelled
    Closed,
}

/// Resolves a request status from optionally-fetched account data.
/// `None` (account missing) means the request was fully settled or cancelled.
pub fn redemption_request_status(
    account_data: Option<&[u8]>,
) -> Result<RedemptionRequestStatus, OnreError> {
    match account_data {
        None => Ok(RedemptionRequestStatus::Closed),
        Some(data) => Ok(RedemptionRequest::load(data)?.status()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redemption_offer_data(
        offer: Pubkey,
        request_counter: u64,
        requested_redemptions: u128,
        disabled: u8,
    ) -> Vec<u8> {
        let mut d = vec![0u8; 8 + REDEMPTION_OFFER_SERIALIZED_LEN];
        d[8..40].copy_from_slice(offer.as_ref());
        d[8 + 112..8 + 128].copy_from_slice(&requested_redemptions.to_le_bytes());
        d[8 + 128..8 + 130].copy_from_slice(&50u16.to_le_bytes());
        d[8 + 130..8 + 138].copy_from_slice(&request_counter.to_le_bytes());
        d[8 + 138] = 254; // bump
        d[8 + 141] = disabled;
        d
    }

    fn redemption_request_data(
        offer: Pubkey,
        request_id: u64,
        redeemer: Pubkey,
        amount: u64,
        fulfilled_amount: u64,
    ) -> Vec<u8> {
        let mut d = vec![0u8; 8 + REDEMPTION_REQUEST_SERIALIZED_LEN];
        d[8..40].copy_from_slice(offer.as_ref());
        d[8 + 32..8 + 40].copy_from_slice(&request_id.to_le_bytes());
        d[8 + 40..8 + 72].copy_from_slice(redeemer.as_ref());
        d[8 + 72..8 + 80].copy_from_slice(&amount.to_le_bytes());
        d[8 + 81..8 + 89].copy_from_slice(&fulfilled_amount.to_le_bytes());
        d
    }

    #[test]
    fn test_redemption_offer_parses_counter_and_disabled() {
        let offer = Pubkey::new_unique();
        let ro = RedemptionOffer::load(&redemption_offer_data(offer, 7, 1_000, 0)).unwrap();
        assert_eq!(ro.offer, offer);
        assert_eq!(ro.request_counter, 7);
        assert_eq!(ro.requested_redemptions, 1_000);
        assert_eq!(ro.fee_basis_points, 50);
        assert!(!ro.is_disabled());

        let ro = RedemptionOffer::load(&redemption_offer_data(offer, 0, 0, 1)).unwrap();
        assert!(ro.is_disabled());
    }

    #[test]
    fn test_redemption_offer_rejects_truncated_data() {
        assert!(RedemptionOffer::load(&[0u8; 64]).is_err());
    }

    #[test]
    fn test_redemption_request_parses_fields() {
        let offer = Pubkey::new_unique();
        let redeemer = Pubkey::new_unique();
        let req =
            RedemptionRequest::load(&redemption_request_data(offer, 3, redeemer, 500, 0)).unwrap();
        assert_eq!(req.offer, offer);
        assert_eq!(req.request_id, 3);
        assert_eq!(req.redeemer, redeemer);
        assert_eq!(req.amount, 500);
        assert_eq!(req.fulfilled_amount, 0);
    }

    #[test]
    fn test_redemption_request_status_transitions() {
        let offer = Pubkey::new_unique();
        let redeemer = Pubkey::new_unique();

        let pending = redemption_request_data(offer, 0, redeemer, 500, 0);
        assert_eq!(
            redemption_request_status(Some(&pending)).unwrap(),
            RedemptionRequestStatus::Pending
        );

        let partial = redemption_request_data(offer, 0, redeemer, 500, 200);
        assert_eq!(
            redemption_request_status(Some(&partial)).unwrap(),
            RedemptionRequestStatus::PartiallyFulfilled {
                fulfilled: 200,
                total: 500
            }
        );

        assert_eq!(
            redemption_request_status(None).unwrap(),
            RedemptionRequestStatus::Closed
        );
    }
}
