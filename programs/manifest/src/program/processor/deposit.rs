use std::cell::RefMut;

use crate::{
    logs::{emit_stack, DepositLog},
    state::MarketRefMut,
    validation::{
        loaders::DepositContext,
        Signer, TokenAccountInfo, TokenProgram,
    },
};
use borsh::{BorshDeserialize, BorshSerialize};
use hypertree::DataIndex;
use solana_program::{account_info::AccountInfo, entrypoint::ProgramResult, pubkey::Pubkey};

use super::{get_trader_index_with_hint, shared::get_mut_dynamic_account};

#[cfg(feature = "certora")]
use early_panic::early_panic;
#[cfg(feature = "certora")]
use solana_cvt::token::spl_token_transfer;

#[derive(BorshDeserialize, BorshSerialize)]
pub struct DepositParams {
    pub amount_atoms: u64,
    pub trader_index_hint: Option<DataIndex>,
}

impl DepositParams {
    pub fn new(amount_atoms: u64, trader_index_hint: Option<DataIndex>) -> Self {
        DepositParams {
            amount_atoms,
            trader_index_hint,
        }
    }
}

pub(crate) fn process_deposit(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    let params: DepositParams = DepositParams::try_from_slice(data)?;
    process_deposit_core(program_id, accounts, params)
}

#[cfg_attr(all(feature = "certora", not(feature = "certora-test")), early_panic)]
pub(crate) fn process_deposit_core(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    params: DepositParams,
) -> ProgramResult {
    let deposit_context: DepositContext = DepositContext::load(accounts)?;
    let DepositParams {
        amount_atoms,
        trader_index_hint,
    } = params;
    let deposited_amount_atoms: u64 = amount_atoms;

    let DepositContext {
        market,
        payer,
        trader_token,
        vault,
        token_program,
        mint: _,
    } = deposit_context;

    let market_data: &mut RefMut<&mut [u8]> = &mut market.try_borrow_mut_data()?;
    let mut dynamic_account: MarketRefMut = get_mut_dynamic_account(market_data);

    // Perps: only quote (USDC) deposits allowed — base is virtual
    // The loader already validates that the trader_token is for the quote mint.

    // CPI to token program (ephemeral-spl-token on ER)
    spl_token_transfer_from_trader_to_vault(
        &token_program,
        &trader_token,
        &vault,
        &payer,
        amount_atoms,
    )?;

    let trader_index: DataIndex =
        get_trader_index_with_hint(trader_index_hint, &dynamic_account, &payer)?;
    // is_base = false: always depositing quote in perps
    dynamic_account.deposit(trader_index, deposited_amount_atoms, false)?;

    emit_stack(DepositLog {
        market: *market.key,
        trader: *payer.key,
        mint: *dynamic_account.get_quote_mint(),
        amount_atoms: deposited_amount_atoms,
    })?;

    Ok(())
}

/** Transfer from base (quote) trader to base (quote) vault using SPL Token **/
#[cfg(not(feature = "certora"))]
fn spl_token_transfer_from_trader_to_vault<'a, 'info>(
    token_program: &TokenProgram<'a, 'info>,
    trader_account: &TokenAccountInfo<'a, 'info>,
    vault: &TokenAccountInfo<'a, 'info>,
    payer: &Signer<'a, 'info>,
    amount: u64,
) -> ProgramResult {
    crate::program::invoke(
        &spl_token::instruction::transfer(
            token_program.key,
            trader_account.key,
            vault.key,
            payer.key,
            &[],
            amount,
        )?,
        &[
            token_program.as_ref().clone(),
            trader_account.as_ref().clone(),
            vault.as_ref().clone(),
            payer.as_ref().clone(),
        ],
    )
}
#[cfg(feature = "certora")]
/** (Summary) Transfer from base (quote) trader to base (quote) vault using SPL Token **/
fn spl_token_transfer_from_trader_to_vault<'a, 'info>(
    _token_program: &TokenProgram<'a, 'info>,
    trader_account: &TokenAccountInfo<'a, 'info>,
    vault: &TokenAccountInfo<'a, 'info>,
    payer: &Signer<'a, 'info>,
    amount: u64,
) -> ProgramResult {
    spl_token_transfer(trader_account.info, vault.info, payer.info, amount)
}

