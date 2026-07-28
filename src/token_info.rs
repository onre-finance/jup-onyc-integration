use solana_account::Account;
use solana_pubkey::Pubkey;
use spl_associated_token_account::get_associated_token_address_with_program_id;
use spl_token_2022::{
    extension::{BaseStateWithExtensions, StateWithExtensions},
    state::Mint,
};

use crate::constants::{TOKEN_22_PROGRAM, TOKEN_PROGRAM};
use crate::errors::OnreError;

#[derive(Default, Clone, Copy, Debug, Eq, PartialEq)]
pub struct TokenInfo {
    pub pubkey: Pubkey,
    pub decimals: i32,
    pub is_token_2022: bool,
    pub supply: u64,
    pub transfer_fee: Option<u16>,
    pub maximum_fee: Option<u64>,
}

impl TokenInfo {
    pub fn new(pubkey: &Pubkey, account: &Account) -> Result<Self, OnreError> {
        if account.owner != TOKEN_PROGRAM && account.owner != TOKEN_22_PROGRAM {
            return Err(OnreError::UnsupportedTokenProgram(account.owner));
        }

        if let Ok(mint) = StateWithExtensions::<Mint>::unpack(&account.data) {
            let is_token_2022 = account.owner == TOKEN_22_PROGRAM;

            // Extract transfer fee if present (Token-2022 extension)
            let (transfer_fee, maximum_fee) = if is_token_2022 {
                match mint
                    .get_extension::<spl_token_2022::extension::transfer_fee::TransferFeeConfig>()
                {
                    Ok(fee_config) => {
                        let epoch_fee = fee_config.get_epoch_fee(u64::MAX);
                        let fee_bps: u16 = epoch_fee.transfer_fee_basis_points.into();
                        (Some(fee_bps), Some(epoch_fee.maximum_fee.into()))
                    }
                    Err(_) => (None, None),
                }
            } else {
                (None, None)
            };

            Ok(TokenInfo {
                pubkey: *pubkey,
                decimals: mint.base.decimals as i32,
                is_token_2022,
                supply: mint.base.supply,
                transfer_fee,
                maximum_fee,
            })
        } else {
            Err(OnreError::DeserializationFailed(*pubkey))
        }
    }

    /// Get the appropriate token program for this mint
    pub fn get_token_program(&self) -> Pubkey {
        if self.is_token_2022 {
            TOKEN_22_PROGRAM
        } else {
            TOKEN_PROGRAM
        }
    }

    /// Compute the associated token account for a wallet
    pub fn get_associated_token_address(&self, wallet: &Pubkey) -> Pubkey {
        get_associated_token_address_with_program_id(
            wallet,
            &self.pubkey,
            &self.get_token_program(),
        )
    }
}
