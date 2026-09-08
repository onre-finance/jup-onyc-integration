//! v5 redemption flow: create redemption requests and track their status.
//!
//! Fulfillment is protocol-side (redemption_admin). The integrator surface is:
//! create a request (locks ONyc in the redemption vault) + read back its state.

use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use spl_associated_token_account::get_associated_token_address_with_program_id;

use crate::constants::{
    ANCHOR_DISCRIMINATOR_LEN, ASSOCIATED_TOKEN_PROGRAM, CREATE_REDEMPTION_REQUEST_DISCRIMINATOR,
    ONRE_PROGRAM_ID, REDEMPTION_OFFER_ACCOUNT_DISCRIMINATOR,
    REDEMPTION_REQUEST_ACCOUNT_DISCRIMINATOR, SEED_OFFER, SEED_REDEMPTION_OFFER,
    SEED_REDEMPTION_OFFER_VAULT_AUTHORITY, SEED_REDEMPTION_REQUEST, SEED_STATE, SYSTEM_PROGRAM,
    TOKEN_PROGRAM,
};
use crate::errors::OnreError;
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
    pub fn load(data: &[u8]) -> Result<Self, OnreError> {
        if data.len() < ANCHOR_DISCRIMINATOR_LEN + REDEMPTION_OFFER_SERIALIZED_LEN {
            return Err(OnreError::DeserializationFailed(Pubkey::default()));
        }
        if data[..ANCHOR_DISCRIMINATOR_LEN] != REDEMPTION_OFFER_ACCOUNT_DISCRIMINATOR {
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
            fee_basis_points_prop_amm_sell: read_u16(d, 142),
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
        if data[..ANCHOR_DISCRIMINATOR_LEN] != REDEMPTION_REQUEST_ACCOUNT_DISCRIMINATOR {
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

/// Derives the RedemptionOffer PDA for a `token_in -> token_out` redemption
/// (token_in is the token being redeemed, i.e. ONyc).
pub fn find_redemption_offer_pda(token_in_mint: &Pubkey, token_out_mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            SEED_REDEMPTION_OFFER,
            token_in_mint.as_ref(),
            token_out_mint.as_ref(),
        ],
        &ONRE_PROGRAM_ID,
    )
}

/// Derives the RedemptionRequest PDA for a redemption offer + request id
/// (the offer's `request_counter` at creation time).
pub fn find_redemption_request_pda(redemption_offer: &Pubkey, request_id: u64) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            SEED_REDEMPTION_REQUEST,
            redemption_offer.as_ref(),
            &request_id.to_le_bytes(),
        ],
        &ONRE_PROGRAM_ID,
    )
}

/// Builds a v5 `create_redemption_request` instruction.
///
/// * `redemption_token_in_mint` - the token being redeemed (ONyc). The program
///   requires this mint to be owned by SPL Token (not Token-2022).
/// * `redemption_token_out_mint` - the token the redeemer will eventually
///   receive on fulfillment (e.g. USDC).
/// * `request_counter` - current `RedemptionOffer.request_counter` (fetch the
///   offer first; the counter seeds the new request PDA).
///
/// Fulfillment/cancellation are protocol-side; the integrator tracks the
/// request PDA with [`redemption_request_status`].
pub fn build_create_redemption_request_instruction(
    redeemer: &Pubkey,
    redemption_token_in_mint: &Pubkey,
    redemption_token_out_mint: &Pubkey,
    amount: u64,
    request_counter: u64,
) -> Instruction {
    let (state, _) = Pubkey::find_program_address(&[SEED_STATE], &ONRE_PROGRAM_ID);
    let (redemption_offer, _) =
        find_redemption_offer_pda(redemption_token_in_mint, redemption_token_out_mint);
    // The mint-side Offer runs in the opposite direction (token_out -> token_in)
    let (offer, _) = Pubkey::find_program_address(
        &[
            SEED_OFFER,
            redemption_token_out_mint.as_ref(),
            redemption_token_in_mint.as_ref(),
        ],
        &ONRE_PROGRAM_ID,
    );
    let (redemption_request, _) = find_redemption_request_pda(&redemption_offer, request_counter);
    let (vault_authority, _) =
        Pubkey::find_program_address(&[SEED_REDEMPTION_OFFER_VAULT_AUTHORITY], &ONRE_PROGRAM_ID);

    let redeemer_token_account = get_associated_token_address_with_program_id(
        redeemer,
        redemption_token_in_mint,
        &TOKEN_PROGRAM,
    );
    let vault_token_account = get_associated_token_address_with_program_id(
        &vault_authority,
        redemption_token_in_mint,
        &TOKEN_PROGRAM,
    );

    let mut data = Vec::with_capacity(16);
    data.extend_from_slice(&CREATE_REDEMPTION_REQUEST_DISCRIMINATOR);
    data.extend_from_slice(&amount.to_le_bytes());

    Instruction {
        program_id: ONRE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(state, false),
            AccountMeta::new(redemption_offer, false),
            AccountMeta::new_readonly(offer, false),
            AccountMeta::new(redemption_request, false),
            AccountMeta::new(*redeemer, true),
            AccountMeta::new_readonly(vault_authority, false),
            AccountMeta::new_readonly(*redemption_token_in_mint, false),
            AccountMeta::new(redeemer_token_account, false),
            AccountMeta::new(vault_token_account, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM, false),
            AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
        ],
        data,
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
        fee_basis_points_prop_amm_sell: u16,
    ) -> Vec<u8> {
        let mut d = vec![0u8; 8 + REDEMPTION_OFFER_SERIALIZED_LEN];
        d[..8].copy_from_slice(&REDEMPTION_OFFER_ACCOUNT_DISCRIMINATOR);
        d[8..40].copy_from_slice(offer.as_ref());
        d[8 + 112..8 + 128].copy_from_slice(&requested_redemptions.to_le_bytes());
        d[8 + 128..8 + 130].copy_from_slice(&50u16.to_le_bytes());
        d[8 + 130..8 + 138].copy_from_slice(&request_counter.to_le_bytes());
        d[8 + 138] = 254; // bump
        d[8 + 141] = disabled;
        d[8 + 142..8 + 144].copy_from_slice(&fee_basis_points_prop_amm_sell.to_le_bytes());
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
        d[..8].copy_from_slice(&REDEMPTION_REQUEST_ACCOUNT_DISCRIMINATOR);
        d[8..40].copy_from_slice(offer.as_ref());
        d[8 + 32..8 + 40].copy_from_slice(&request_id.to_le_bytes());
        d[8 + 40..8 + 72].copy_from_slice(redeemer.as_ref());
        d[8 + 72..8 + 80].copy_from_slice(&amount.to_le_bytes());
        d[8 + 81..8 + 89].copy_from_slice(&fulfilled_amount.to_le_bytes());
        d
    }

    #[test]
    fn test_create_redemption_request_instruction_layout() {
        use crate::constants::*;

        let redeemer = Pubkey::new_unique();
        let onyc_mint = Pubkey::new_unique(); // redemption token_in
        let usdc_mint = Pubkey::new_unique(); // redemption token_out
        let counter = 5u64;

        let ix = build_create_redemption_request_instruction(
            &redeemer, &onyc_mint, &usdc_mint, 750, counter,
        );

        assert_eq!(ix.program_id, ONRE_PROGRAM_ID);

        // data: discriminator (from v5 IDL) + u64 amount
        let mut expected = vec![201u8, 53, 181, 254, 115, 137, 70, 151];
        expected.extend_from_slice(&750u64.to_le_bytes());
        assert_eq!(ix.data, expected);

        // Independent derivations with literal seeds (from the v5 IDL)
        let pda = |seeds: &[&[u8]]| Pubkey::find_program_address(seeds, &ONRE_PROGRAM_ID).0;
        let ata = |owner: &Pubkey, mint: &Pubkey| {
            spl_associated_token_account::get_associated_token_address_with_program_id(
                owner,
                mint,
                &TOKEN_PROGRAM,
            )
        };

        let state = pda(&[b"state"]);
        let redemption_offer = pda(&[b"redemption_offer", onyc_mint.as_ref(), usdc_mint.as_ref()]);
        // The mint-side offer runs in the opposite direction: USDC -> ONyc
        let offer = pda(&[b"offer", usdc_mint.as_ref(), onyc_mint.as_ref()]);
        let request = pda(&[
            b"redemption_request",
            redemption_offer.as_ref(),
            &counter.to_le_bytes(),
        ]);
        let vault_authority = pda(&[b"redemption_offer_vault_authority"]);

        let expected: Vec<(Pubkey, bool, bool)> = vec![
            (state, false, false),
            (redemption_offer, true, false),
            (offer, false, false),
            (request, true, false),
            (redeemer, true, true),
            (vault_authority, false, false),
            (onyc_mint, false, false),
            (ata(&redeemer, &onyc_mint), true, false),
            (ata(&vault_authority, &onyc_mint), true, false),
            (TOKEN_PROGRAM, false, false),
            (ASSOCIATED_TOKEN_PROGRAM, false, false),
            (SYSTEM_PROGRAM, false, false),
        ];
        assert_eq!(ix.accounts.len(), expected.len());
        for (i, (key, writable, signer)) in expected.iter().enumerate() {
            assert_eq!(ix.accounts[i].pubkey, *key, "account {} pubkey mismatch", i);
            assert_eq!(
                ix.accounts[i].is_writable, *writable,
                "account {} writable mismatch",
                i
            );
            assert_eq!(
                ix.accounts[i].is_signer, *signer,
                "account {} signer mismatch",
                i
            );
        }
    }

    #[test]
    fn test_find_redemption_request_pda_matches_seed_derivation() {
        use crate::constants::ONRE_PROGRAM_ID;
        let redemption_offer = Pubkey::new_unique();
        let (pda, bump) = find_redemption_request_pda(&redemption_offer, 9);
        let (expected, expected_bump) = Pubkey::find_program_address(
            &[
                b"redemption_request",
                redemption_offer.as_ref(),
                &9u64.to_le_bytes(),
            ],
            &ONRE_PROGRAM_ID,
        );
        assert_eq!(pda, expected);
        assert_eq!(bump, expected_bump);
    }

    #[test]
    fn test_redemption_offer_parses_counter_and_disabled() {
        let offer = Pubkey::new_unique();
        let ro = RedemptionOffer::load(&redemption_offer_data(offer, 7, 1_000, 0, 250)).unwrap();
        assert_eq!(ro.offer, offer);
        assert_eq!(ro.request_counter, 7);
        assert_eq!(ro.requested_redemptions, 1_000);
        assert_eq!(ro.fee_basis_points, 50);
        assert_eq!(ro.fee_basis_points_prop_amm_sell, 250);
        assert!(!ro.is_disabled());

        let ro = RedemptionOffer::load(&redemption_offer_data(offer, 0, 0, 1, 0)).unwrap();
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
