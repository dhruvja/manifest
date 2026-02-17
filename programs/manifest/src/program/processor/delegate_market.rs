use solana_program::{account_info::AccountInfo, entrypoint::ProgramResult, pubkey::Pubkey};

use crate::{
    require,
    state::MarketFixed,
    validation::{get_market_address, ManifestAccountInfo},
};
use ephemeral_rollups_sdk::cpi::{DelegateAccounts, DelegateConfig};
use hypertree::get_helper;
use std::cell::Ref;

pub(crate) fn process_delegate_market(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    _data: &[u8],
) -> ProgramResult {
    // accounts[0] = payer (signer, writable)
    // accounts[1] = market (writable)
    // accounts[2] = owner_program (manifest program)
    // accounts[3] = delegation_program
    // accounts[4] = delegation_record (writable)
    // accounts[5] = delegation_metadata (writable)
    // accounts[6] = system_program
    // accounts[7] = buffer (writable)

    let payer: &AccountInfo = &accounts[0];
    require!(
        payer.is_signer,
        solana_program::program_error::ProgramError::MissingRequiredSignature,
        "Payer must be signer",
    )?;

    let market: &AccountInfo = &accounts[1];
    let owner_program: &AccountInfo = &accounts[2];
    let delegation_program: &AccountInfo = &accounts[3];
    let delegation_record: &AccountInfo = &accounts[4];
    let delegation_metadata: &AccountInfo = &accounts[5];
    let system_program: &AccountInfo = &accounts[6];
    let buffer: &AccountInfo = &accounts[7];

    // Verify the owner_program is actually our program
    require!(
        *owner_program.key == crate::id(),
        solana_program::program_error::ProgramError::IncorrectProgramId,
        "Owner program must be the Manifest program",
    )?;

    // Verify the market is owned by our program (not yet delegated)
    let market_info: ManifestAccountInfo<MarketFixed> =
        ManifestAccountInfo::<MarketFixed>::new(market)?;

    // Read base_mint_index and quote_mint from the market to verify PDA
    let market_data: Ref<&mut [u8]> = market_info.try_borrow_data()?;
    let fixed: &MarketFixed = get_helper::<MarketFixed>(&market_data, 0_u32);
    let base_mint_index: u8 = fixed.get_base_mint_index();
    let quote_mint: Pubkey = *fixed.get_quote_mint();
    drop(market_data);

    // Verify market is at the expected PDA
    let (expected_market_key, _bump) = get_market_address(base_mint_index, &quote_mint);
    require!(
        expected_market_key == *market.key,
        crate::program::ManifestError::InvalidMarketPubkey,
        "Market account is not at expected PDA address",
    )?;

    // Build PDA seeds for delegation (without bump - the SDK finds it)
    let pda_seeds: &[&[u8]] = &[b"market", &[base_mint_index], quote_mint.as_ref()];

    // Delegate the market account to the ephemeral rollup
    ephemeral_rollups_sdk::cpi::delegate_account(
        DelegateAccounts {
            payer,
            pda: market,
            owner_program,
            buffer,
            delegation_record,
            delegation_metadata,
            delegation_program,
            system_program,
        },
        pda_seeds,
        DelegateConfig {
            commit_frequency_ms: 30,
            validator: None,
        },
    )?;

    Ok(())
}
