//! Shared LiteSVM harness for the v5 integration tests.
//!
//! Only test-side admin/setup builders live here (initialize, make_offer,
//! kill switch, ...). The integrator-facing instruction builders under test
//! come from the `onre_titan` crate itself.

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use litesvm::types::FailedTransactionMetadata;
use litesvm::LiteSVM;
use onre_titan::constants::*;
use onre_titan::errors::OnreError;
use onre_titan::trading_venue::{AccountsCache, FromAccount, OnreVenue};
use solana_account::Account;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;
use solana_sdk::clock::Clock;
use solana_sdk::message::Message;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;
use spl_associated_token_account::get_associated_token_address_with_program_id;

pub const BPF_UPGRADEABLE_LOADER_ID: Pubkey =
    solana_pubkey::pubkey!("BPFLoaderUpgradeab1e11111111111111111111111");
pub const COMPUTE_BUDGET_PROGRAM_ID: Pubkey =
    solana_pubkey::pubkey!("ComputeBudget111111111111111111111111111111");

pub const INITIAL_LAMPORTS: u64 = 1_000_000_000;
pub const USER_USDC_BALANCE: u64 = 10_000_000_000;

pub fn pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &ONRE_PROGRAM_ID).0
}

pub fn derive_ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    get_associated_token_address_with_program_id(owner, mint, &TOKEN_PROGRAM)
}

/// Anchor global instruction discriminator: sha256("global:<name>")[..8]
pub fn ix_discriminator(name: &str) -> [u8; 8] {
    let preimage = format!("global:{}", name);
    let hash = solana_sdk::hash::hash(preimage.as_bytes());
    let mut disc = [0u8; 8];
    disc.copy_from_slice(&hash.to_bytes()[..8]);
    disc
}

fn program_so_path() -> PathBuf {
    if let Ok(path) = std::env::var("ONREAPP_SO_PATH") {
        return PathBuf::from(path);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../onre-sol/target/deploy/onreapp.so")
}

fn load_program(svm: &mut LiteSVM, upgrade_authority: &Pubkey) {
    let path = program_so_path();
    let program_bytes = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "failed to read v5 program binary at {} ({}). Build it with `anchor build` \
             in the onre-sol repo (branch programV5) or set ONREAPP_SO_PATH.",
            path.display(),
            e
        )
    });

    // The program checks its upgrade authority during `initialize`, so it must
    // be installed as an upgradeable program (program account + programdata).
    let program_data_pda =
        Pubkey::find_program_address(&[ONRE_PROGRAM_ID.as_ref()], &BPF_UPGRADEABLE_LOADER_ID).0;

    let mut program_data = vec![0u8; 45 + program_bytes.len()];
    program_data[0..4].copy_from_slice(&3u32.to_le_bytes()); // ProgramData
    program_data[4..12].copy_from_slice(&0u64.to_le_bytes()); // slot
    program_data[12] = 1; // upgrade authority present
    program_data[13..45].copy_from_slice(upgrade_authority.as_ref());
    program_data[45..].copy_from_slice(&program_bytes);
    svm.set_account(
        program_data_pda,
        Account {
            lamports: 100 * INITIAL_LAMPORTS,
            data: program_data,
            owner: BPF_UPGRADEABLE_LOADER_ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();

    let mut program_account = vec![0u8; 36];
    program_account[0..4].copy_from_slice(&2u32.to_le_bytes()); // Program
    program_account[4..36].copy_from_slice(program_data_pda.as_ref());
    svm.set_account(
        ONRE_PROGRAM_ID,
        Account {
            lamports: INITIAL_LAMPORTS,
            data: program_account,
            owner: BPF_UPGRADEABLE_LOADER_ID,
            executable: true,
            rent_epoch: 0,
        },
    )
    .unwrap();
}

pub fn create_mint(svm: &mut LiteSVM, decimals: u8, mint_authority: &Pubkey) -> Pubkey {
    let mint = Keypair::new();
    let mut data = vec![0u8; 82];
    data[0..4].copy_from_slice(&1u32.to_le_bytes()); // authority COption::Some
    data[4..36].copy_from_slice(mint_authority.as_ref());
    data[44] = decimals;
    data[45] = 1; // initialized
    data[46..50].copy_from_slice(&1u32.to_le_bytes()); // freeze authority Some
    data[50..82].copy_from_slice(mint_authority.as_ref());
    svm.set_account(
        mint.pubkey(),
        Account {
            lamports: INITIAL_LAMPORTS,
            data,
            owner: TOKEN_PROGRAM,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
    mint.pubkey()
}

/// Creates an ATA holding `amount` tokens and bumps the mint supply to match.
pub fn create_token_account(
    svm: &mut LiteSVM,
    mint: &Pubkey,
    owner: &Pubkey,
    amount: u64,
) -> Pubkey {
    let ata = derive_ata(owner, mint);
    let mut data = vec![0u8; 165];
    data[0..32].copy_from_slice(mint.as_ref());
    data[32..64].copy_from_slice(owner.as_ref());
    data[64..72].copy_from_slice(&amount.to_le_bytes());
    data[108] = 1; // AccountState::Initialized
    svm.set_account(
        ata,
        Account {
            lamports: INITIAL_LAMPORTS,
            data,
            owner: TOKEN_PROGRAM,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();

    if amount > 0 {
        let mut mint_account = svm.get_account(mint).expect("mint account not found");
        let supply = u64::from_le_bytes(mint_account.data[36..44].try_into().unwrap());
        mint_account.data[36..44].copy_from_slice(&(supply + amount).to_le_bytes());
        svm.set_account(*mint, mint_account).unwrap();
    }
    ata
}

/// Seeds an already-initialized v5 configurable-vault authority.
///
/// LiteSVM 0.6 predates the runtime's SIMD-0312
/// `CreateAccountAllowPrefund` instruction used by current onre-sol for lazy
/// PDA creation. Pre-seeding lets this compatibility suite exercise the real
/// mint instruction; onre-sol's own LiteSVM 0.14 suite covers lazy creation.
pub fn create_configurable_vault(svm: &mut LiteSVM, vault_seed: &[u8], kind: u8) -> Pubkey {
    const CONFIGURABLE_VAULT_DISCRIMINATOR: [u8; 8] = [208, 230, 235, 106, 163, 86, 250, 199];

    let (vault, bump) =
        Pubkey::find_program_address(&[SEED_CONFIGURABLE_VAULT, vault_seed], &ONRE_PROGRAM_ID);
    let mut data = vec![0u8; 73];
    data[..8].copy_from_slice(&CONFIGURABLE_VAULT_DISCRIMINATOR);
    data[8] = kind;
    data[41] = bump;
    svm.set_account(
        vault,
        Account {
            lamports: INITIAL_LAMPORTS,
            data,
            owner: ONRE_PROGRAM_ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
    vault
}

/// Pre-seeds MarketStats for the same LiteSVM 0.6 compatibility reason as
/// `create_configurable_vault`.
pub fn create_market_stats(svm: &mut LiteSVM) -> Pubkey {
    const MARKET_STATS_DISCRIMINATOR: [u8; 8] = [240, 45, 182, 233, 92, 118, 209, 83];

    let (market_stats, bump) = Pubkey::find_program_address(&[SEED_MARKET_STATS], &ONRE_PROGRAM_ID);
    let mut data = vec![0u8; 160];
    data[..8].copy_from_slice(&MARKET_STATS_DISCRIMINATOR);
    data[64] = bump;
    svm.set_account(
        market_stats,
        Account {
            lamports: INITIAL_LAMPORTS,
            data,
            owner: ONRE_PROGRAM_ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
    market_stats
}

pub fn create_circulating_supply_excluded_balance(svm: &mut LiteSVM) -> Pubkey {
    let (key, bump) = Pubkey::find_program_address(
        &[SEED_CIRCULATING_SUPPLY_EXCLUDED_BALANCE],
        &ONRE_PROGRAM_ID,
    );
    // 8 (disc) + 8 (amount) + 8 (last_updated_at) + 8 (last_updated_slot) + 1 (bump) + 31 (reserved)
    let mut data = vec![0u8; 64];
    data[..8].copy_from_slice(&CIRCULATING_SUPPLY_EXCLUDED_BALANCE_ACCOUNT_DISCRIMINATOR);
    data[32] = bump;
    svm.set_account(
        key,
        Account {
            lamports: INITIAL_LAMPORTS,
            data,
            owner: ONRE_PROGRAM_ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
    key
}

// ===========================================================================
// Test-side admin instruction builders (setup only)
// ===========================================================================

pub fn build_initialize_ix(boss: &Pubkey, onyc_mint: &Pubkey) -> Instruction {
    let program_data_pda =
        Pubkey::find_program_address(&[ONRE_PROGRAM_ID.as_ref()], &BPF_UPGRADEABLE_LOADER_ID).0;
    Instruction {
        program_id: ONRE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(pda(&[SEED_STATE]), false),
            AccountMeta::new(pda(&[SEED_MINT_AUTHORITY]), false),
            AccountMeta::new(pda(&[SEED_OFFER_VAULT_AUTHORITY]), false),
            AccountMeta::new(*boss, true),
            AccountMeta::new_readonly(ONRE_PROGRAM_ID, false),
            AccountMeta::new(program_data_pda, false),
            AccountMeta::new_readonly(*onyc_mint, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
        ],
        data: ix_discriminator("initialize").to_vec(),
    }
}

pub fn build_make_offer_ix(
    boss: &Pubkey,
    token_in_mint: &Pubkey,
    token_out_mint: &Pubkey,
    fee_basis_points: u16,
) -> Instruction {
    let vault_authority = pda(&[SEED_OFFER_VAULT_AUTHORITY]);
    let offer = pda(&[SEED_OFFER, token_in_mint.as_ref(), token_out_mint.as_ref()]);
    let mut data = ix_discriminator("make_offer").to_vec();
    data.extend_from_slice(&fee_basis_points.to_le_bytes());
    data.push(0); // needs_approval = false
    data.push(1); // allow_permissionless = true
    Instruction {
        program_id: ONRE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(vault_authority, false),
            AccountMeta::new_readonly(*token_in_mint, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM, false),
            AccountMeta::new(derive_ata(&vault_authority, token_in_mint), false),
            AccountMeta::new_readonly(*token_out_mint, false),
            AccountMeta::new(offer, false),
            AccountMeta::new_readonly(pda(&[SEED_STATE]), false),
            AccountMeta::new(*boss, true),
            AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
        ],
        data,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_add_offer_vector_ix(
    boss: &Pubkey,
    token_in_mint: &Pubkey,
    token_out_mint: &Pubkey,
    start_time: u64,
    base_time: u64,
    base_price: u64,
    apr: u64,
    price_fix_duration: u64,
) -> Instruction {
    let offer = pda(&[SEED_OFFER, token_in_mint.as_ref(), token_out_mint.as_ref()]);
    let mut data = ix_discriminator("add_offer_vector").to_vec();
    data.push(1); // Option::Some(start_time)
    data.extend_from_slice(&start_time.to_le_bytes());
    data.extend_from_slice(&base_time.to_le_bytes());
    data.extend_from_slice(&base_price.to_le_bytes());
    data.extend_from_slice(&apr.to_le_bytes());
    data.extend_from_slice(&price_fix_duration.to_le_bytes());
    Instruction {
        program_id: ONRE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(offer, false),
            AccountMeta::new_readonly(*token_in_mint, false),
            AccountMeta::new_readonly(*token_out_mint, false),
            AccountMeta::new_readonly(pda(&[SEED_STATE]), false),
            AccountMeta::new_readonly(*boss, true),
        ],
        data,
    }
}

pub fn build_set_main_offer_ix(boss: &Pubkey, offer: &Pubkey) -> Instruction {
    Instruction {
        program_id: ONRE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(pda(&[SEED_STATE]), false),
            AccountMeta::new_readonly(*boss, true),
            AccountMeta::new_readonly(*offer, false),
        ],
        data: ix_discriminator("set_main_offer").to_vec(),
    }
}

pub fn build_configure_prop_amm_ix(
    boss: &Pubkey,
    asset_mint: &Pubkey,
    onyc_mint: &Pubkey,
) -> Instruction {
    let offer = pda(&[SEED_OFFER, asset_mint.as_ref(), onyc_mint.as_ref()]);
    let pair_state = pda(&[SEED_PROP_AMM_PAIR_STATE, offer.as_ref()]);
    let mut data = ix_discriminator("configure_prop_amm").to_vec();
    data.push(1); // enabled
    data.extend_from_slice(&700u16.to_le_bytes()); // curve peg haircut
    data.extend_from_slice(&25_000u32.to_le_bytes()); // curve exponent
    data.extend_from_slice(&20u32.to_le_bytes()); // cadence threshold
    data.extend_from_slice(&10_000u32.to_le_bytes()); // cadence wave
    data.extend_from_slice(&86_400i64.to_le_bytes()); // epoch duration
    data.extend_from_slice(&20_000u32.to_le_bytes()); // wall sensitivity
    data.extend_from_slice(&0u64.to_le_bytes()); // no minimum sell size in test
    Instruction {
        program_id: ONRE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(pda(&[SEED_STATE]), false),
            AccountMeta::new_readonly(offer, false),
            AccountMeta::new_readonly(*asset_mint, false),
            AccountMeta::new(pair_state, false),
            AccountMeta::new(*boss, true),
            AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
        ],
        data,
    }
}

pub fn build_make_redemption_offer_ix(
    boss: &Pubkey,
    token_in_mint: &Pubkey,  // token being redeemed (ONyc)
    token_out_mint: &Pubkey, // token returned on fulfillment (USDC)
    fee_basis_points: u16,
    fee_basis_points_prop_amm_sell: u16,
) -> Instruction {
    let vault_authority = pda(&[SEED_REDEMPTION_OFFER_VAULT_AUTHORITY]);
    // Mint-side offer runs the opposite direction
    let offer = pda(&[SEED_OFFER, token_out_mint.as_ref(), token_in_mint.as_ref()]);
    let redemption_offer = pda(&[
        SEED_REDEMPTION_OFFER,
        token_in_mint.as_ref(),
        token_out_mint.as_ref(),
    ]);
    let mut data = ix_discriminator("make_redemption_offer").to_vec();
    data.extend_from_slice(&fee_basis_points.to_le_bytes());
    data.extend_from_slice(&fee_basis_points_prop_amm_sell.to_le_bytes());
    Instruction {
        program_id: ONRE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new_readonly(pda(&[SEED_STATE]), false),
            AccountMeta::new_readonly(offer, false),
            AccountMeta::new_readonly(vault_authority, false),
            AccountMeta::new_readonly(*token_in_mint, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM, false),
            AccountMeta::new(derive_ata(&vault_authority, token_in_mint), false),
            AccountMeta::new_readonly(*token_out_mint, false),
            AccountMeta::new_readonly(TOKEN_PROGRAM, false),
            AccountMeta::new(derive_ata(&vault_authority, token_out_mint), false),
            AccountMeta::new(redemption_offer, false),
            AccountMeta::new(*boss, true),
            AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
        ],
        data,
    }
}

pub fn build_set_offer_disabled_ix(
    signer: &Pubkey,
    token_in_mint: &Pubkey,
    token_out_mint: &Pubkey,
    disabled: bool,
) -> Instruction {
    let offer = pda(&[SEED_OFFER, token_in_mint.as_ref(), token_out_mint.as_ref()]);
    let mut data = ix_discriminator("set_offer_disabled").to_vec();
    data.push(disabled as u8);
    Instruction {
        program_id: ONRE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(offer, false),
            AccountMeta::new_readonly(pda(&[SEED_STATE]), false),
            AccountMeta::new_readonly(*signer, true),
        ],
        data,
    }
}

pub fn build_update_offer_permissionless_fee_ix(
    boss: &Pubkey,
    token_in_mint: &Pubkey,
    token_out_mint: &Pubkey,
    fee_basis_points_permissionless: u16,
) -> Instruction {
    let offer = pda(&[SEED_OFFER, token_in_mint.as_ref(), token_out_mint.as_ref()]);
    let mut data = ix_discriminator("update_offer_permissionless_fee").to_vec();
    data.extend_from_slice(&fee_basis_points_permissionless.to_le_bytes());
    Instruction {
        program_id: ONRE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(offer, false),
            AccountMeta::new_readonly(*token_in_mint, false),
            AccountMeta::new_readonly(*token_out_mint, false),
            AccountMeta::new_readonly(pda(&[SEED_STATE]), false),
            AccountMeta::new_readonly(*boss, true),
        ],
        data,
    }
}

pub fn build_set_redemption_offer_disabled_ix(
    signer: &Pubkey,
    token_in_mint: &Pubkey,
    token_out_mint: &Pubkey,
    disabled: bool,
) -> Instruction {
    let redemption_offer = pda(&[
        SEED_REDEMPTION_OFFER,
        token_in_mint.as_ref(),
        token_out_mint.as_ref(),
    ]);
    let mut data = ix_discriminator("set_redemption_offer_disabled").to_vec();
    data.push(disabled as u8);
    Instruction {
        program_id: ONRE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(redemption_offer, false),
            AccountMeta::new_readonly(pda(&[SEED_STATE]), false),
            AccountMeta::new_readonly(*signer, true),
        ],
        data,
    }
}

pub fn build_set_kill_switch_ix(signer: &Pubkey, enable: bool) -> Instruction {
    let mut data = ix_discriminator("set_kill_switch").to_vec();
    data.push(enable as u8);
    Instruction {
        program_id: ONRE_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(pda(&[SEED_STATE]), false),
            AccountMeta::new_readonly(*signer, true),
        ],
        data,
    }
}

fn compute_budget_ix(units: u32) -> Instruction {
    let mut data = vec![2u8]; // SetComputeUnitLimit
    data.extend_from_slice(&units.to_le_bytes());
    Instruction {
        program_id: COMPUTE_BUDGET_PROGRAM_ID,
        accounts: vec![],
        data,
    }
}

// ===========================================================================
// AccountsCache backed by a LiteSVM snapshot
// ===========================================================================

pub struct SnapshotCache {
    accounts: HashMap<Pubkey, Account>,
}

impl SnapshotCache {
    pub fn from_svm(svm: &LiteSVM, pubkeys: &[Pubkey]) -> Self {
        let mut accounts = HashMap::new();
        for key in pubkeys {
            if let Some(account) = svm.get_account(key) {
                accounts.insert(*key, account);
            }
        }
        SnapshotCache { accounts }
    }
}

#[async_trait]
impl AccountsCache for SnapshotCache {
    async fn get_account(&self, pubkey: &Pubkey) -> Result<Option<Account>, OnreError> {
        Ok(self.accounts.get(pubkey).cloned())
    }

    async fn get_accounts(&self, pubkeys: &[Pubkey]) -> Result<Vec<Option<Account>>, OnreError> {
        Ok(pubkeys
            .iter()
            .map(|k| self.accounts.get(k).cloned())
            .collect())
    }
}

// ===========================================================================
// Test context
// ===========================================================================

pub struct MintOfferCtx {
    pub svm: LiteSVM,
    pub payer: Keypair,
    pub user: Keypair,
    pub usdc_mint: Pubkey,
    pub onyc_mint: Pubkey,
    pub offer_pda: Pubkey,
}

impl MintOfferCtx {
    #[allow(clippy::result_large_err)]
    pub fn send_ixs(
        &mut self,
        ixs: &[Instruction],
        signers: &[&Keypair],
    ) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
        let payer = signers[0].pubkey();
        let mut all_ixs = vec![compute_budget_ix(1_400_000)];
        all_ixs.extend_from_slice(ixs);
        let msg = Message::new(&all_ixs, Some(&payer));
        let tx = Transaction::new(signers, msg, self.svm.latest_blockhash());
        self.svm.send_transaction(tx)
    }

    pub fn token_balance(&self, owner: &Pubkey, mint: &Pubkey) -> u64 {
        let account = self
            .svm
            .get_account(&derive_ata(owner, mint))
            .expect("token account not found");
        u64::from_le_bytes(account.data[64..72].try_into().unwrap())
    }

    pub fn token_balance_or_zero(&self, owner: &Pubkey, mint: &Pubkey) -> u64 {
        self.svm
            .get_account(&derive_ata(owner, mint))
            .map(|account| u64::from_le_bytes(account.data[64..72].try_into().unwrap()))
            .unwrap_or(0)
    }

    /// Builds an initialized OnreVenue the way an integrator would:
    /// from_account on the offer, then update_state through AccountsCache.
    pub fn load_venue(&self) -> OnreVenue {
        let offer_account = self
            .svm
            .get_account(&self.offer_pda)
            .expect("offer missing");
        let mut venue = OnreVenue::from_account(&self.offer_pda, &offer_account)
            .expect("failed to load venue from offer account");

        let keys = venue.get_required_pubkeys_for_update().unwrap();
        let cache = SnapshotCache::from_svm(&self.svm, &keys);
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(venue.update_state(&cache))
            .expect("update_state failed");
        venue
    }

    /// Sets up sell-side accounts with zero-fee defaults so that `load_venue`
    /// / `update_state` can find all required accounts.
    pub fn setup_sell_side_defaults(&mut self) {
        self.setup_redemption_offer();
        let boss = self.payer.insecure_clone();
        let ix = build_configure_prop_amm_ix(&boss.pubkey(), &self.usdc_mint, &self.onyc_mint);
        self.send_ixs(&[ix], &[&boss])
            .expect("configure_prop_amm failed");
        let redemption_vault_authority = pda(&[SEED_REDEMPTION_OFFER_VAULT_AUTHORITY]);
        create_token_account(
            &mut self.svm,
            &self.usdc_mint,
            &redemption_vault_authority,
            0,
        );
    }

    /// Creates the ONyc -> USDC redemption offer (boss-signed).
    pub fn setup_redemption_offer(&mut self) {
        self.setup_redemption_offer_with_fees(0, 0);
    }

    /// Creates the ONyc -> USDC redemption offer with independently configured
    /// regular-redemption and Prop AMM sell fees.
    pub fn setup_redemption_offer_with_fees(
        &mut self,
        fee_basis_points: u16,
        fee_basis_points_prop_amm_sell: u16,
    ) {
        let boss = self.payer.insecure_clone();
        let ix = build_make_redemption_offer_ix(
            &boss.pubkey(),
            &self.onyc_mint,
            &self.usdc_mint,
            fee_basis_points,
            fee_basis_points_prop_amm_sell,
        );
        self.send_ixs(&[ix], &[&boss])
            .expect("make_redemption_offer failed");
    }
}

/// Extracts the custom program error code from a failed transaction.
pub fn custom_error_code(failure: &FailedTransactionMetadata) -> Option<u32> {
    use solana_sdk::instruction::InstructionError;
    use solana_sdk::transaction::TransactionError;
    match &failure.err {
        TransactionError::InstructionError(_, InstructionError::Custom(code)) => Some(*code),
        _ => None,
    }
}

/// Full v5 environment: initialized state, USDC -> ONyc permissionless offer,
/// main offer set, vault/permissionless ATAs seeded and a funded user.
///
/// The pricing vector uses `base_time == start_time == now` with a 1-day
/// `price_fix_duration`, so on-chain and off-chain quotes both land in step 0
/// (constant price) for any `apr`, keeping the tests deterministic.
pub fn setup_mint_offer_with_pricing(
    fee_basis_points: u16,
    fee_basis_points_permissionless: u16,
    base_price: u64,
    apr: u64,
) -> MintOfferCtx {
    let mut svm = LiteSVM::new().with_precompiles();
    let payer = Keypair::new();
    let boss = payer.pubkey();
    svm.airdrop(&boss, 100 * INITIAL_LAMPORTS).unwrap();

    load_program(&mut svm, &boss);

    // Align the chain clock with wall time so venue quotes (SystemTime::now)
    // and on-chain pricing see the same interval.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let clock = Clock {
        slot: 1,
        epoch_start_timestamp: now as i64,
        epoch: 0,
        leader_schedule_epoch: 0,
        unix_timestamp: now as i64,
    };
    svm.set_sysvar(&clock);

    let onyc_mint = create_mint(&mut svm, 9, &boss);
    let usdc_mint = create_mint(&mut svm, 6, &boss);

    let mut ctx = MintOfferCtx {
        svm,
        payer,
        user: Keypair::new(),
        usdc_mint,
        onyc_mint,
        offer_pda: pda(&[SEED_OFFER, usdc_mint.as_ref(), onyc_mint.as_ref()]),
    };

    let boss_kp = ctx.payer.insecure_clone();
    let ix = build_initialize_ix(&boss, &onyc_mint);
    ctx.send_ixs(&[ix], &[&boss_kp]).expect("initialize failed");

    let ix = build_make_offer_ix(&boss, &usdc_mint, &onyc_mint, fee_basis_points);
    ctx.send_ixs(&[ix], &[&boss_kp]).expect("make_offer failed");

    // The permissionless v2 take charges the permissionless fee (a field distinct
    // from make_offer's regular fee), so configure it to match the scenario.
    let ix = build_update_offer_permissionless_fee_ix(
        &boss,
        &usdc_mint,
        &onyc_mint,
        fee_basis_points_permissionless,
    );
    ctx.send_ixs(&[ix], &[&boss_kp])
        .expect("update_offer_permissionless_fee failed");

    let ix = build_set_main_offer_ix(&boss, &ctx.offer_pda);
    ctx.send_ixs(&[ix], &[&boss_kp])
        .expect("set_main_offer failed");

    let ix = build_add_offer_vector_ix(
        &boss, &usdc_mint, &onyc_mint, now, now, base_price, apr, 86_400,
    );
    ctx.send_ixs(&[ix], &[&boss_kp])
        .expect("add_offer_vector failed");

    create_configurable_vault(&mut ctx.svm, SEED_OFFER_PROCEEDS_VAULT, 4);
    create_configurable_vault(&mut ctx.svm, SEED_PERMISSIONLESS_OFFER_FEE_VAULT, 6);
    create_market_stats(&mut ctx.svm);

    // Vault holds pre-minted ONyc so takes are vault-funded
    let vault_authority = pda(&[SEED_OFFER_VAULT_AUTHORITY]);
    create_token_account(
        &mut ctx.svm,
        &onyc_mint,
        &vault_authority,
        1_000_000_000_000,
    );
    create_token_account(&mut ctx.svm, &usdc_mint, &vault_authority, 0);

    let permissionless_authority = pda(&[SEED_PERMISSIONLESS_AUTHORITY]);
    create_token_account(&mut ctx.svm, &usdc_mint, &permissionless_authority, 0);
    create_token_account(&mut ctx.svm, &onyc_mint, &permissionless_authority, 0);

    create_token_account(&mut ctx.svm, &usdc_mint, &boss, 0);

    create_circulating_supply_excluded_balance(&mut ctx.svm);

    let user_pk = ctx.user.pubkey();
    ctx.svm.airdrop(&user_pk, 10 * INITIAL_LAMPORTS).unwrap();
    create_token_account(&mut ctx.svm, &usdc_mint, &user_pk, USER_USDC_BALANCE);
    create_token_account(&mut ctx.svm, &onyc_mint, &user_pk, 0);

    ctx
}

#[allow(clippy::too_many_arguments)]
pub fn build_open_swap_sell_instruction(
    user: &Pubkey,
    onyc_mint: &Pubkey,
    asset_mint: &Pubkey,
    onyc_token_program: &Pubkey,
    asset_token_program: &Pubkey,
    state_main_offer: &Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
) -> Result<Instruction, OnreError> {
    fn ata(owner: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
        get_associated_token_address_with_program_id(owner, mint, token_program)
    }

    let offer = pda(&[SEED_OFFER, asset_mint.as_ref(), onyc_mint.as_ref()]);

    if amount_in > 0 && minimum_amount_out == 0 {
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
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());

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
