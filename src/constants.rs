use solana_pubkey::{pubkey, Pubkey};

pub const ONRE_PROGRAM_ID: Pubkey = pubkey!("onreuGhHHgVzMWSkj2oQDLDtvvGvoepBPkqyaubFcwe");
pub const AMM_LABEL: &str = "OnRe";

// Token Programs
pub const TOKEN_PROGRAM: Pubkey = pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
pub const TOKEN_22_PROGRAM: Pubkey = pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
pub const ASSOCIATED_TOKEN_PROGRAM: Pubkey =
    pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
pub const SYSVAR_INSTRUCTIONS: Pubkey = pubkey!("Sysvar1nstructions1111111111111111111111111");
pub const SYSTEM_PROGRAM: Pubkey = pubkey!("11111111111111111111111111111111");

// PDA Seeds
pub const SEED_STATE: &[u8] = b"state";
pub const SEED_OFFER: &[u8] = b"offer";
pub const SEED_OFFER_VAULT_AUTHORITY: &[u8] = b"offer_vault_authority";
pub const SEED_PERMISSIONLESS_AUTHORITY: &[u8] = b"permissionless-1";
pub const SEED_MINT_AUTHORITY: &[u8] = b"mint_authority";

// v5 PDA Seeds
pub const SEED_REDEMPTION_OFFER: &[u8] = b"redemption_offer";
pub const SEED_REDEMPTION_OFFER_VAULT_AUTHORITY: &[u8] = b"redemption_offer_vault_authority";
pub const SEED_REDEMPTION_REQUEST: &[u8] = b"redemption_request";
pub const SEED_CONFIGURABLE_VAULT: &[u8] = b"configurable_vault";
pub const SEED_OFFER_PROCEEDS_VAULT: &[u8] = b"offer_proceeds";
pub const SEED_PERMISSIONLESS_OFFER_FEE_VAULT: &[u8] = b"permissionless_offer_fee";
pub const SEED_MANAGEMENT_FEE_VAULT: &[u8] = b"management_fee";
pub const SEED_PERFORMANCE_FEE_VAULT: &[u8] = b"performance_fee";
pub const SEED_BUFFER_STATE: &[u8] = b"buffer_state";
pub const SEED_RESERVE_VAULT_AUTHORITY: &[u8] = b"reserve_vault_authority";
pub const SEED_MARKET_STATS: &[u8] = b"market_stats";
pub const SEED_CIRCULATING_SUPPLY_EXCLUDED_BALANCE: &[u8] = b"circ_supply_excl_balance";
pub const SEED_PROP_AMM_PAIR_STATE: &[u8] = b"prop_amm_pair";
pub const SEED_PROP_AMM_PROCEEDS_VAULT: &[u8] = b"prop_amm_proceeds";
pub const SEED_PROP_AMM_SELL_FEE_VAULT: &[u8] = b"prop_amm_sell_fee";

// Price decimals
pub const PRICE_DECIMALS: u8 = 9;

// Maximum basis points (100%)
pub const MAX_BASIS_POINTS: u128 = 10000;

// Seconds in a year for APR calculations
pub const SECONDS_IN_YEAR: u128 = 31_536_000;

// APR scale factor: 1_000_000 = 100%, so 1% = 10,000, 5% = 50,000
pub const APR_SCALE: u128 = 1_000_000;

// Maximum number of pricing vectors per offer
pub const MAX_VECTORS: usize = 10;

// Anchor discriminator length
pub const ANCHOR_DISCRIMINATOR_LEN: usize = 8;

// v5 account discriminators (from target/idl/onreapp.json)
pub const OFFER_ACCOUNT_DISCRIMINATOR: [u8; 8] = [215, 88, 60, 71, 170, 162, 73, 229];
pub const STATE_ACCOUNT_DISCRIMINATOR: [u8; 8] = [216, 146, 107, 94, 104, 75, 182, 177];
pub const REDEMPTION_OFFER_ACCOUNT_DISCRIMINATOR: [u8; 8] = [170, 229, 178, 15, 184, 107, 140, 41];
pub const REDEMPTION_REQUEST_ACCOUNT_DISCRIMINATOR: [u8; 8] = [117, 157, 214, 214, 64, 160, 31, 58];

// Instruction discriminator for take_offer_permissionless
pub const TAKE_OFFER_PERMISSIONLESS_DISCRIMINATOR: [u8; 8] = [37, 190, 224, 77, 197, 39, 203, 230];

// v5 instruction discriminators (from target/idl/onreapp.json)
pub const TAKE_OFFER_PERMISSIONLESS_V2_DISCRIMINATOR: [u8; 8] =
    [250, 180, 68, 89, 124, 124, 31, 250];
pub const CREATE_REDEMPTION_REQUEST_DISCRIMINATOR: [u8; 8] = [201, 53, 181, 254, 115, 137, 70, 151];
pub const QUOTE_SWAP_SELL_DISCRIMINATOR: [u8; 8] = [198, 1, 48, 226, 172, 136, 51, 251];
pub const OPEN_SWAP_SELL_DISCRIMINATOR: [u8; 8] = [93, 206, 188, 72, 45, 138, 181, 71];
