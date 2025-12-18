use solana_pubkey::Pubkey;

pub const ONRE_PROGRAM_ID: Pubkey =
    solana_pubkey::pubkey!("onreuGhHHgVzMWSkj2oQDLDtvvGvoepBPkqyaubFcwe");
pub const AMM_LABEL: &str = "OnRe";

// Token Programs
pub const TOKEN_PROGRAM: Pubkey =
    solana_pubkey::pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
pub const TOKEN_22_PROGRAM: Pubkey =
    solana_pubkey::pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
pub const ASSOCIATED_TOKEN_PROGRAM: Pubkey =
    solana_pubkey::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
pub const SYSVAR_INSTRUCTIONS: Pubkey =
    solana_pubkey::pubkey!("Sysvar1nstructions1111111111111111111111111");
pub const SYSTEM_PROGRAM: Pubkey = solana_pubkey::pubkey!("11111111111111111111111111111111");

// PDA Seeds
pub const SEED_STATE: &[u8] = b"state";
pub const SEED_OFFER: &[u8] = b"offer";
pub const SEED_OFFER_VAULT_AUTHORITY: &[u8] = b"offer_vault_authority";
pub const SEED_PERMISSIONLESS_AUTHORITY: &[u8] = b"permissionless-1";
pub const SEED_MINT_AUTHORITY: &[u8] = b"mint_authority";

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

// Instruction discriminator for take_offer_permissionless
pub const TAKE_OFFER_PERMISSIONLESS_DISCRIMINATOR: [u8; 8] = [37, 190, 224, 77, 197, 39, 203, 230];
