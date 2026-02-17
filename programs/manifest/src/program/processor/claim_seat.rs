use std::cell::RefMut;

use crate::{
    logs::{emit_stack, ClaimSeatLog},
    state::{MarketFixed, MarketRefMut},
    validation::{loaders::ClaimSeatContext, ManifestAccountInfo, Signer},
};
use solana_program::{account_info::AccountInfo, entrypoint::ProgramResult, pubkey::Pubkey};

use super::shared::get_mut_dynamic_account;

#[cfg(feature = "certora")]
use early_panic::early_panic;

#[cfg_attr(all(feature = "certora", not(feature = "certora-test")), early_panic)]
pub(crate) fn process_claim_seat(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    _data: &[u8],
) -> ProgramResult {
    let claim_seat_context: ClaimSeatContext = ClaimSeatContext::load(accounts)?;
    let ClaimSeatContext { market, payer, .. } = claim_seat_context;

    // Require a free block to exist before claiming — market must be pre-expanded
    // via the Expand instruction. Cannot expand here since realloc fails while delegated.
    {
        use crate::{program::get_dynamic_account, program::ManifestError, require};
        let market_data = market.try_borrow_data()?;
        let dynamic_account = get_dynamic_account(&market_data);
        require!(
            dynamic_account.has_free_block(),
            ManifestError::InvalidFreeList,
            "No free block available. Call Expand before ClaimSeat.",
        )?;
    }

    process_claim_seat_internal(&market, &payer)?;

    Ok(())
}

#[cfg_attr(all(feature = "certora", not(feature = "certora-test")), early_panic)]
pub(crate) fn process_claim_seat_internal<'a, 'info>(
    market: &ManifestAccountInfo<'a, 'info, MarketFixed>,
    payer: &Signer<'a, 'info>,
) -> ProgramResult {
    let market_data: &mut RefMut<&mut [u8]> = &mut market.try_borrow_mut_data()?;
    let mut dynamic_account: MarketRefMut = get_mut_dynamic_account(market_data);
    dynamic_account.claim_seat(payer.key)?;

    emit_stack(ClaimSeatLog {
        market: *market.key,
        trader: *payer.key,
    })?;

    Ok(())
}
