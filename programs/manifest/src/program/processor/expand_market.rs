use solana_program::{
    account_info::AccountInfo, entrypoint::ProgramResult, program_error::ProgramError,
    pubkey::Pubkey,
};

use crate::{
    program::{batch_expand_market_escrow, expand_market_escrow, get_dynamic_account},
    state::MarketRef,
    validation::loaders::ExpandMarketContext,
};
use std::cell::Ref;

/// Instruction data layout (after discriminant):
///   [0..4]   num_blocks: u32
///   [4..36]  validator: Pubkey (32 bytes)
///   [36..44] escrow_slot: u64
const EXPAND_DATA_LEN: usize = 4 + 32 + 8; // 44 bytes

pub(crate) fn process_expand_market(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    let expand_market_context: ExpandMarketContext = ExpandMarketContext::load(accounts)?;
    let ExpandMarketContext {
        market,
        payer,
        escrow,
        er_spl_program,
    } = expand_market_context;

    if data.len() < EXPAND_DATA_LEN {
        solana_program::msg!(
            "Expand data too short: {} < {}",
            data.len(),
            EXPAND_DATA_LEN
        );
        return Err(ProgramError::InvalidInstructionData);
    }

    let num_blocks = u32::from_le_bytes(data[0..4].try_into().unwrap());
    let validator = Pubkey::new_from_array(data[4..36].try_into().unwrap());
    let escrow_slot = u64::from_le_bytes(data[36..44].try_into().unwrap());

    if num_blocks <= 1 {
        // Single block expand (or 0 = expand if needed)
        if num_blocks == 0 {
            let has_two_free_blocks: bool = {
                let market_data: Ref<'_, &mut [u8]> = market.try_borrow_data()?;
                let dynamic_account: MarketRef = get_dynamic_account(&market_data);
                dynamic_account.has_two_free_blocks()
            };
            if has_two_free_blocks {
                return Ok(());
            }
        }
        expand_market_escrow(&payer, &market, escrow, er_spl_program, &validator, escrow_slot)
    } else {
        // Batch expand: ensure at least num_blocks free blocks exist
        if let Some(blocks_missing) = {
            let market_data: Ref<'_, &mut [u8]> = market.try_borrow_data()?;
            let dynamic_account: MarketRef = get_dynamic_account(&market_data);
            dynamic_account.free_blocks_short_of_n(num_blocks)
        } {
            batch_expand_market_escrow(
                &payer,
                &market,
                escrow,
                er_spl_program,
                blocks_missing,
                &validator,
                escrow_slot,
            )
        } else {
            Ok(())
        }
    }
}
