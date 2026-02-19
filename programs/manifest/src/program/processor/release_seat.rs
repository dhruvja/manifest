use std::cell::RefMut;

use crate::{
    program::ManifestError,
    require,
    state::{MarketFixed, MarketRefMut},
    validation::{loaders::ReleaseSeatContext, ManifestAccountInfo, Signer},
};
use solana_program::{account_info::AccountInfo, entrypoint::ProgramResult, pubkey::Pubkey};

use super::shared::get_mut_dynamic_account;

pub(crate) fn process_release_seat(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    _data: &[u8],
) -> ProgramResult {
    let release_seat_context: ReleaseSeatContext = ReleaseSeatContext::load(accounts)?;
    let ReleaseSeatContext { market, payer, .. } = release_seat_context;

    process_release_seat_internal(&market, &payer)?;

    Ok(())
}

fn process_release_seat_internal<'a, 'info>(
    market: &ManifestAccountInfo<'a, 'info, MarketFixed>,
    payer: &Signer<'a, 'info>,
) -> ProgramResult {
    let market_data: &mut RefMut<&mut [u8]> = &mut market.try_borrow_mut_data()?;
    let mut dynamic_account: MarketRefMut = get_mut_dynamic_account(market_data);

    // Check that the trader has zero quote balance
    let (base_balance, quote_balance) = dynamic_account.get_trader_balance(payer.key);
    require!(
        quote_balance == crate::quantities::QuoteAtoms::ZERO,
        ManifestError::InvalidWithdrawAccounts,
        "Cannot release seat with non-zero quote balance: {}",
        quote_balance,
    )?;
    require!(
        base_balance == crate::quantities::BaseAtoms::ZERO,
        ManifestError::InvalidWithdrawAccounts,
        "Cannot release seat with non-zero base balance: {}",
        base_balance,
    )?;

    // Check that the trader has no open position
    let (position_size, _cost_basis) = dynamic_account.get_trader_position(payer.key);
    require!(
        position_size == 0,
        ManifestError::InvalidPerpsOperation,
        "Cannot release seat with open position: {}",
        position_size,
    )?;

    dynamic_account.release_seat(payer.key)?;

    Ok(())
}
