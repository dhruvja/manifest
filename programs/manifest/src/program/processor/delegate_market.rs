use solana_program::{
    account_info::AccountInfo,
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};

use crate::{
    program::invoke,
    require,
    state::MarketFixed,
    validation::{get_market_address, ManifestAccountInfo},
};
use ephemeral_rollups_sdk::cpi::{DelegateAccounts, DelegateConfig};
use hypertree::get_helper;
use std::cell::Ref;

const EPHEMERAL_SPL_TOKEN_ID: Pubkey =
    solana_program::pubkey!("SPLxh1LVZzEkX99H6rqYizhytLWPZVV296zyYDPagv2");

pub(crate) fn process_delegate_market(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    _data: &[u8],
) -> ProgramResult {
    // accounts[0]  = payer (signer, writable)
    // accounts[1]  = market (writable)
    // accounts[2]  = owner_program (manifest program)
    // accounts[3]  = delegation_program
    // accounts[4]  = delegation_record (writable, for market)
    // accounts[5]  = delegation_metadata (writable, for market)
    // accounts[6]  = system_program
    // accounts[7]  = buffer (writable, for market)
    // accounts[8]  = ephemeral_vault_ata (writable)
    // accounts[9]  = ephemeral_spl_token program
    // accounts[10] = vault_ata_buffer (writable)
    // accounts[11] = vault_ata_delegation_record (writable)
    // accounts[12] = vault_ata_delegation_metadata (writable)

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
    let ephemeral_vault_ata: &AccountInfo = &accounts[8];
    let ephemeral_spl_token: &AccountInfo = &accounts[9];
    let vault_ata_buffer: &AccountInfo = &accounts[10];
    let vault_ata_delegation_record: &AccountInfo = &accounts[11];
    let vault_ata_delegation_metadata: &AccountInfo = &accounts[12];

    // Verify the owner_program is actually our program
    require!(
        *owner_program.key == crate::id(),
        solana_program::program_error::ProgramError::IncorrectProgramId,
        "Owner program must be the Manifest program",
    )?;

    // Verify the market is owned by our program (not yet delegated)
    let market_info: ManifestAccountInfo<MarketFixed> =
        ManifestAccountInfo::<MarketFixed>::new(market)?;

    // Read base_mint_index and quote_mint from the market BEFORE delegation zeroes it
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

    // Verify ephemeral vault ATA is at expected PDA([market, quote_mint], ephemeral_spl_token)
    let (expected_vault_ata_key, vault_ata_bump) = Pubkey::find_program_address(
        &[market.key.as_ref(), quote_mint.as_ref()],
        &EPHEMERAL_SPL_TOKEN_ID,
    );
    require!(
        expected_vault_ata_key == *ephemeral_vault_ata.key,
        crate::program::ManifestError::InvalidMarketPubkey,
        "Ephemeral vault ATA is not at expected PDA address",
    )?;

    // Delegate the ephemeral vault ATA BEFORE delegating the market
    // (market data is still readable at this point)
    // CPI to ephemeral-spl-token disc=4 (DelegateEphemeralAta)
    // accounts: [payer, ata, owner_program(e_spl), buffer, dlg_record, dlg_metadata, dlg_program, system_program]
    invoke(
        &Instruction {
            program_id: EPHEMERAL_SPL_TOKEN_ID,
            accounts: vec![
                AccountMeta::new(*payer.key, true),
                AccountMeta::new(*ephemeral_vault_ata.key, false),
                AccountMeta::new_readonly(*ephemeral_spl_token.key, false),
                AccountMeta::new(*vault_ata_buffer.key, false),
                AccountMeta::new(*vault_ata_delegation_record.key, false),
                AccountMeta::new(*vault_ata_delegation_metadata.key, false),
                AccountMeta::new_readonly(*delegation_program.key, false),
                AccountMeta::new_readonly(*system_program.key, false),
            ],
            // disc=4 (DelegateEphemeralAta), then bump
            data: vec![4u8, vault_ata_bump],
        },
        &[
            payer.clone(),
            ephemeral_vault_ata.clone(),
            ephemeral_spl_token.clone(),
            vault_ata_buffer.clone(),
            vault_ata_delegation_record.clone(),
            vault_ata_delegation_metadata.clone(),
            delegation_program.clone(),
            system_program.clone(),
        ],
    )?;

    // Build PDA seeds for market delegation (without bump - the SDK finds it)
    let pda_seeds: &[&[u8]] = &[b"market", &[base_mint_index], quote_mint.as_ref()];

    // Delegate the market account to the ephemeral rollup (zeroes market data)
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
