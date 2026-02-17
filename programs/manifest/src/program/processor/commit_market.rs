use solana_program::{account_info::AccountInfo, entrypoint::ProgramResult, pubkey::Pubkey};

use crate::{
    require,
    state::MarketFixed,
    validation::{get_market_address, ManifestAccountInfo},
};
use ephemeral_rollups_sdk::consts::{MAGIC_CONTEXT_ID, MAGIC_PROGRAM_ID};
use hypertree::get_helper;
use std::cell::Ref;

pub(crate) fn process_commit_market(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    _data: &[u8],
) -> ProgramResult {
    // accounts[0] = payer (signer, writable)
    // accounts[1] = market (writable, delegated)
    // accounts[2] = magic_program
    // accounts[3] = magic_context

    let payer: &AccountInfo = &accounts[0];
    require!(
        payer.is_signer,
        solana_program::program_error::ProgramError::MissingRequiredSignature,
        "Payer must be signer",
    )?;

    let market: &AccountInfo = &accounts[1];
    let magic_program: &AccountInfo = &accounts[2];
    let magic_context: &AccountInfo = &accounts[3];

    // Validate MagicBlock accounts
    require!(
        *magic_program.key == MAGIC_PROGRAM_ID,
        crate::program::ManifestError::InvalidMagicProgramId,
        "Invalid magic program ID",
    )?;
    require!(
        *magic_context.key == MAGIC_CONTEXT_ID,
        crate::program::ManifestError::InvalidMagicContextId,
        "Invalid magic context ID",
    )?;

    // Use new_delegated since market is owned by delegation program on ER
    let market_info: ManifestAccountInfo<MarketFixed> =
        ManifestAccountInfo::<MarketFixed>::new_delegated(market)?;

    // Verify market PDA
    let market_data: Ref<&mut [u8]> = market_info.try_borrow_data()?;
    let fixed: &MarketFixed = get_helper::<MarketFixed>(&market_data, 0_u32);
    let base_mint_index: u8 = fixed.get_base_mint_index();
    let quote_mint: Pubkey = *fixed.get_quote_mint();
    drop(market_data);

    let (expected_market_key, _bump) = get_market_address(base_mint_index, &quote_mint);
    require!(
        expected_market_key == *market.key,
        crate::program::ManifestError::InvalidMarketPubkey,
        "Market account is not at expected PDA address",
    )?;

    // Commit the delegated account state back to mainnet
    ephemeral_rollups_sdk::ephem::commit_accounts(
        payer,
        vec![market],
        magic_context,
        magic_program,
    )?;

    Ok(())
}
