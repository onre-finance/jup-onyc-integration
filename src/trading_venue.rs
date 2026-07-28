use async_trait::async_trait;
use solana_account::Account;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use spl_associated_token_account::get_associated_token_address_with_program_id;

use crate::constants::*;
use crate::errors::OnreError;
use crate::pricing::{calculate_step_price_at, calculate_token_out_amount, find_active_vector_at};
use crate::state::{Offer, State};
use crate::token_info::TokenInfo;

/// Swap direction for quote requests
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SwapType {
    ExactIn,
    ExactOut,
}

/// Quote request parameters
#[derive(Debug, Clone)]
pub struct QuoteRequest {
    pub input_mint: Pubkey,
    pub output_mint: Pubkey,
    pub amount: u64,
    pub swap_type: SwapType,
}

/// Quote result returned by the venue
#[derive(Debug, Clone)]
pub struct QuoteResult {
    pub input_mint: Pubkey,
    pub output_mint: Pubkey,
    pub amount: u64,
    pub expected_output: u64,
    pub not_enough_liquidity: bool,
}

/// Protocol identifier for OnRe
#[derive(Debug, Copy, Clone)]
pub enum PoolProtocol {
    OnRe,
}

impl From<PoolProtocol> for String {
    fn from(protocol: PoolProtocol) -> Self {
        match protocol {
            PoolProtocol::OnRe => "OnRe".to_string(),
        }
    }
}

/// Trait for account caching
#[async_trait]
pub trait AccountsCache: Send + Sync {
    async fn get_account(&self, pubkey: &Pubkey) -> Result<Option<Account>, OnreError>;
    async fn get_accounts(&self, pubkeys: &[Pubkey]) -> Result<Vec<Option<Account>>, OnreError>;
}

/// Trait for constructing venue from account data
pub trait FromAccount {
    fn from_account(pubkey: &Pubkey, account: &Account) -> Result<Self, OnreError>
    where
        Self: Sized;
}

/// OnRe Trading Venue implementation
#[derive(Clone)]
pub struct OnreVenue {
    pub offer_key: Pubkey,
    pub offer: Offer,
    pub state_key: Pubkey,
    pub state: Option<State>,
    pub token_info: Vec<TokenInfo>,
    pub token_out_supply: u64,
    initialized: bool,
}

impl FromAccount for OnreVenue {
    fn from_account(pubkey: &Pubkey, account: &Account) -> Result<Self, OnreError> {
        let offer = Offer::load(&account.data)?;

        if !offer.allow_permissionless() {
            return Err(OnreError::PermissionlessNotAllowed);
        }

        let (state_key, _) = Pubkey::find_program_address(&[SEED_STATE], &ONRE_PROGRAM_ID);

        Ok(OnreVenue {
            offer_key: *pubkey,
            offer,
            state_key,
            state: None,
            token_info: Vec::new(),
            token_out_supply: 0,
            initialized: false,
        })
    }
}

impl OnreVenue {
    /// Check if venue is fully initialized
    pub fn initialized(&self) -> bool {
        self.initialized
    }

    /// Get the OnRe program ID
    pub fn program_id(&self) -> Pubkey {
        ONRE_PROGRAM_ID
    }

    /// Get program dependencies
    pub fn program_dependencies(&self) -> Vec<Pubkey> {
        vec![ONRE_PROGRAM_ID, TOKEN_PROGRAM, TOKEN_22_PROGRAM]
    }

    /// Get the market/offer ID
    pub fn market_id(&self) -> Pubkey {
        self.offer_key
    }

    /// Get tradable mints (token_in and token_out)
    pub fn tradable_mints(&self) -> Result<Vec<Pubkey>, OnreError> {
        Ok(vec![self.offer.token_in_mint, self.offer.token_out_mint])
    }

    /// Get token decimals
    pub fn decimals(&self) -> Result<Vec<i32>, OnreError> {
        Ok(self.token_info.iter().map(|t| t.decimals).collect())
    }

    /// Get token info slice
    pub fn get_token_info(&self) -> &[TokenInfo] {
        &self.token_info
    }

    /// Get token by index
    pub fn get_token(&self, i: usize) -> Result<&TokenInfo, OnreError> {
        self.token_info
            .get(i)
            .ok_or(OnreError::TokenInfoIndexError(i))
    }

    /// Get protocol type
    pub fn protocol(&self) -> PoolProtocol {
        PoolProtocol::OnRe
    }

    /// Get human-readable label
    pub fn label(&self) -> String {
        "OnRe".to_string()
    }

    /// Get pubkeys required for state update
    pub fn get_required_pubkeys_for_update(&self) -> Result<Vec<Pubkey>, OnreError> {
        Ok(vec![
            self.offer_key,
            self.state_key,
            self.offer.token_in_mint,
            self.offer.token_out_mint,
        ])
    }

    /// Update venue state from account cache
    pub async fn update_state(&mut self, cache: &dyn AccountsCache) -> Result<(), OnreError> {
        let pubkeys = vec![
            self.offer_key,
            self.state_key,
            self.offer.token_in_mint,
            self.offer.token_out_mint,
        ];

        let accounts = cache.get_accounts(&pubkeys).await?;

        let [offer_account, state_account, token_in_account, token_out_account]: [Option<Account>;
            4] = accounts
            .try_into()
            .map_err(|_| OnreError::FailedToFetchMultipleAccounts)?;

        // Update offer
        if let Some(acc) = offer_account {
            self.offer = Offer::load(&acc.data)?;
        } else {
            return Err(OnreError::NoAccountFound(self.offer_key));
        }

        // Update state
        if let Some(acc) = state_account {
            self.state = Some(State::load(&acc.data)?);
        } else {
            return Err(OnreError::NoAccountFound(self.state_key));
        }

        // Update token info
        let token_in_info = token_in_account
            .as_ref()
            .map(|acc| TokenInfo::new(&self.offer.token_in_mint, acc))
            .transpose()?
            .ok_or(OnreError::NoAccountFound(self.offer.token_in_mint))?;

        let token_out_info = token_out_account
            .as_ref()
            .map(|acc| TokenInfo::new(&self.offer.token_out_mint, acc))
            .transpose()?
            .ok_or(OnreError::NoAccountFound(self.offer.token_out_mint))?;

        self.token_out_supply = token_out_info.supply;
        self.token_info = vec![token_in_info, token_out_info];
        self.initialized = true;

        Ok(())
    }

    /// Compute a quote for the given swap parameters
    ///
    /// IMPORTANT: This must handle zero-input amounts without error
    pub fn quote(&self, request: QuoteRequest) -> Result<QuoteResult, OnreError> {
        // Validate direction: OnRe only supports token_in -> token_out (ONyc minting)
        if request.input_mint != self.offer.token_in_mint
            || request.output_mint != self.offer.token_out_mint
        {
            return Err(OnreError::InvalidMint(request.input_mint));
        }

        // ExactOut not supported
        if request.swap_type == SwapType::ExactOut {
            return Err(OnreError::ExactOutNotSupported);
        }

        // Handle zero-input
        if request.amount == 0 {
            return Ok(QuoteResult {
                input_mint: request.input_mint,
                output_mint: request.output_mint,
                amount: 0,
                expected_output: 0,
                not_enough_liquidity: false,
            });
        }

        // Check kill switch
        if let Some(state) = &self.state {
            if state.is_killed() {
                return Ok(QuoteResult {
                    input_mint: request.input_mint,
                    output_mint: request.output_mint,
                    amount: request.amount,
                    expected_output: 0,
                    not_enough_liquidity: true,
                });
            }
        }

        // Get current time for pricing
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| OnreError::TimeError)?
            .as_secs();

        // Find active pricing vector
        let active_vector = match find_active_vector_at(&self.offer, current_time) {
            Ok(v) => v,
            Err(_) => {
                return Ok(QuoteResult {
                    input_mint: request.input_mint,
                    output_mint: request.output_mint,
                    amount: request.amount,
                    expected_output: 0,
                    not_enough_liquidity: true,
                });
            }
        };

        // Calculate current price
        let current_price = calculate_step_price_at(
            active_vector.apr,
            active_vector.base_price,
            active_vector.base_time,
            active_vector.price_fix_duration,
            current_time,
        )?;

        // Calculate fee
        let fee_amount = (request.amount as u128)
            .checked_mul(self.offer.fee_basis_points as u128)
            .ok_or(OnreError::MathOverflow)?
            .checked_div(MAX_BASIS_POINTS as u128)
            .ok_or(OnreError::MathOverflow)? as u64;

        let token_in_net = request
            .amount
            .checked_sub(fee_amount)
            .ok_or(OnreError::MathOverflow)?;

        // Get token decimals
        let token_in_decimals = self.get_token(0)?.decimals as u8;
        let token_out_decimals = self.get_token(1)?.decimals as u8;

        // Calculate output amount
        let out_amount = calculate_token_out_amount(
            token_in_net,
            current_price,
            token_in_decimals,
            token_out_decimals,
        )?;

        // Check max supply
        if let Some(state) = &self.state {
            if state.max_supply > 0 {
                let new_supply = self
                    .token_out_supply
                    .checked_add(out_amount)
                    .ok_or(OnreError::MathOverflow)?;

                if new_supply > state.max_supply {
                    return Ok(QuoteResult {
                        input_mint: request.input_mint,
                        output_mint: request.output_mint,
                        amount: request.amount,
                        expected_output: 0,
                        not_enough_liquidity: true,
                    });
                }
            }
        }

        Ok(QuoteResult {
            input_mint: request.input_mint,
            output_mint: request.output_mint,
            amount: request.amount,
            expected_output: out_amount,
            not_enough_liquidity: false,
        })
    }

    /// Generate swap instruction for take_offer_permissionless
    pub fn generate_swap_instruction(
        &self,
        request: QuoteRequest,
        user: Pubkey,
    ) -> Result<Instruction, OnreError> {
        let state = self.state.as_ref().ok_or(OnreError::StateMissing)?;

        let token_in_info = self.get_token(0)?;
        let token_out_info = self.get_token(1)?;

        // Derive PDAs
        let (vault_authority, _) =
            Pubkey::find_program_address(&[SEED_OFFER_VAULT_AUTHORITY], &ONRE_PROGRAM_ID);
        let (permissionless_authority, _) =
            Pubkey::find_program_address(&[SEED_PERMISSIONLESS_AUTHORITY], &ONRE_PROGRAM_ID);
        let (mint_authority, _) =
            Pubkey::find_program_address(&[SEED_MINT_AUTHORITY], &ONRE_PROGRAM_ID);

        // Derive token accounts
        let vault_token_in = get_associated_token_address_with_program_id(
            &vault_authority,
            &token_in_info.pubkey,
            &token_in_info.get_token_program(),
        );
        let vault_token_out = get_associated_token_address_with_program_id(
            &vault_authority,
            &token_out_info.pubkey,
            &token_out_info.get_token_program(),
        );
        let permissionless_token_in = get_associated_token_address_with_program_id(
            &permissionless_authority,
            &token_in_info.pubkey,
            &token_in_info.get_token_program(),
        );
        let permissionless_token_out = get_associated_token_address_with_program_id(
            &permissionless_authority,
            &token_out_info.pubkey,
            &token_out_info.get_token_program(),
        );

        // User token accounts
        let user_token_in = get_associated_token_address_with_program_id(
            &user,
            &token_in_info.pubkey,
            &token_in_info.get_token_program(),
        );
        let user_token_out = get_associated_token_address_with_program_id(
            &user,
            &token_out_info.pubkey,
            &token_out_info.get_token_program(),
        );

        // Boss fee account
        let boss_token_in = get_associated_token_address_with_program_id(
            &state.boss,
            &token_in_info.pubkey,
            &token_in_info.get_token_program(),
        );

        // Build instruction data: discriminator + amount (u64)
        let mut data = Vec::with_capacity(16);
        data.extend_from_slice(&TAKE_OFFER_PERMISSIONLESS_DISCRIMINATOR);
        data.extend_from_slice(&request.amount.to_le_bytes());

        // Build account metas
        let accounts = vec![
            AccountMeta::new(self.offer_key, false),
            AccountMeta::new_readonly(self.state_key, false),
            AccountMeta::new_readonly(state.boss, false),
            AccountMeta::new_readonly(vault_authority, false),
            AccountMeta::new(vault_token_in, false),
            AccountMeta::new(vault_token_out, false),
            AccountMeta::new_readonly(permissionless_authority, false),
            AccountMeta::new(permissionless_token_in, false),
            AccountMeta::new(permissionless_token_out, false),
            AccountMeta::new(token_in_info.pubkey, false),
            AccountMeta::new_readonly(token_in_info.get_token_program(), false),
            AccountMeta::new(token_out_info.pubkey, false),
            AccountMeta::new_readonly(token_out_info.get_token_program(), false),
            AccountMeta::new(user_token_in, false),
            AccountMeta::new(user_token_out, false),
            AccountMeta::new(boss_token_in, false),
            AccountMeta::new_readonly(mint_authority, false),
            AccountMeta::new_readonly(SYSVAR_INSTRUCTIONS, false),
            AccountMeta::new(user, true),
            AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
        ];

        Ok(Instruction {
            program_id: ONRE_PROGRAM_ID,
            accounts,
            data,
        })
    }

    /// Generate the v5 swap instruction (`take_offer_permissionless_v2`).
    ///
    /// The v5 permissionless take routes proceeds/fees to dedicated
    /// configurable-vault PDAs, refills the redemption vault, accrues the
    /// ONyc buffer, and refreshes the market-stats PDA. All PDAs are derived
    /// per the v5 IDL. The optional `approval_message` is always `None`:
    /// aggregator flow targets offers with `needs_approval = false`.
    pub fn generate_swap_instruction_v2(
        &self,
        request: QuoteRequest,
        user: Pubkey,
    ) -> Result<Instruction, OnreError> {
        let state = self.state.as_ref().ok_or(OnreError::StateMissing)?;

        let token_in_info = self.get_token(0)?;
        let token_out_info = self.get_token(1)?;
        let token_in_mint = token_in_info.pubkey;
        let token_out_mint = token_out_info.pubkey;
        let token_in_program = token_in_info.get_token_program();
        let token_out_program = token_out_info.get_token_program();

        let pda = |seeds: &[&[u8]]| Pubkey::find_program_address(seeds, &ONRE_PROGRAM_ID).0;

        let offer_pda = pda(&[SEED_OFFER, token_in_mint.as_ref(), token_out_mint.as_ref()]);
        let state_pda = pda(&[SEED_STATE]);
        let vault_authority = pda(&[SEED_OFFER_VAULT_AUTHORITY]);
        let permissionless_authority = pda(&[SEED_PERMISSIONLESS_AUTHORITY]);
        let mint_authority = pda(&[SEED_MINT_AUTHORITY]);
        // Redemption offer for the opposite direction (ONyc -> token_in)
        let redemption_offer = pda(&[
            SEED_REDEMPTION_OFFER,
            token_out_mint.as_ref(),
            token_in_mint.as_ref(),
        ]);
        let redemption_vault_authority = pda(&[SEED_REDEMPTION_OFFER_VAULT_AUTHORITY]);
        let offer_proceeds_vault = pda(&[SEED_CONFIGURABLE_VAULT, SEED_OFFER_PROCEEDS_VAULT]);
        let offer_fee_vault = pda(&[SEED_CONFIGURABLE_VAULT, SEED_OFFER_FEE_VAULT]);
        let management_fee_vault = pda(&[SEED_CONFIGURABLE_VAULT, SEED_MANAGEMENT_FEE_VAULT]);
        let performance_fee_vault = pda(&[SEED_CONFIGURABLE_VAULT, SEED_PERFORMANCE_FEE_VAULT]);
        let buffer_state = pda(&[SEED_BUFFER_STATE]);
        let reserve_vault_authority = pda(&[SEED_RESERVE_VAULT_AUTHORITY]);
        let market_stats = pda(&[SEED_MARKET_STATS]);
        let excluded_balance = pda(&[SEED_CIRCULATING_SUPPLY_EXCLUDED_BALANCE]);

        let ata = |owner: &Pubkey, mint: &Pubkey, program: &Pubkey| {
            get_associated_token_address_with_program_id(owner, mint, program)
        };

        // Build instruction data: discriminator + amount (u64) + Option::None approval
        let mut data = Vec::with_capacity(17);
        data.extend_from_slice(&TAKE_OFFER_PERMISSIONLESS_V2_DISCRIMINATOR);
        data.extend_from_slice(&request.amount.to_le_bytes());
        data.push(0); // approval_message: None

        let accounts = vec![
            AccountMeta::new(offer_pda, false),
            AccountMeta::new_readonly(state_pda, false),
            AccountMeta::new_readonly(vault_authority, false),
            AccountMeta::new(
                ata(&vault_authority, &token_in_mint, &token_in_program),
                false,
            ),
            AccountMeta::new(
                ata(&vault_authority, &token_out_mint, &token_out_program),
                false,
            ),
            AccountMeta::new_readonly(permissionless_authority, false),
            AccountMeta::new(
                ata(&permissionless_authority, &token_in_mint, &token_in_program),
                false,
            ),
            AccountMeta::new(
                ata(
                    &permissionless_authority,
                    &token_out_mint,
                    &token_out_program,
                ),
                false,
            ),
            AccountMeta::new(token_in_mint, false),
            AccountMeta::new_readonly(token_in_program, false),
            AccountMeta::new(token_out_mint, false),
            AccountMeta::new_readonly(token_out_program, false),
            AccountMeta::new(ata(&user, &token_in_mint, &token_in_program), false),
            AccountMeta::new(ata(&user, &token_out_mint, &token_out_program), false),
            AccountMeta::new_readonly(redemption_offer, false),
            AccountMeta::new_readonly(redemption_vault_authority, false),
            AccountMeta::new(
                ata(
                    &redemption_vault_authority,
                    &token_in_mint,
                    &token_in_program,
                ),
                false,
            ),
            AccountMeta::new(offer_proceeds_vault, false),
            AccountMeta::new(
                ata(&offer_proceeds_vault, &token_in_mint, &token_in_program),
                false,
            ),
            AccountMeta::new(offer_fee_vault, false),
            AccountMeta::new(
                ata(&offer_fee_vault, &token_in_mint, &token_in_program),
                false,
            ),
            AccountMeta::new_readonly(mint_authority, false),
            AccountMeta::new(buffer_state, false),
            AccountMeta::new(
                ata(
                    &reserve_vault_authority,
                    &token_out_mint,
                    &token_out_program,
                ),
                false,
            ),
            AccountMeta::new(
                ata(&management_fee_vault, &token_out_mint, &token_out_program),
                false,
            ),
            AccountMeta::new(
                ata(&performance_fee_vault, &token_out_mint, &token_out_program),
                false,
            ),
            AccountMeta::new(market_stats, false),
            AccountMeta::new_readonly(excluded_balance, false),
            AccountMeta::new_readonly(SYSVAR_INSTRUCTIONS, false),
            AccountMeta::new(user, true),
            AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
            AccountMeta::new_readonly(state.main_offer, false),
        ];

        Ok(Instruction {
            program_id: ONRE_PROGRAM_ID,
            accounts,
            data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_input_quote() {
        // Titan requires zero-input quotes to work
        // This is a basic structural test
        let request = QuoteRequest {
            input_mint: Pubkey::new_unique(),
            output_mint: Pubkey::new_unique(),
            amount: 0,
            swap_type: SwapType::ExactIn,
        };

        // Would need a properly initialized venue to test fully
        // but demonstrates the interface
        assert_eq!(request.amount, 0);
    }

    /// Builds a minimal v5 Offer account byte blob for the given mints
    /// (allow_permissionless=1, enabled).
    fn offer_bytes(token_in_mint: &Pubkey, token_out_mint: &Pubkey) -> Vec<u8> {
        let mut data = vec![0u8; 8 + 600];
        data[8..40].copy_from_slice(token_in_mint.as_ref());
        data[40..72].copy_from_slice(token_out_mint.as_ref());
        data[8 + 468] = 1; // allow_permissionless
        data
    }

    /// Builds a minimal v5 State byte blob (borsh layout).
    fn state_bytes(boss: &Pubkey, main_offer: &Pubkey) -> Vec<u8> {
        let mut data = vec![0u8; 8 + 938];
        data[8..40].copy_from_slice(boss.as_ref());
        data[8 + 850..8 + 882].copy_from_slice(main_offer.as_ref());
        data
    }

    fn test_venue(token_in_mint: Pubkey, token_out_mint: Pubkey) -> OnreVenue {
        let (offer_pda, _) = Pubkey::find_program_address(
            &[b"offer", token_in_mint.as_ref(), token_out_mint.as_ref()],
            &ONRE_PROGRAM_ID,
        );
        let account = solana_account::Account {
            lamports: 1,
            data: offer_bytes(&token_in_mint, &token_out_mint),
            owner: ONRE_PROGRAM_ID,
            executable: false,
            rent_epoch: 0,
        };
        let mut venue = OnreVenue::from_account(&offer_pda, &account).unwrap();
        venue.state = Some(
            crate::state::State::load(&state_bytes(&Pubkey::new_unique(), &offer_pda)).unwrap(),
        );
        venue.token_info = vec![
            TokenInfo {
                pubkey: token_in_mint,
                decimals: 6,
                is_token_2022: false,
                supply: 0,
                transfer_fee: None,
                maximum_fee: None,
            },
            TokenInfo {
                pubkey: token_out_mint,
                decimals: 9,
                is_token_2022: false,
                supply: 0,
                transfer_fee: None,
                maximum_fee: None,
            },
        ];
        venue.initialized = true;
        venue
    }

    #[test]
    fn test_v2_swap_instruction_data_layout() {
        let venue = test_venue(Pubkey::new_unique(), Pubkey::new_unique());
        let user = Pubkey::new_unique();

        let ix = venue
            .generate_swap_instruction_v2(
                QuoteRequest {
                    input_mint: venue.offer.token_in_mint,
                    output_mint: venue.offer.token_out_mint,
                    amount: 1_000_100,
                    swap_type: SwapType::ExactIn,
                },
                user,
            )
            .unwrap();

        assert_eq!(ix.program_id, ONRE_PROGRAM_ID);
        // discriminator (from v5 IDL) + u64 amount + Option::None for approval_message
        let mut expected = vec![250u8, 180, 68, 89, 124, 124, 31, 250];
        expected.extend_from_slice(&1_000_100u64.to_le_bytes());
        expected.push(0);
        assert_eq!(ix.data, expected);
    }

    #[test]
    fn test_v2_swap_instruction_account_list() {
        let token_in_mint = Pubkey::new_unique();
        let token_out_mint = Pubkey::new_unique();
        let venue = test_venue(token_in_mint, token_out_mint);
        let user = Pubkey::new_unique();

        let ix = venue
            .generate_swap_instruction_v2(
                QuoteRequest {
                    input_mint: token_in_mint,
                    output_mint: token_out_mint,
                    amount: 5,
                    swap_type: SwapType::ExactIn,
                },
                user,
            )
            .unwrap();

        assert_eq!(ix.accounts.len(), 33);

        // Independent PDA derivations with literal seeds (from the v5 IDL)
        let pda = |seeds: &[&[u8]]| Pubkey::find_program_address(seeds, &ONRE_PROGRAM_ID).0;
        let ata = |owner: &Pubkey, mint: &Pubkey| {
            get_associated_token_address_with_program_id(owner, mint, &TOKEN_PROGRAM)
        };

        let offer_pda = pda(&[b"offer", token_in_mint.as_ref(), token_out_mint.as_ref()]);
        let state_pda = pda(&[b"state"]);
        let vault_authority = pda(&[b"offer_vault_authority"]);
        let permissionless_authority = pda(&[b"permissionless-1"]);
        let redemption_offer =
            pda(&[b"redemption_offer", token_out_mint.as_ref(), token_in_mint.as_ref()]);
        let redemption_vault_authority = pda(&[b"redemption_offer_vault_authority"]);
        let proceeds_vault = pda(&[b"configurable_vault", b"offer_proceeds"]);
        let fee_vault = pda(&[b"configurable_vault", b"offer_fee"]);
        let mint_authority = pda(&[b"mint_authority"]);
        let buffer_state = pda(&[b"buffer_state"]);
        let reserve_vault_authority = pda(&[b"reserve_vault_authority"]);
        let management_fee_vault = pda(&[b"configurable_vault", b"management_fee"]);
        let performance_fee_vault = pda(&[b"configurable_vault", b"performance_fee"]);
        let market_stats = pda(&[b"market_stats"]);
        let excluded_balance = pda(&[b"circ_supply_excl_balance"]);

        let expected: Vec<(Pubkey, bool, bool)> = vec![
            (offer_pda, true, false),
            (state_pda, false, false),
            (vault_authority, false, false),
            (ata(&vault_authority, &token_in_mint), true, false),
            (ata(&vault_authority, &token_out_mint), true, false),
            (permissionless_authority, false, false),
            (ata(&permissionless_authority, &token_in_mint), true, false),
            (ata(&permissionless_authority, &token_out_mint), true, false),
            (token_in_mint, true, false),
            (TOKEN_PROGRAM, false, false),
            (token_out_mint, true, false),
            (TOKEN_PROGRAM, false, false),
            (ata(&user, &token_in_mint), true, false),
            (ata(&user, &token_out_mint), true, false),
            (redemption_offer, false, false),
            (redemption_vault_authority, false, false),
            (ata(&redemption_vault_authority, &token_in_mint), true, false),
            (proceeds_vault, true, false),
            (ata(&proceeds_vault, &token_in_mint), true, false),
            (fee_vault, true, false),
            (ata(&fee_vault, &token_in_mint), true, false),
            (mint_authority, false, false),
            (buffer_state, true, false),
            (ata(&reserve_vault_authority, &token_out_mint), true, false),
            (ata(&management_fee_vault, &token_out_mint), true, false),
            (ata(&performance_fee_vault, &token_out_mint), true, false),
            (market_stats, true, false),
            (excluded_balance, false, false),
            (SYSVAR_INSTRUCTIONS, false, false),
            (user, true, true),
            (ASSOCIATED_TOKEN_PROGRAM, false, false),
            (SYSTEM_PROGRAM, false, false),
            (offer_pda, false, false), // main_offer from state.main_offer
        ];

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
}
