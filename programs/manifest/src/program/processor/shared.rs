use std::{
    cell::{Ref, RefMut},
    mem::size_of,
};

use crate::{
    require,
    state::{
        claimed_seat::ClaimedSeat, constants::MARKET_BLOCK_SIZE, DynamicAccount, GlobalFixed,
        MarketFixed, MarketRefMut, GLOBAL_BLOCK_SIZE,
    },
    validation::{ManifestAccount, ManifestAccountInfo, Signer},
};
use bytemuck::Pod;
use hypertree::{get_helper, get_mut_helper, DataIndex, Get, RBNode};
#[cfg(not(feature = "certora"))]
use solana_program::sysvar::Sysvar;
use solana_program::{
    account_info::AccountInfo,
    entrypoint::ProgramResult,
    instruction::Instruction,
    pubkey::Pubkey,
    sysvar::slot_history::ProgramError,
};
#[cfg(not(feature = "certora"))]
use solana_program::instruction::AccountMeta;

use super::batch_update::MarketDataTreeNodeType;

/// Expand market using lamport escrow from ephemeral-rollups-spl.
/// CPI claims lamports from escrow PDA into the market account, then reallocs.
#[cfg(not(feature = "certora"))]
pub(crate) fn expand_market_escrow<'a, 'info, T: ManifestAccount + Pod + Clone>(
    payer: &Signer<'a, 'info>,
    manifest_account: &ManifestAccountInfo<'a, 'info, T>,
    escrow: &AccountInfo<'info>,
    er_spl_program: &AccountInfo<'info>,
    validator: &Pubkey,
    escrow_slot: u64,
) -> ProgramResult {
    expand_dynamic_escrow(
        payer,
        manifest_account,
        escrow,
        er_spl_program,
        MARKET_BLOCK_SIZE,
        validator,
        escrow_slot,
    )?;
    expand_market_fixed(manifest_account.info)?;
    Ok(())
}

/// Batch expand market using lamport escrow from ephemeral-rollups-spl.
#[cfg(not(feature = "certora"))]
pub(crate) fn batch_expand_market_escrow<'a, 'info, T: ManifestAccount + Pod + Clone>(
    payer: &Signer<'a, 'info>,
    manifest_account: &ManifestAccountInfo<'a, 'info, T>,
    escrow: &AccountInfo<'info>,
    er_spl_program: &AccountInfo<'info>,
    num_blocks: u32,
    validator: &Pubkey,
    escrow_slot: u64,
) -> ProgramResult {
    expand_dynamic_escrow(
        payer,
        manifest_account,
        escrow,
        er_spl_program,
        num_blocks as usize * MARKET_BLOCK_SIZE,
        validator,
        escrow_slot,
    )?;
    expand_market_fixed_n(manifest_account.info, num_blocks)?;
    Ok(())
}

// Expand is always needed because global doesnt free bytes ever.
pub(crate) fn expand_global<'a, 'info, T: ManifestAccount + Pod + Clone>(
    payer: &Signer<'a, 'info>,
    manifest_account: &ManifestAccountInfo<'a, 'info, T>,
) -> ProgramResult {
    // Expand twice because of two trees at once.
    expand_dynamic(payer, manifest_account, 2 * GLOBAL_BLOCK_SIZE)?;
    expand_global_fixed(manifest_account.info)?;
    Ok(())
}

#[cfg(feature = "certora")]
fn expand_dynamic<'a, 'info, T: ManifestAccount + Pod + Clone>(
    _payer: &Signer<'a, 'info>,
    _manifest_account: &ManifestAccountInfo<'a, 'info, T>,
    _block_size: usize,
) -> ProgramResult {
    Ok(())
}
#[cfg(not(feature = "certora"))]
fn expand_dynamic<'a, 'info, T: ManifestAccount + Pod + Clone>(
    payer: &Signer<'a, 'info>,
    manifest_account: &ManifestAccountInfo<'a, 'info, T>,
    block_size: usize,
) -> ProgramResult {
    // Account types were already validated, so do not need to reverify that the
    // accounts are in order: payer, expandable_account, ...
    let expandable_account: &AccountInfo = manifest_account.info;
    let new_size: usize = expandable_account.data_len() + block_size;

    let rent: solana_program::rent::Rent = solana_program::rent::Rent::get()?;
    let new_minimum_balance: u64 = rent.minimum_balance(new_size);
    let old_minimum_balance: u64 = rent.minimum_balance(expandable_account.data_len());
    let lamports_diff: u64 = new_minimum_balance.saturating_sub(old_minimum_balance);

    let payer: &AccountInfo = payer.info;

    invoke(
        &solana_program::system_instruction::transfer(
            payer.key,
            expandable_account.key,
            lamports_diff,
        ),
        &[payer.clone(), expandable_account.clone()],
    )?;

    #[cfg(feature = "fuzz")]
    {
        solana_program::program::invoke(
            &solana_program::system_instruction::allocate(expandable_account.key, new_size as u64),
            &[expandable_account.clone()],
        )?;
    }
    #[cfg(not(feature = "fuzz"))]
    {
        #[allow(deprecated)]
        expandable_account.realloc(new_size, false)?;
    }
    Ok(())
}

/// ephemeral-rollups-spl `lamport_escrow_claim` instruction discriminant.
#[cfg(not(feature = "certora"))]
const LAMPORT_ESCROW_CLAIM_DISCRIMINANT: [u8; 8] =
    [0x62, 0x2B, 0x40, 0xA9, 0xC1, 0xE1, 0x1D, 0x72];

/// Expand dynamic account by claiming lamports from ephemeral-rollups-spl lamport escrow.
/// Used inside MagicBlock ER where `system_instruction::transfer` doesn't work on
/// wallet accounts.
#[cfg(not(feature = "certora"))]
fn expand_dynamic_escrow<'a, 'info, T: ManifestAccount + Pod + Clone>(
    payer: &Signer<'a, 'info>,
    manifest_account: &ManifestAccountInfo<'a, 'info, T>,
    escrow: &AccountInfo<'info>,
    er_spl_program: &AccountInfo<'info>,
    block_size: usize,
    validator: &Pubkey,
    escrow_slot: u64,
) -> ProgramResult {
    let expandable_account: &AccountInfo = manifest_account.info;
    let new_size: usize = expandable_account.data_len() + block_size;

    let rent: solana_program::rent::Rent = solana_program::rent::Rent::get()?;
    let new_minimum_balance: u64 = rent.minimum_balance(new_size);
    let old_minimum_balance: u64 = rent.minimum_balance(expandable_account.data_len());
    let lamports_diff: u64 = new_minimum_balance.saturating_sub(old_minimum_balance);

    // CPI into ephemeral-rollups-spl: lamport_escrow_claim
    // Accounts: [authority (signer), destination (writable), escrow (writable)]
    // Data: [discriminant(8), validator(32), slot(8), lamports(8)]
    let mut claim_data = Vec::with_capacity(8 + 32 + 8 + 8);
    claim_data.extend_from_slice(&LAMPORT_ESCROW_CLAIM_DISCRIMINANT);
    claim_data.extend_from_slice(&validator.to_bytes());
    claim_data.extend_from_slice(&escrow_slot.to_le_bytes());
    claim_data.extend_from_slice(&lamports_diff.to_le_bytes());

    invoke(
        &Instruction {
            program_id: *er_spl_program.key,
            accounts: vec![
                AccountMeta::new_readonly(*payer.info.key, true),
                AccountMeta::new(*expandable_account.key, false),
                AccountMeta::new(*escrow.key, false),
            ],
            data: claim_data,
        },
        &[
            payer.info.clone(),
            expandable_account.clone(),
            escrow.clone(),
        ],
    )?;

    #[cfg(feature = "fuzz")]
    {
        solana_program::program::invoke(
            &solana_program::system_instruction::allocate(expandable_account.key, new_size as u64),
            &[expandable_account.clone()],
        )?;
    }
    #[cfg(not(feature = "fuzz"))]
    {
        #[allow(deprecated)]
        expandable_account.realloc(new_size, false)?;
    }
    Ok(())
}

fn expand_market_fixed(expandable_account: &AccountInfo) -> ProgramResult {
    let market_data: &mut RefMut<&mut [u8]> = &mut expandable_account.try_borrow_mut_data()?;
    let mut dynamic_account: DynamicAccount<&mut MarketFixed, &mut [u8]> =
        get_mut_dynamic_account(market_data);
    dynamic_account.market_expand()?;
    Ok(())
}

fn expand_market_fixed_n(expandable_account: &AccountInfo, n: u32) -> ProgramResult {
    let market_data: &mut RefMut<&mut [u8]> = &mut expandable_account.try_borrow_mut_data()?;
    let mut dynamic_account: DynamicAccount<&mut MarketFixed, &mut [u8]> =
        get_mut_dynamic_account(market_data);
    dynamic_account.market_expand_n(n)?;
    Ok(())
}

fn expand_global_fixed(expandable_account: &AccountInfo) -> ProgramResult {
    let global_data: &mut RefMut<&mut [u8]> = &mut expandable_account.try_borrow_mut_data()?;
    let mut dynamic_account: DynamicAccount<&mut GlobalFixed, &mut [u8]> =
        get_mut_dynamic_account(global_data);
    dynamic_account.global_expand()?;
    Ok(())
}

/// Generic get dynamic account from the data bytes of the account.
pub fn get_dynamic_account<'a, T: Get>(
    data: &'a Ref<'a, &'a mut [u8]>,
) -> DynamicAccount<&'a T, &'a [u8]> {
    let (fixed_data, dynamic) = data.split_at(size_of::<T>());
    let fixed: &T = get_helper::<T>(fixed_data, 0_u32);

    let dynamic_account: DynamicAccount<&'a T, &'a [u8]> = DynamicAccount { fixed, dynamic };
    dynamic_account
}

/// Generic get mutable dynamic account from the data bytes of the account.
pub fn get_mut_dynamic_account<'a, T: Get>(
    data: &'a mut RefMut<'_, &mut [u8]>,
) -> DynamicAccount<&'a mut T, &'a mut [u8]> {
    let (fixed_data, dynamic) = data.split_at_mut(size_of::<T>());
    let fixed: &mut T = get_mut_helper::<T>(fixed_data, 0_u32);

    let dynamic_account: DynamicAccount<&'a mut T, &'a mut [u8]> =
        DynamicAccount { fixed, dynamic };
    dynamic_account
}

pub fn get_dynamic_ref<T: Get>(data: &[u8]) -> DynamicAccount<&'_ T, &'_ [u8]> {
    let (fixed_data, dynamic_data) = data.split_at(size_of::<T>());
    let market_fixed: &T = get_helper::<T>(fixed_data, 0_u32);

    let dynamic_account: DynamicAccount<&T, &[u8]> = DynamicAccount {
        fixed: market_fixed,
        dynamic: dynamic_data,
    };
    dynamic_account
}

/// Generic get owned dynamic account from the data bytes of the account.
pub fn get_dynamic_value<T: Get>(data: &[u8]) -> DynamicAccount<T, Vec<u8>> {
    let (fixed_data, dynamic_data) = data.split_at(size_of::<T>());
    let market_fixed: &T = get_helper::<T>(fixed_data, 0_u32);

    let dynamic_account: DynamicAccount<T, Vec<u8>> = DynamicAccount {
        fixed: *market_fixed,
        dynamic: (dynamic_data).to_vec(),
    };
    dynamic_account
}

// Uses a MarketRefMut instead of a MarketRef because callers will have mutable data.
pub(crate) fn get_trader_index_with_hint(
    trader_index_hint: Option<DataIndex>,
    dynamic_account: &MarketRefMut,
    payer: &Signer,
) -> Result<DataIndex, ProgramError> {
    let trader_index: DataIndex = match trader_index_hint {
        None => dynamic_account.get_trader_index(payer.key),
        Some(hinted_index) => {
            verify_trader_index_hint(hinted_index, &dynamic_account, &payer)?;
            hinted_index
        }
    };
    Ok(trader_index)
}

fn verify_trader_index_hint(
    hinted_index: DataIndex,
    dynamic_account: &MarketRefMut,
    payer: &Signer,
) -> ProgramResult {
    require!(
        hinted_index % (MARKET_BLOCK_SIZE as DataIndex) == 0,
        crate::program::ManifestError::WrongIndexHintParams,
        "Invalid trader hint index {} did not align",
        hinted_index,
    )?;
    require!(
        get_helper::<RBNode<ClaimedSeat>>(&dynamic_account.dynamic, hinted_index)
            .get_payload_type()
            == MarketDataTreeNodeType::ClaimedSeat as u8,
        crate::program::ManifestError::WrongIndexHintParams,
        "Invalid trader hint index {} is not a ClaimedSeat",
        hinted_index,
    )?;
    require!(
        payer
            .key
            .eq(dynamic_account.get_trader_key_by_index(hinted_index)),
        crate::program::ManifestError::WrongIndexHintParams,
        "Invalid trader hint index {} did not match payer",
        hinted_index
    )?;
    Ok(())
}

// TODO: Same for invoke_signed

pub fn invoke(ix: &Instruction, account_infos: &[AccountInfo<'_>]) -> ProgramResult {
    #[cfg(target_os = "solana")]
    {
        solana_invoke::invoke_unchecked(ix, account_infos)
    }
    #[cfg(not(target_os = "solana"))]
    {
        solana_program::program::invoke(ix, account_infos)
    }
}
