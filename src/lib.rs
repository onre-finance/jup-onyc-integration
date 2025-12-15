use anyhow::Result;
use jupiter_amm_interface::{
    try_get_account_data, AccountMap, Amm, AmmContext, KeyedAccount, Quote, QuoteParams, Swap,
    SwapAndAccountMetas, SwapParams,
};
use rust_decimal::Decimal;
use solana_sdk::{
    instruction::AccountMeta, program_pack::Pack, pubkey::Pubkey,
    system_program::ID as SYSTEM_PROGRAM_ID,
};
use spl_associated_token_account::get_associated_token_address_with_program_id;
use spl_token::state::Mint as TokenMint;
use spl_token_2022::{extension::StateWithExtensionsOwned, state::Mint as Mint22};

pub mod constants;
pub mod errors;
pub mod pricing;
pub mod state;

use constants::*;
use errors::OnreAmmError;
use pricing::{calculate_step_price_at, calculate_token_out_amount, find_active_vector_at};
use state::{Offer, State};

/// Main AMM implementation for OnRe Protocol
///
/// Note: For time-based pricing, we use `SystemTime::now()` rather than
/// on-chain Clock sysvar. This is acceptable because:
/// 1. OnRe uses discrete interval pricing (price_fix_duration)
/// 2. Small time differences won't affect price within an interval
/// 3. Jupiter executes quotes quickly after generation
pub struct OnreAmm {
    pub label: String,
    pub program_id: Pubkey,
    pub offer_key: Pubkey,
    pub offer: Offer,
    pub state_key: Pubkey,
    pub state: Option<State>,
    pub token_in_mint: Pubkey,
    pub token_out_mint: Pubkey,
    pub token_in_decimals: u8,
    pub token_out_decimals: u8,
    pub token_in_program: Pubkey,
    pub token_out_program: Pubkey,
    pub token_out_supply: u64,
}

impl OnreAmm {
    pub fn new(offer_key: Pubkey, offer: Offer) -> Self {
        let (state_key, _) = Pubkey::find_program_address(&[SEED_STATE], &ONRE_PROGRAM_ID);

        OnreAmm {
            label: AMM_LABEL.to_owned(),
            program_id: ONRE_PROGRAM_ID,
            offer_key,
            offer,
            state_key,
            state: None,
            token_in_mint: offer.token_in_mint,
            token_out_mint: offer.token_out_mint,
            token_in_decimals: 6,
            token_out_decimals: 9,
            token_in_program: TOKEN_PROGRAM,
            token_out_program: TOKEN_PROGRAM,
            token_out_supply: 0,
        }
    }
}

impl Clone for OnreAmm {
    fn clone(&self) -> Self {
        OnreAmm {
            label: self.label.clone(),
            program_id: self.program_id,
            offer_key: self.offer_key,
            offer: self.offer,
            state_key: self.state_key,
            state: self.state,
            token_in_mint: self.token_in_mint,
            token_out_mint: self.token_out_mint,
            token_in_decimals: self.token_in_decimals,
            token_out_decimals: self.token_out_decimals,
            token_in_program: self.token_in_program,
            token_out_program: self.token_out_program,
            token_out_supply: self.token_out_supply,
        }
    }
}

/// Helper struct for building swap account metas
#[derive(Copy, Clone, Debug)]
pub struct OnreSwap {
    pub user_source: Pubkey,
    pub user_destination: Pubkey,
    pub user_transfer_authority: Pubkey,
}

impl Amm for OnreAmm {
    fn from_keyed_account(keyed_account: &KeyedAccount, _amm_context: &AmmContext) -> Result<Self> {
        let offer = Offer::load(&keyed_account.account.data)?;

        if !offer.allow_permissionless() {
            return Err(OnreAmmError::PermissionlessNotAllowed.into());
        }

        Ok(OnreAmm::new(keyed_account.key, offer))
    }

    fn label(&self) -> String {
        self.label.clone()
    }

    fn program_id(&self) -> Pubkey {
        self.program_id
    }

    fn key(&self) -> Pubkey {
        self.offer_key
    }

    fn get_reserve_mints(&self) -> Vec<Pubkey> {
        vec![self.token_in_mint, self.token_out_mint]
    }

    fn get_accounts_to_update(&self) -> Vec<Pubkey> {
        vec![
            self.offer_key,
            self.state_key,
            self.token_in_mint,
            self.token_out_mint,
        ]
    }

    fn update(&mut self, account_map: &AccountMap) -> Result<()> {
        // Update offer state
        let offer_data = try_get_account_data(account_map, &self.offer_key)?;
        self.offer = Offer::load(offer_data)?;

        // Update program state (for boss address)
        let state_data = try_get_account_data(account_map, &self.state_key)?;
        self.state = Some(State::load(state_data)?);

        // Update token_in mint info
        let token_in_data = try_get_account_data(account_map, &self.token_in_mint)?;
        let (decimals, program, _supply) = get_mint_info(token_in_data)?;
        self.token_in_decimals = decimals;
        self.token_in_program = program;

        // Update token_out mint info (including supply for max supply check)
        let token_out_data = try_get_account_data(account_map, &self.token_out_mint)?;
        let (decimals, program, supply) = get_mint_info(token_out_data)?;
        self.token_out_decimals = decimals;
        self.token_out_program = program;
        self.token_out_supply = supply;

        Ok(())
    }

    fn quote(&self, quote_params: &QuoteParams) -> Result<Quote> {
        if quote_params.input_mint != self.token_in_mint
            || quote_params.output_mint != self.token_out_mint
        {
            return Err(OnreAmmError::InvalidDirection.into());
        }

        // Check kill switch
        if let Some(state) = &self.state {
            if state.is_killed() {
                return Err(OnreAmmError::KillSwitchActivated.into());
            }
        }

        // Get current time
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| OnreAmmError::TimeError)?
            .as_secs();

        // Find active pricing vector
        let active_vector = find_active_vector_at(&self.offer, current_time)?;

        // Calculate current price
        let current_price = calculate_step_price_at(
            active_vector.apr,
            active_vector.base_price,
            active_vector.base_time,
            active_vector.price_fix_duration,
            current_time,
        )?;

        // Calculate fee
        let fee_amount = (quote_params.amount as u128)
            .checked_mul(self.offer.fee_basis_points as u128)
            .ok_or(OnreAmmError::MathOverflow)?
            .checked_div(MAX_BASIS_POINTS as u128)
            .ok_or(OnreAmmError::MathOverflow)? as u64;

        let token_in_net = quote_params
            .amount
            .checked_sub(fee_amount)
            .ok_or(OnreAmmError::MathOverflow)?;

        // Calculate output amount
        let out_amount = calculate_token_out_amount(
            token_in_net,
            current_price,
            self.token_in_decimals,
            self.token_out_decimals,
        )?;

        if let Some(state) = &self.state {
            if state.max_supply > 0 {
                let new_supply = self
                    .token_out_supply
                    .checked_add(out_amount)
                    .ok_or(OnreAmmError::MathOverflow)?;

                if new_supply > state.max_supply {
                    return Err(OnreAmmError::MaxSupplyExceeded {
                        requested: out_amount,
                        current_supply: self.token_out_supply,
                        max_supply: state.max_supply,
                    }
                    .into());
                }
            }
        }

        let fee_pct = Decimal::new(self.offer.fee_basis_points as i64, 4);

        Ok(Quote {
            fee_pct,
            in_amount: quote_params.amount,
            out_amount,
            fee_amount,
            fee_mint: quote_params.input_mint,
            ..Quote::default()
        })
    }

    fn get_swap_and_account_metas(&self, swap_params: &SwapParams) -> Result<SwapAndAccountMetas> {
        let state = self.state.as_ref().ok_or(OnreAmmError::StateMissing)?;

        let (vault_authority, _) =
            Pubkey::find_program_address(&[SEED_OFFER_VAULT_AUTHORITY], &self.program_id);
        let (permissionless_authority, _) =
            Pubkey::find_program_address(&[SEED_PERMISSIONLESS_AUTHORITY], &self.program_id);
        let (mint_authority, _) =
            Pubkey::find_program_address(&[SEED_MINT_AUTHORITY], &self.program_id);

        // Derive token accounts
        let vault_token_in = get_associated_token_address_with_program_id(
            &vault_authority,
            &self.token_in_mint,
            &self.token_in_program,
        );
        let vault_token_out = get_associated_token_address_with_program_id(
            &vault_authority,
            &self.token_out_mint,
            &self.token_out_program,
        );
        let permissionless_token_in = get_associated_token_address_with_program_id(
            &permissionless_authority,
            &self.token_in_mint,
            &self.token_in_program,
        );
        let permissionless_token_out = get_associated_token_address_with_program_id(
            &permissionless_authority,
            &self.token_out_mint,
            &self.token_out_program,
        );
        let boss_token_in = get_associated_token_address_with_program_id(
            &state.boss,
            &self.token_in_mint,
            &self.token_in_program,
        );

        // Build account metas for take_offer_permissionless
        let account_metas = vec![
            AccountMeta::new(self.offer_key, false),
            AccountMeta::new_readonly(self.state_key, false),
            AccountMeta::new_readonly(state.boss, false),
            AccountMeta::new_readonly(vault_authority, false),
            AccountMeta::new(vault_token_in, false),
            AccountMeta::new(vault_token_out, false),
            AccountMeta::new_readonly(permissionless_authority, false),
            AccountMeta::new(permissionless_token_in, false),
            AccountMeta::new(permissionless_token_out, false),
            AccountMeta::new(self.token_in_mint, false),
            AccountMeta::new_readonly(self.token_in_program, false),
            AccountMeta::new(self.token_out_mint, false),
            AccountMeta::new_readonly(self.token_out_program, false),
            AccountMeta::new(swap_params.source_token_account, false),
            AccountMeta::new(swap_params.destination_token_account, false),
            AccountMeta::new(boss_token_in, false),
            AccountMeta::new_readonly(mint_authority, false),
            AccountMeta::new_readonly(SYSVAR_INSTRUCTIONS, false),
            AccountMeta::new(swap_params.token_transfer_authority.clone(), true),
            AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        ];

        Ok(SwapAndAccountMetas {
            swap: Swap::TokenSwap,
            account_metas,
        })
    }

    fn clone_amm(&self) -> Box<dyn Amm + Send + Sync> {
        Box::new(self.clone())
    }

    fn unidirectional(&self) -> bool {
        true
    }

    fn supports_exact_out(&self) -> bool {
        false
    }

    fn get_accounts_len(&self) -> usize {
        21
    }
}

fn get_mint_info(data: &[u8]) -> Result<(u8, Pubkey, u64)> {
    if let Ok(mint) = TokenMint::unpack(data) {
        return Ok((mint.decimals, TOKEN_PROGRAM, mint.supply));
    }

    let mint = StateWithExtensionsOwned::<Mint22>::unpack(data.to_vec())?;
    Ok((mint.base.decimals, TOKEN_22_PROGRAM, mint.base.supply))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pda_derivation() {
        let (state_pda, _) = Pubkey::find_program_address(&[SEED_STATE], &ONRE_PROGRAM_ID);
        println!("State PDA: {}", state_pda);

        let (vault_auth, _) =
            Pubkey::find_program_address(&[SEED_OFFER_VAULT_AUTHORITY], &ONRE_PROGRAM_ID);
        println!("Vault Authority: {}", vault_auth);

        let (permissionless_auth, _) =
            Pubkey::find_program_address(&[SEED_PERMISSIONLESS_AUTHORITY], &ONRE_PROGRAM_ID);
        println!("Permissionless Authority: {}", permissionless_auth);

        let (mint_auth, _) = Pubkey::find_program_address(&[SEED_MINT_AUTHORITY], &ONRE_PROGRAM_ID);
        println!("Mint Authority: {}", mint_auth);
    }

    #[test]
    fn test_max_supply_check() {
        let current_supply: u64 = 900_000_000_000; // 900 ONyc (9 decimals)
        let max_supply: u64 = 1_000_000_000_000; // 1000 ONyc max
        let out_amount: u64 = 150_000_000_000; // 150 ONyc requested

        let new_supply = current_supply.checked_add(out_amount).unwrap();
        assert!(new_supply > max_supply, "Should exceed max supply");

        let small_amount: u64 = 50_000_000_000; // 50 ONyc
        let new_supply_ok = current_supply.checked_add(small_amount).unwrap();
        assert!(new_supply_ok <= max_supply, "Should not exceed max supply");
    }
}
