//! Atomic ONyc sell primitives for OnRe's v5 proprietary AMM.
//!
//! A router simulates [`build_quote_swap_sell_instruction`], validates the
//! return data with [`parse_swap_sell_quote`], and executes
//! [`build_open_swap_sell_instruction`] with the returned `minimum_out`.

use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use spl_associated_token_account::get_associated_token_address_with_program_id;

use crate::constants::*;
use crate::errors::OnreError;

/// Exact Borsh-serialized size of the v5 `SwapQuote` return value.
pub const SWAP_QUOTE_SERIALIZED_LEN: usize = 152;

/// A validated quote returned by `quote_swap_sell`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SwapQuote {
    pub offer: Pubkey,
    pub token_in_mint: Pubkey,
    pub token_out_mint: Pubkey,
    pub token_in_amount: u64,
    pub token_in_net_amount: u64,
    pub token_in_fee_amount: u64,
    pub token_out_amount: u64,
    pub minimum_out: u64,
    pub current_price: u64,
    pub quoted_at: i64,
}

fn pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &ONRE_PROGRAM_ID).0
}

fn ata(owner: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    get_associated_token_address_with_program_id(owner, mint, token_program)
}

fn require_token_program(program: &Pubkey) -> Result<(), OnreError> {
    if *program == TOKEN_PROGRAM || *program == TOKEN_22_PROGRAM {
        Ok(())
    } else {
        Err(OnreError::UnsupportedTokenProgram(*program))
    }
}

fn read_pubkey(data: &[u8], offset: usize) -> Pubkey {
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&data[offset..offset + 32]);
    Pubkey::new_from_array(bytes)
}

fn read_u64(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(
        data[offset..offset + 8]
            .try_into()
            .expect("validated quote length"),
    )
}

fn read_i64(data: &[u8], offset: usize) -> i64 {
    i64::from_le_bytes(
        data[offset..offset + 8]
            .try_into()
            .expect("validated quote length"),
    )
}

/// Build the read-only on-chain quote instruction for an atomic ONyc -> asset
/// sell. The router must simulate it and retain the return-data program id and
/// bytes.
pub fn build_quote_swap_sell_instruction(
    onyc_mint: &Pubkey,
    asset_mint: &Pubkey,
    asset_token_program: &Pubkey,
    token_in_amount: u64,
) -> Result<Instruction, OnreError> {
    require_token_program(asset_token_program)?;

    let offer = pda(&[SEED_OFFER, asset_mint.as_ref(), onyc_mint.as_ref()]);
    let pair_state = pda(&[SEED_PROP_AMM_PAIR_STATE, offer.as_ref()]);
    let redemption_offer = pda(&[
        SEED_REDEMPTION_OFFER,
        onyc_mint.as_ref(),
        asset_mint.as_ref(),
    ]);
    let state = pda(&[SEED_STATE]);
    let redemption_vault_authority = pda(&[SEED_REDEMPTION_OFFER_VAULT_AUTHORITY]);
    let redemption_vault_asset = ata(&redemption_vault_authority, asset_mint, asset_token_program);
    let market_stats = pda(&[SEED_MARKET_STATS]);

    let mut data = Vec::with_capacity(16);
    data.extend_from_slice(&QUOTE_SWAP_SELL_DISCRIMINATOR);
    data.extend_from_slice(&token_in_amount.to_le_bytes());

    Ok(Instruction {
        program_id: ONRE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(offer, false),
            AccountMeta::new_readonly(pair_state, false),
            AccountMeta::new_readonly(redemption_offer, false),
            AccountMeta::new_readonly(state, false),
            AccountMeta::new_readonly(redemption_vault_authority, false),
            AccountMeta::new_readonly(redemption_vault_asset, false),
            AccountMeta::new_readonly(*onyc_mint, false),
            AccountMeta::new_readonly(*asset_mint, false),
            AccountMeta::new_readonly(*asset_token_program, false),
            AccountMeta::new_readonly(market_stats, false),
        ],
        data,
    })
}

/// Parse and bind simulated return data to the exact sell request.
///
/// `quoted_at` is informational. Execution recomputes the price and enforces
/// `minimum_out` on chain.
pub fn parse_swap_sell_quote(
    return_data_program: &Pubkey,
    data: &[u8],
    onyc_mint: &Pubkey,
    asset_mint: &Pubkey,
    token_in_amount: u64,
) -> Result<SwapQuote, OnreError> {
    if *return_data_program != ONRE_PROGRAM_ID {
        return Err(OnreError::InvalidQuoteProgram {
            expected: ONRE_PROGRAM_ID,
            actual: *return_data_program,
        });
    }
    if data.len() != SWAP_QUOTE_SERIALIZED_LEN {
        return Err(OnreError::InvalidQuoteLength {
            expected: SWAP_QUOTE_SERIALIZED_LEN,
            actual: data.len(),
        });
    }

    let quote = SwapQuote {
        offer: read_pubkey(data, 0),
        token_in_mint: read_pubkey(data, 32),
        token_out_mint: read_pubkey(data, 64),
        token_in_amount: read_u64(data, 96),
        token_in_net_amount: read_u64(data, 104),
        token_in_fee_amount: read_u64(data, 112),
        token_out_amount: read_u64(data, 120),
        minimum_out: read_u64(data, 128),
        current_price: read_u64(data, 136),
        quoted_at: read_i64(data, 144),
    };
    let expected_offer = pda(&[SEED_OFFER, asset_mint.as_ref(), onyc_mint.as_ref()]);
    if quote.offer != expected_offer
        || quote.token_in_mint != *onyc_mint
        || quote.token_out_mint != *asset_mint
        || quote.token_in_amount != token_in_amount
    {
        return Err(OnreError::QuoteMismatch);
    }
    if token_in_amount > 0 && quote.minimum_out == 0 {
        return Err(OnreError::InvalidMinimumOut);
    }

    Ok(quote)
}

/// Build atomic ONyc -> asset execution with the quote's on-chain slippage
/// guard. Callers must not replace `minimum_out` with zero.
#[allow(clippy::too_many_arguments)]
pub fn build_open_swap_sell_instruction(
    user: &Pubkey,
    onyc_mint: &Pubkey,
    asset_mint: &Pubkey,
    onyc_token_program: &Pubkey,
    asset_token_program: &Pubkey,
    state_main_offer: &Pubkey,
    quote: &SwapQuote,
) -> Result<Instruction, OnreError> {
    require_token_program(onyc_token_program)?;
    require_token_program(asset_token_program)?;

    let offer = pda(&[SEED_OFFER, asset_mint.as_ref(), onyc_mint.as_ref()]);
    if quote.offer != offer
        || quote.token_in_mint != *onyc_mint
        || quote.token_out_mint != *asset_mint
    {
        return Err(OnreError::QuoteMismatch);
    }
    if quote.token_in_amount > 0 && quote.minimum_out == 0 {
        return Err(OnreError::InvalidMinimumOut);
    }

    let pair_state = pda(&[SEED_PROP_AMM_PAIR_STATE, offer.as_ref()]);
    let redemption_offer = pda(&[
        SEED_REDEMPTION_OFFER,
        onyc_mint.as_ref(),
        asset_mint.as_ref(),
    ]);
    let state = pda(&[SEED_STATE]);
    let offer_vault_authority = pda(&[SEED_OFFER_VAULT_AUTHORITY]);
    let redemption_vault_authority = pda(&[SEED_REDEMPTION_OFFER_VAULT_AUTHORITY]);
    let prop_amm_proceeds_vault = pda(&[SEED_CONFIGURABLE_VAULT, SEED_PROP_AMM_PROCEEDS_VAULT]);
    let prop_amm_sell_fee_vault = pda(&[SEED_CONFIGURABLE_VAULT, SEED_PROP_AMM_SELL_FEE_VAULT]);
    let mint_authority = pda(&[SEED_MINT_AUTHORITY]);
    let buffer_state = pda(&[SEED_BUFFER_STATE]);
    let reserve_vault_authority = pda(&[SEED_RESERVE_VAULT_AUTHORITY]);
    let management_fee_vault = pda(&[SEED_CONFIGURABLE_VAULT, SEED_MANAGEMENT_FEE_VAULT]);
    let performance_fee_vault = pda(&[SEED_CONFIGURABLE_VAULT, SEED_PERFORMANCE_FEE_VAULT]);
    let market_stats = pda(&[SEED_MARKET_STATS]);
    let excluded_balance = pda(&[SEED_CIRCULATING_SUPPLY_EXCLUDED_BALANCE]);

    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&OPEN_SWAP_SELL_DISCRIMINATOR);
    data.extend_from_slice(&quote.token_in_amount.to_le_bytes());
    data.extend_from_slice(&quote.minimum_out.to_le_bytes());

    Ok(Instruction {
        program_id: ONRE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(offer, false),
            AccountMeta::new(pair_state, false),
            AccountMeta::new_readonly(redemption_offer, false),
            AccountMeta::new_readonly(state, false),
            AccountMeta::new_readonly(offer_vault_authority, false),
            AccountMeta::new_readonly(redemption_vault_authority, false),
            AccountMeta::new(
                ata(&redemption_vault_authority, onyc_mint, onyc_token_program),
                false,
            ),
            AccountMeta::new(
                ata(&redemption_vault_authority, asset_mint, asset_token_program),
                false,
            ),
            AccountMeta::new(*onyc_mint, false),
            AccountMeta::new_readonly(*onyc_token_program, false),
            AccountMeta::new(*asset_mint, false),
            AccountMeta::new_readonly(*asset_token_program, false),
            AccountMeta::new(ata(user, onyc_mint, onyc_token_program), false),
            AccountMeta::new(ata(user, asset_mint, asset_token_program), false),
            AccountMeta::new(prop_amm_proceeds_vault, false),
            AccountMeta::new(
                ata(&prop_amm_proceeds_vault, onyc_mint, onyc_token_program),
                false,
            ),
            AccountMeta::new(prop_amm_sell_fee_vault, false),
            AccountMeta::new(
                ata(&prop_amm_sell_fee_vault, onyc_mint, onyc_token_program),
                false,
            ),
            AccountMeta::new_readonly(mint_authority, false),
            AccountMeta::new(buffer_state, false),
            AccountMeta::new(
                ata(&reserve_vault_authority, onyc_mint, onyc_token_program),
                false,
            ),
            AccountMeta::new(
                ata(&management_fee_vault, onyc_mint, onyc_token_program),
                false,
            ),
            AccountMeta::new(
                ata(&performance_fee_vault, onyc_mint, onyc_token_program),
                false,
            ),
            AccountMeta::new(market_stats, false),
            AccountMeta::new_readonly(excluded_balance, false),
            AccountMeta::new_readonly(SYSVAR_INSTRUCTIONS, false),
            AccountMeta::new(*user, true),
            AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
            AccountMeta::new_readonly(*state_main_offer, false),
            AccountMeta::new_readonly(
                ata(&offer_vault_authority, onyc_mint, onyc_token_program),
                false,
            ),
        ],
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serialized_quote(
        offer: Pubkey,
        onyc_mint: Pubkey,
        asset_mint: Pubkey,
        input: u64,
        minimum_out: u64,
    ) -> Vec<u8> {
        let mut data = Vec::with_capacity(SWAP_QUOTE_SERIALIZED_LEN);
        data.extend_from_slice(offer.as_ref());
        data.extend_from_slice(onyc_mint.as_ref());
        data.extend_from_slice(asset_mint.as_ref());
        data.extend_from_slice(&input.to_le_bytes());
        data.extend_from_slice(&(input - 20).to_le_bytes());
        data.extend_from_slice(&20u64.to_le_bytes());
        data.extend_from_slice(&(minimum_out + 10).to_le_bytes());
        data.extend_from_slice(&minimum_out.to_le_bytes());
        data.extend_from_slice(&1_010_000_000u64.to_le_bytes());
        data.extend_from_slice(&123i64.to_le_bytes());
        assert_eq!(data.len(), SWAP_QUOTE_SERIALIZED_LEN);
        data
    }

    #[test]
    fn quote_sell_instruction_matches_v5_idl() {
        let onyc = Pubkey::new_unique();
        let asset = Pubkey::new_unique();
        let ix = build_quote_swap_sell_instruction(&onyc, &asset, &TOKEN_PROGRAM, 1_000).unwrap();

        let offer = pda(&[SEED_OFFER, asset.as_ref(), onyc.as_ref()]);
        let expected_keys = [
            offer,
            pda(&[SEED_PROP_AMM_PAIR_STATE, offer.as_ref()]),
            pda(&[SEED_REDEMPTION_OFFER, onyc.as_ref(), asset.as_ref()]),
            pda(&[SEED_STATE]),
            pda(&[SEED_REDEMPTION_OFFER_VAULT_AUTHORITY]),
            ata(
                &pda(&[SEED_REDEMPTION_OFFER_VAULT_AUTHORITY]),
                &asset,
                &TOKEN_PROGRAM,
            ),
            onyc,
            asset,
            TOKEN_PROGRAM,
            pda(&[SEED_MARKET_STATS]),
        ];
        assert_eq!(ix.accounts.len(), expected_keys.len());
        for (meta, expected) in ix.accounts.iter().zip(expected_keys) {
            assert_eq!(meta.pubkey, expected);
            assert!(!meta.is_writable);
            assert!(!meta.is_signer);
        }
        let mut expected_data = QUOTE_SWAP_SELL_DISCRIMINATOR.to_vec();
        expected_data.extend_from_slice(&1_000u64.to_le_bytes());
        assert_eq!(ix.data, expected_data);
    }

    #[test]
    fn quote_parser_binds_return_data_to_request() {
        let onyc = Pubkey::new_unique();
        let asset = Pubkey::new_unique();
        let offer = pda(&[SEED_OFFER, asset.as_ref(), onyc.as_ref()]);
        let data = serialized_quote(offer, onyc, asset, 1_000, 970);

        let quote = parse_swap_sell_quote(&ONRE_PROGRAM_ID, &data, &onyc, &asset, 1_000).unwrap();
        assert_eq!(quote.token_in_fee_amount, 20);
        assert_eq!(quote.minimum_out, 970);
        assert_eq!(quote.quoted_at, 123);

        assert!(matches!(
            parse_swap_sell_quote(&Pubkey::new_unique(), &data, &onyc, &asset, 1_000),
            Err(OnreError::InvalidQuoteProgram { .. })
        ));
        assert!(matches!(
            parse_swap_sell_quote(&ONRE_PROGRAM_ID, &data[..151], &onyc, &asset, 1_000),
            Err(OnreError::InvalidQuoteLength { .. })
        ));
        assert!(matches!(
            parse_swap_sell_quote(&ONRE_PROGRAM_ID, &data, &onyc, &asset, 999),
            Err(OnreError::QuoteMismatch)
        ));
    }

    #[test]
    fn open_sell_instruction_matches_v5_idl_shape() {
        let user = Pubkey::new_unique();
        let onyc = Pubkey::new_unique();
        let asset = Pubkey::new_unique();
        let main_offer = Pubkey::new_unique();
        let offer = pda(&[SEED_OFFER, asset.as_ref(), onyc.as_ref()]);
        let quote = parse_swap_sell_quote(
            &ONRE_PROGRAM_ID,
            &serialized_quote(offer, onyc, asset, 1_000, 970),
            &onyc,
            &asset,
            1_000,
        )
        .unwrap();

        let ix = build_open_swap_sell_instruction(
            &user,
            &onyc,
            &asset,
            &TOKEN_PROGRAM,
            &TOKEN_PROGRAM,
            &main_offer,
            &quote,
        )
        .unwrap();
        assert_eq!(ix.accounts.len(), 31);
        assert_eq!(ix.accounts[0].pubkey, offer);
        assert_eq!(
            ix.accounts[1].pubkey,
            pda(&[SEED_PROP_AMM_PAIR_STATE, offer.as_ref()])
        );
        assert_eq!(
            ix.accounts[2].pubkey,
            pda(&[SEED_REDEMPTION_OFFER, onyc.as_ref(), asset.as_ref()])
        );
        assert_eq!(ix.accounts[8].pubkey, onyc);
        assert_eq!(ix.accounts[10].pubkey, asset);
        assert_eq!(ix.accounts[26].pubkey, user);
        assert!(ix.accounts[26].is_signer);
        assert_eq!(ix.accounts[29].pubkey, main_offer);
        assert_eq!(
            ix.accounts[30].pubkey,
            ata(&pda(&[SEED_OFFER_VAULT_AUTHORITY]), &onyc, &TOKEN_PROGRAM)
        );
        let mut expected_data = OPEN_SWAP_SELL_DISCRIMINATOR.to_vec();
        expected_data.extend_from_slice(&1_000u64.to_le_bytes());
        expected_data.extend_from_slice(&970u64.to_le_bytes());
        assert_eq!(ix.data, expected_data);
    }

    #[test]
    fn positive_sell_rejects_zero_minimum_out() {
        let onyc = Pubkey::new_unique();
        let asset = Pubkey::new_unique();
        let offer = pda(&[SEED_OFFER, asset.as_ref(), onyc.as_ref()]);
        let data = serialized_quote(offer, onyc, asset, 1_000, 0);
        assert!(matches!(
            parse_swap_sell_quote(&ONRE_PROGRAM_ID, &data, &onyc, &asset, 1_000),
            Err(OnreError::InvalidMinimumOut)
        ));
    }
}
