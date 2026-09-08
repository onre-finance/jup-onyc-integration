use crate::constants::*;
use crate::errors::OnreError;
use crate::prop_amm::PropAmmPairState;
use crate::redemption::RedemptionOffer;
use crate::state::{CirculatingSupplyExcludedBalance, Offer, State};
use crate::token_info::TokenInfo;
use async_trait::async_trait;
use solana_account::Account;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use spl_associated_token_account::get_associated_token_address_with_program_id;
use spl_token_2022::extension::StateWithExtensions;

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

impl QuoteRequest {
    fn to_zero_result(&self, not_enough_liquidity: bool) -> QuoteResult {
        QuoteResult {
            input_mint: self.input_mint,
            output_mint: self.output_mint,
            amount: self.amount,
            expected_output: 0,
            not_enough_liquidity,
        }
    }
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

/// Operational status of the venue, surfaced as an explicit state
/// (not a generic failure) so integrators can distinguish emergency
/// controls from transient errors.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum VenueStatus {
    /// Offer enabled and kill switch off
    Active,
    /// Global kill switch is on (`State.is_killed`)
    KillSwitchActive,
    /// This offer is disabled (v5 `require_enabled`)
    OfferDisabled,
}

#[derive(Debug, Copy, Clone)]
enum SwapDirection {
    Buy,
    Sell,
}

impl From<PoolProtocol> for String {
    fn from(protocol: PoolProtocol) -> Self {
        match protocol {
            PoolProtocol::OnRe => AMM_LABEL.to_string(),
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
    pub prop_amm_pair_state_key: Pubkey,
    pub prop_amm_pair_state: Option<PropAmmPairState>,
    pub token_info: Vec<TokenInfo>,
    pub token_out_supply: u64,

    pub redemption_offer_key: Pubkey,
    pub redemption_offer: Option<RedemptionOffer>,

    pub redemption_vault_token_in_key: Pubkey,
    pub redemption_vault_token_in_22_key: Pubkey,
    pub redemption_vault_token_in: Option<spl_token_2022::state::Account>,

    pub circulating_supply_excluded_balance_key: Pubkey,
    pub circulating_supply_excluded_balance: Option<CirculatingSupplyExcludedBalance>,

    initialized: bool,
}

impl FromAccount for OnreVenue {
    fn from_account(pubkey: &Pubkey, account: &Account) -> Result<Self, OnreError> {
        if account.owner != ONRE_PROGRAM_ID {
            return Err(OnreError::InvalidAccountOwner {
                account: *pubkey,
                expected: ONRE_PROGRAM_ID,
                actual: account.owner,
            });
        }
        let offer = Offer::load(&account.data)?;

        let expected_offer = Pubkey::find_program_address(
            &[
                SEED_OFFER,
                offer.token_in_mint.as_ref(),
                offer.token_out_mint.as_ref(),
            ],
            &ONRE_PROGRAM_ID,
        )
        .0;
        if *pubkey != expected_offer {
            return Err(OnreError::InvalidPda {
                expected: expected_offer,
                actual: *pubkey,
            });
        }

        if !offer.allow_permissionless() {
            return Err(OnreError::PermissionlessNotAllowed);
        }

        let (state_key, _) = Pubkey::find_program_address(&[SEED_STATE], &ONRE_PROGRAM_ID);

        let (prop_amm_pair_state_key, _) = Pubkey::find_program_address(
            &[SEED_PROP_AMM_PAIR_STATE, &pubkey.to_bytes()],
            &ONRE_PROGRAM_ID,
        );

        // Redemption runs the opposite direction (ONyc -> USDC)
        let (redemption_offer_key, _) = Pubkey::find_program_address(
            &[
                SEED_REDEMPTION_OFFER,
                offer.token_out_mint.as_ref(),
                offer.token_in_mint.as_ref(),
            ],
            &ONRE_PROGRAM_ID,
        );

        let (redemption_vault_authority, _) = Pubkey::find_program_address(
            &[SEED_REDEMPTION_OFFER_VAULT_AUTHORITY],
            &ONRE_PROGRAM_ID,
        );

        // Redemption vault holds the payout asset (token_in of the buy offer, e.g. USDC)
        let redemption_vault_token_in_account = get_associated_token_address_with_program_id(
            &redemption_vault_authority,
            &offer.token_in_mint,
            &TOKEN_PROGRAM,
        );
        let redemption_vault_token_in_account_22 = get_associated_token_address_with_program_id(
            &redemption_vault_authority,
            &offer.token_in_mint,
            &TOKEN_22_PROGRAM,
        );

        let (excluded_balance_key, _) = Pubkey::find_program_address(
            &[SEED_CIRCULATING_SUPPLY_EXCLUDED_BALANCE],
            &ONRE_PROGRAM_ID,
        );

        Ok(OnreVenue {
            offer_key: *pubkey,
            offer,
            state_key,
            state: None,
            prop_amm_pair_state_key,
            prop_amm_pair_state: None,
            token_info: Vec::new(),
            token_out_supply: 0,

            redemption_offer_key,
            redemption_offer: None,

            redemption_vault_token_in_key: redemption_vault_token_in_account,
            redemption_vault_token_in_22_key: redemption_vault_token_in_account_22,
            redemption_vault_token_in: None,

            circulating_supply_excluded_balance_key: excluded_balance_key,
            circulating_supply_excluded_balance: None,

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

    /// Operational status of the venue. Kill switch (global) dominates the
    /// per-offer disabled flag.
    pub fn status(&self) -> VenueStatus {
        if let Some(state) = &self.state {
            if state.is_killed() {
                return VenueStatus::KillSwitchActive;
            }
        }
        if self.offer.is_disabled() {
            return VenueStatus::OfferDisabled;
        }
        VenueStatus::Active
    }

    /// Get human-readable label
    pub fn label(&self) -> String {
        AMM_LABEL.to_string()
    }

    fn swap_direction(&self, quote_request: &QuoteRequest) -> Result<SwapDirection, OnreError> {
        let offer = self.offer;
        let (req_in, req_out) = (quote_request.input_mint, quote_request.output_mint);

        if (offer.token_in_mint, offer.token_out_mint) == (req_in, req_out) {
            Ok(SwapDirection::Buy)
        } else if (offer.token_in_mint, offer.token_out_mint) == (req_out, req_in) {
            Ok(SwapDirection::Sell)
        } else if offer.token_in_mint == req_in {
            Err(OnreError::InvalidMint(req_out))
        } else {
            Err(OnreError::InvalidMint(req_in))
        }
    }

    /// Get pubkeys required for state update
    pub fn get_required_pubkeys_for_update(&self) -> Result<Vec<Pubkey>, OnreError> {
        Ok(vec![
            self.offer_key,
            self.state_key,
            self.offer.token_in_mint,
            self.offer.token_out_mint,
            self.prop_amm_pair_state_key,
            self.redemption_offer_key,
            self.redemption_vault_token_in_key,
            self.redemption_vault_token_in_22_key,
            self.circulating_supply_excluded_balance_key,
        ])
    }

    /// Update venue state from account cache
    pub async fn update_state(&mut self, cache: &dyn AccountsCache) -> Result<(), OnreError> {
        let pubkeys = vec![
            self.offer_key,
            self.state_key,
            self.offer.token_in_mint,
            self.offer.token_out_mint,
            self.prop_amm_pair_state_key,
            self.redemption_offer_key,
            self.redemption_vault_token_in_key,
            self.redemption_vault_token_in_22_key,
            self.circulating_supply_excluded_balance_key,
        ];

        let accounts = cache.get_accounts(&pubkeys).await?;

        let [
            offer_account,
            state_account,
            token_in_account,
            token_out_account,
            prop_amm_pair_state_account,
            redemption_offer_account,
            redemption_vault_token_in_account,
            redemption_vault_token_in_22_account,
            circulating_supply_excluded_balance_account,
        ]: [Option<Account>; 9] = accounts
            .try_into()
            .map_err(|_| OnreError::FailedToFetchMultipleAccounts)?;

        // Update offer
        let offer_account = offer_account.ok_or(OnreError::NoAccountFound(self.offer_key))?;

        if offer_account.owner != ONRE_PROGRAM_ID {
            return Err(OnreError::InvalidAccountOwner {
                account: self.offer_key,
                expected: ONRE_PROGRAM_ID,
                actual: offer_account.owner,
            });
        }

        let updated_offer = Offer::load(&offer_account.data)?;
        let (expected_offer_key, _) = Pubkey::find_program_address(
            &[
                SEED_OFFER,
                updated_offer.token_in_mint.as_ref(),
                updated_offer.token_out_mint.as_ref(),
            ],
            &ONRE_PROGRAM_ID,
        );

        if expected_offer_key != self.offer_key {
            return Err(OnreError::InvalidPda {
                expected: expected_offer_key,
                actual: self.offer_key,
            });
        }
        self.offer = updated_offer;

        // Update state
        let state_account = state_account.ok_or(OnreError::NoAccountFound(self.state_key))?;

        if state_account.owner != ONRE_PROGRAM_ID {
            return Err(OnreError::InvalidAccountOwner {
                account: self.state_key,
                expected: ONRE_PROGRAM_ID,
                actual: state_account.owner,
            });
        }
        self.state = Some(State::load(&state_account.data)?);

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

        // Update PropAmmPairState
        let prop_amm_pair_state_account = prop_amm_pair_state_account
            .ok_or(OnreError::NoAccountFound(self.prop_amm_pair_state_key))?;
        self.prop_amm_pair_state = Some(PropAmmPairState::load(&prop_amm_pair_state_account.data)?);

        // Now that we know the token program of token_in, we can choose the correct account
        let redemption_vault_token_in_account = if token_in_info.is_token_2022 {
            redemption_vault_token_in_22_account.ok_or(OnreError::NoAccountFound(
                self.redemption_vault_token_in_22_key,
            ))
        } else {
            redemption_vault_token_in_account.ok_or(OnreError::NoAccountFound(
                self.redemption_vault_token_in_key,
            ))
        }?;

        let redemption_vault_token_in =
            StateWithExtensions::<spl_token_2022::state::Account>::unpack(
                &redemption_vault_token_in_account.data,
            )
            .map_err(|_| OnreError::DeserializationFailed(self.redemption_vault_token_in_key))?
            .base;

        self.redemption_vault_token_in = Some(redemption_vault_token_in);

        let redemption_offer_account =
            redemption_offer_account.ok_or(OnreError::NoAccountFound(self.redemption_offer_key))?;
        self.redemption_offer = Some(RedemptionOffer::load(&redemption_offer_account.data)?);

        let circulating_supply_excluded_balance_account =
            circulating_supply_excluded_balance_account.ok_or(OnreError::NoAccountFound(
                self.circulating_supply_excluded_balance_key,
            ))?;

        self.circulating_supply_excluded_balance = Some(CirculatingSupplyExcludedBalance::load(
            &circulating_supply_excluded_balance_account.data,
        )?);

        self.initialized = true;

        Ok(())
    }

    /// Compute a quote for the given swap parameters
    ///
    /// IMPORTANT: This must handle zero-input amounts without error
    pub fn quote(&self, request: QuoteRequest) -> Result<QuoteResult, OnreError> {
        // ExactOut not supported
        if request.swap_type == SwapType::ExactOut {
            return Err(OnreError::ExactOutNotSupported);
        }

        let direction = self.swap_direction(&request)?;

        // Handle zero-input
        if request.amount == 0 {
            return Ok(request.to_zero_result(false));
        }

        // Kill switch or v5 disabled offer: surface as no-liquidity, not an error
        if self.status() != VenueStatus::Active {
            return Ok(request.to_zero_result(true));
        }

        // Get current time for pricing
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| OnreError::TimeError)?
            .as_secs();

        let vectors: [onre_pricing::PriceVector; MAX_VECTORS] =
            self.offer.vectors.map(|vector| vector.into());

        // Find active pricing vector
        let active_vector = match onre_pricing::find_active_vector_at(&vectors, current_time) {
            Ok(vector) => vector,
            Err(_) => {
                return Ok(request.to_zero_result(true));
            }
        };

        // Get token decimals
        let token_in_decimals = self.get_token(0)?.decimals as u8;
        let token_out_decimals = self.get_token(1)?.decimals as u8;

        let amount_out = match direction {
            SwapDirection::Buy => {
                let state = self.state.ok_or(OnreError::NotInitialized)?;

                let amount_out = onre_pricing::buy::calculate_amount_out(
                    active_vector,
                    current_time,
                    request.amount,
                    // Note: We are using the permisionless flow, which has its own fee_basis_points
                    self.offer.fee_basis_points_permissionless,
                    token_in_decimals,
                    token_out_decimals,
                )?;

                // Check max supply
                if state.max_supply > 0 {
                    let new_supply = self
                        .token_out_supply
                        .checked_add(amount_out)
                        .ok_or(OnreError::MathOverflow)?;

                    if new_supply > state.max_supply {
                        return Ok(request.to_zero_result(true));
                    }
                }

                amount_out
            }
            SwapDirection::Sell => {
                let prop_amm_state = self.prop_amm_pair_state.ok_or(OnreError::NotInitialized)?;
                if prop_amm_state.is_disabled() {
                    return Ok(request.to_zero_result(true));
                }

                let redemption_offer = self.redemption_offer.ok_or(OnreError::NotInitialized)?;
                let excluded_balance = self
                    .circulating_supply_excluded_balance
                    .ok_or(OnreError::NotInitialized)?;
                let redemption_vault_token_out = self
                    .redemption_vault_token_in
                    .ok_or(OnreError::NotInitialized)?;

                let circulating_supply = self
                    .token_out_supply
                    .saturating_sub(excluded_balance.amount);

                onre_pricing::sell::calculate_amount_out(
                    active_vector,
                    current_time,
                    request.amount,
                    redemption_offer.fee_basis_points_prop_amm_sell,
                    token_out_decimals,
                    token_in_decimals,
                    prop_amm_state.min_sell_fee(),
                    &onre_pricing::types::LiquidityParams {
                        liquidity: redemption_vault_token_out.amount,
                        liquidity_cap_target_bps: redemption_offer.vault_target_bps,
                        circulating_supply,
                        dampening: prop_amm_state.to_dampening_state(),
                    },
                )?
            }
        };

        Ok(QuoteResult {
            input_mint: request.input_mint,
            output_mint: request.output_mint,
            amount: request.amount,
            expected_output: amount_out,
            not_enough_liquidity: false,
        })
    }

    /// Generate swap instruction for take_offer_permissionless
    #[deprecated(note = "use generate_swap_instruction_v2 for the v5 integration")]
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
    /// per the v5 IDL. This instruction has neither an approval-message
    /// argument nor an Instructions sysvar account.
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
        let permissionless_offer_fee_vault =
            pda(&[SEED_CONFIGURABLE_VAULT, SEED_PERMISSIONLESS_OFFER_FEE_VAULT]);
        let management_fee_vault = pda(&[SEED_CONFIGURABLE_VAULT, SEED_MANAGEMENT_FEE_VAULT]);
        let performance_fee_vault = pda(&[SEED_CONFIGURABLE_VAULT, SEED_PERFORMANCE_FEE_VAULT]);
        let buffer_state = pda(&[SEED_BUFFER_STATE]);
        let reserve_vault_authority = pda(&[SEED_RESERVE_VAULT_AUTHORITY]);
        let market_stats = pda(&[SEED_MARKET_STATS]);
        let excluded_balance = pda(&[SEED_CIRCULATING_SUPPLY_EXCLUDED_BALANCE]);

        let ata = |owner: &Pubkey, mint: &Pubkey, program: &Pubkey| {
            get_associated_token_address_with_program_id(owner, mint, program)
        };

        // Build instruction data: discriminator + amount (u64)
        let mut data = Vec::with_capacity(16);
        data.extend_from_slice(&TAKE_OFFER_PERMISSIONLESS_V2_DISCRIMINATOR);
        data.extend_from_slice(&request.amount.to_le_bytes());

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
            AccountMeta::new(permissionless_offer_fee_vault, false),
            AccountMeta::new(
                ata(
                    &permissionless_offer_fee_vault,
                    &token_in_mint,
                    &token_in_program,
                ),
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
        data[..8].copy_from_slice(&OFFER_ACCOUNT_DISCRIMINATOR);
        data[8..40].copy_from_slice(token_in_mint.as_ref());
        data[40..72].copy_from_slice(token_out_mint.as_ref());
        data[8 + 468] = 1; // allow_permissionless
        data
    }

    /// Builds a minimal v5 State byte blob (borsh layout).
    fn state_bytes(boss: &Pubkey, main_offer: &Pubkey) -> Vec<u8> {
        let mut data = vec![0u8; 8 + 938];
        data[..8].copy_from_slice(&STATE_ACCOUNT_DISCRIMINATOR);
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
    fn test_quote_surfaces_disabled_offer_as_no_liquidity() {
        let token_in_mint = Pubkey::new_unique();
        let token_out_mint = Pubkey::new_unique();
        let mut venue = test_venue(token_in_mint, token_out_mint);

        // Flip the v5 disabled byte on the offer
        let mut data = offer_bytes(&token_in_mint, &token_out_mint);
        data[8 + 469] = 1;
        venue.offer = Offer::load(&data).unwrap();

        assert_eq!(venue.status(), VenueStatus::OfferDisabled);

        let quote = venue
            .quote(QuoteRequest {
                input_mint: token_in_mint,
                output_mint: token_out_mint,
                amount: 1_000_000,
                swap_type: SwapType::ExactIn,
            })
            .unwrap();
        assert!(quote.not_enough_liquidity);
        assert_eq!(quote.expected_output, 0);
    }

    #[test]
    fn test_status_reports_kill_switch() {
        let token_in_mint = Pubkey::new_unique();
        let token_out_mint = Pubkey::new_unique();
        let mut venue = test_venue(token_in_mint, token_out_mint);

        let mut data = state_bytes(&Pubkey::new_unique(), &venue.offer_key);
        data[72] = 1; // is_killed
        venue.state = Some(crate::state::State::load(&data).unwrap());

        assert_eq!(venue.status(), VenueStatus::KillSwitchActive);
    }

    #[test]
    fn test_status_active_when_enabled_and_not_killed() {
        let venue = test_venue(Pubkey::new_unique(), Pubkey::new_unique());
        assert_eq!(venue.status(), VenueStatus::Active);
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
        // discriminator (from v5 IDL) + u64 amount
        let mut expected = vec![250u8, 180, 68, 89, 124, 124, 31, 250];
        expected.extend_from_slice(&1_000_100u64.to_le_bytes());
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

        assert_eq!(ix.accounts.len(), 32);

        // Independent PDA derivations with literal seeds (from the v5 IDL)
        let pda = |seeds: &[&[u8]]| Pubkey::find_program_address(seeds, &ONRE_PROGRAM_ID).0;
        let ata = |owner: &Pubkey, mint: &Pubkey| {
            get_associated_token_address_with_program_id(owner, mint, &TOKEN_PROGRAM)
        };

        let offer_pda = pda(&[b"offer", token_in_mint.as_ref(), token_out_mint.as_ref()]);
        let state_pda = pda(&[b"state"]);
        let vault_authority = pda(&[b"offer_vault_authority"]);
        let permissionless_authority = pda(&[b"permissionless-1"]);
        let redemption_offer = pda(&[
            b"redemption_offer",
            token_out_mint.as_ref(),
            token_in_mint.as_ref(),
        ]);
        let redemption_vault_authority = pda(&[b"redemption_offer_vault_authority"]);
        let proceeds_vault = pda(&[b"configurable_vault", b"offer_proceeds"]);
        let permissionless_offer_fee_vault =
            pda(&[b"configurable_vault", b"permissionless_offer_fee"]);
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
            (
                ata(&redemption_vault_authority, &token_in_mint),
                true,
                false,
            ),
            (proceeds_vault, true, false),
            (ata(&proceeds_vault, &token_in_mint), true, false),
            (permissionless_offer_fee_vault, true, false),
            (
                ata(&permissionless_offer_fee_vault, &token_in_mint),
                true,
                false,
            ),
            (mint_authority, false, false),
            (buffer_state, true, false),
            (ata(&reserve_vault_authority, &token_out_mint), true, false),
            (ata(&management_fee_vault, &token_out_mint), true, false),
            (ata(&performance_fee_vault, &token_out_mint), true, false),
            (market_stats, true, false),
            (excluded_balance, false, false),
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
