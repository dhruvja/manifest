use crate::require;
use solana_program::{account_info::AccountInfo, program_error::ProgramError, pubkey::Pubkey};
use spl_token_2022::{
    check_spl_token_program_account, extension::StateWithExtensions, state::Mint,
};
use std::ops::Deref;

/// Ephemeral SPL Token program ID (from magicblock-labs/ephemeral-spl-token)
pub mod ephemeral_spl_token {
    solana_program::declare_id!("SPLxh1LVZzEkX99H6rqYizhytLWPZVV296zyYDPagv2");
}

/// EphemeralAta data layout: [owner(32), mint(32), amount(8)] = 72 bytes
/// Note: SPL token accounts have [mint(32), owner(32), amount(8), ...] = 165 bytes
pub const EPHEMERAL_ATA_SIZE: usize = 72;

#[derive(Clone)]
pub struct MintAccountInfo<'a, 'info> {
    pub mint: Mint,
    pub info: &'a AccountInfo<'info>,
}

impl<'a, 'info> MintAccountInfo<'a, 'info> {
    pub fn new(info: &'a AccountInfo<'info>) -> Result<MintAccountInfo<'a, 'info>, ProgramError> {
        check_spl_token_program_account(info.owner)?;

        let mint: Mint = StateWithExtensions::<Mint>::unpack(&info.data.borrow())?.base;

        Ok(Self { mint, info })
    }
}

impl<'a, 'info> AsRef<AccountInfo<'info>> for MintAccountInfo<'a, 'info> {
    fn as_ref(&self) -> &AccountInfo<'info> {
        self.info
    }
}

#[derive(Clone)]
pub struct TokenAccountInfo<'a, 'info> {
    pub info: &'a AccountInfo<'info>,
}

impl<'a, 'info> TokenAccountInfo<'a, 'info> {
    /// Returns true if this is an EphemeralAta (72 bytes) rather than an SPL token account.
    pub fn is_ephemeral(&self) -> bool {
        self.info.data_len() == EPHEMERAL_ATA_SIZE
    }

    /// Returns the offset where the mint pubkey is stored.
    /// SPL token account: [0..32], EphemeralAta: [32..64]
    fn mint_offset(info: &AccountInfo) -> usize {
        if info.data_len() == EPHEMERAL_ATA_SIZE {
            32
        } else {
            0
        }
    }

    /// Returns the offset where the owner pubkey is stored.
    /// SPL token account: [32..64], EphemeralAta: [0..32]
    fn owner_offset(info: &AccountInfo) -> usize {
        if info.data_len() == EPHEMERAL_ATA_SIZE {
            0
        } else {
            32
        }
    }

    pub fn new(
        info: &'a AccountInfo<'info>,
        mint: &Pubkey,
    ) -> Result<TokenAccountInfo<'a, 'info>, ProgramError> {
        if info.data_len() == EPHEMERAL_ATA_SIZE {
            // EphemeralAta: skip owner check (may be ephemeral-spl-token or delegation program)
        } else {
            require!(
                info.owner == &spl_token::id() || info.owner == &spl_token_2022::id(),
                ProgramError::IllegalOwner,
                "Token account must be owned by the Token Program",
            )?;
        }
        let mint_off = Self::mint_offset(info);
        require!(
            &info.try_borrow_data()?[mint_off..mint_off + 32] == mint.as_ref(),
            ProgramError::InvalidAccountData,
            "Token account mint mismatch",
        )?;
        Ok(Self { info })
    }

    pub fn get_owner(&self) -> Pubkey {
        let off = Self::owner_offset(self.info);
        Pubkey::new_from_array(
            self.info.try_borrow_data().unwrap()[off..off + 32]
                .try_into()
                .unwrap(),
        )
    }

    /// Balance is always at offset 64..72 for both SPL and EphemeralAta.
    pub fn get_balance_atoms(&self) -> u64 {
        u64::from_le_bytes(
            self.info.try_borrow_data().unwrap()[64..72]
                .try_into()
                .unwrap(),
        )
    }

    pub fn new_with_owner(
        info: &'a AccountInfo<'info>,
        mint: &Pubkey,
        owner: &Pubkey,
    ) -> Result<TokenAccountInfo<'a, 'info>, ProgramError> {
        let token_account_info = Self::new(info, mint)?;
        let off = Self::owner_offset(info);
        require!(
            &info.try_borrow_data()?[off..off + 32] == owner.as_ref(),
            ProgramError::IllegalOwner,
            "Token account owner mismatch",
        )?;
        Ok(token_account_info)
    }

    pub fn new_with_owner_and_key(
        info: &'a AccountInfo<'info>,
        mint: &Pubkey,
        owner: &Pubkey,
        key: &Pubkey,
    ) -> Result<TokenAccountInfo<'a, 'info>, ProgramError> {
        require!(
            info.key == key,
            ProgramError::InvalidInstructionData,
            "Invalid pubkey for Token Account {:?}",
            info.key
        )?;
        Self::new_with_owner(info, mint, owner)
    }
}

impl<'a, 'info> AsRef<AccountInfo<'info>> for TokenAccountInfo<'a, 'info> {
    fn as_ref(&self) -> &AccountInfo<'info> {
        self.info
    }
}

impl<'a, 'info> Deref for TokenAccountInfo<'a, 'info> {
    type Target = AccountInfo<'info>;

    fn deref(&self) -> &Self::Target {
        self.info
    }
}

#[macro_export]
macro_rules! market_vault_seeds {
    ( $market:expr, $mint:expr ) => {
        &[b"vault", $market.as_ref(), $mint.as_ref()]
    };
}

#[macro_export]
macro_rules! market_vault_seeds_with_bump {
    ( $market:expr, $mint:expr, $bump:expr ) => {
        &[&[b"vault", $market.as_ref(), $mint.as_ref(), &[$bump]]]
    };
}

#[macro_export]
macro_rules! global_vault_seeds {
    ( $mint:expr ) => {
        &[b"global-vault", $mint.as_ref()]
    };
}

#[macro_export]
macro_rules! global_vault_seeds_with_bump {
    ( $mint:expr, $bump:expr ) => {
        &[&[b"global-vault", $mint.as_ref(), &[$bump]]]
    };
}

pub fn get_vault_address(market: &Pubkey, mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(market_vault_seeds!(market, mint), &crate::ID)
}

pub fn get_global_vault_address(mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(global_vault_seeds!(mint), &crate::ID)
}
