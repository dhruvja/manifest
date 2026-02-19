use std::{cell::Ref, mem::size_of};

use crate::{
    logs::{emit_stack, CreateMarketLog},
    program::{get_mut_dynamic_account, invoke},
    require,
    state::{constants::MARKET_BLOCK_SIZE, MarketFixed},
    utils::create_account,
    validation::{
        get_market_address, loaders::CreateMarketContext, ManifestAccountInfo,
    },
};
use borsh::{BorshDeserialize, BorshSerialize};
use hypertree::{get_mut_helper, trace};
use solana_program::{
    account_info::AccountInfo,
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    program_pack::Pack,
    pubkey::Pubkey,
    rent::Rent,
    sysvar::Sysvar,
};
use spl_associated_token_account;
use spl_token_2022::{
    extension::{
        mint_close_authority::MintCloseAuthority, permanent_delegate::PermanentDelegate,
        BaseStateWithExtensions, StateWithExtensions,
    },
    state::Mint,
};

#[derive(BorshDeserialize, BorshSerialize)]
pub struct CreateMarketParams {
    pub base_mint_index: u8,
    pub base_mint_decimals: u8,
    pub initial_margin_bps: u64,
    pub maintenance_margin_bps: u64,
    pub pyth_feed_account: Pubkey,
    pub taker_fee_bps: u64,
    pub liquidation_buffer_bps: u64,
    pub num_blocks: u32,
}

impl CreateMarketParams {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        base_mint_index: u8,
        base_mint_decimals: u8,
        initial_margin_bps: u64,
        maintenance_margin_bps: u64,
        pyth_feed_account: Pubkey,
        taker_fee_bps: u64,
        liquidation_buffer_bps: u64,
        num_blocks: u32,
    ) -> Self {
        CreateMarketParams {
            base_mint_index,
            base_mint_decimals,
            initial_margin_bps,
            maintenance_margin_bps,
            pyth_feed_account,
            taker_fee_bps,
            liquidation_buffer_bps,
            num_blocks,
        }
    }
}

pub(crate) fn process_create_market(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    let params: CreateMarketParams = CreateMarketParams::try_from_slice(data)?;

    // Validate perps parameters
    require!(
        params.maintenance_margin_bps > 0,
        crate::program::ManifestError::InvalidPerpsOperation,
        "Maintenance margin must be > 0",
    )?;
    require!(
        params.initial_margin_bps >= params.maintenance_margin_bps,
        crate::program::ManifestError::InvalidPerpsOperation,
        "Initial margin must be >= maintenance margin",
    )?;
    require!(
        params.initial_margin_bps <= 50000,
        crate::program::ManifestError::InvalidPerpsOperation,
        "Initial margin cannot exceed 500%",
    )?;
    require!(
        params.taker_fee_bps <= 1000,
        crate::program::ManifestError::InvalidPerpsOperation,
        "Taker fee cannot exceed 10%",
    )?;
    require!(
        params.liquidation_buffer_bps < params.maintenance_margin_bps,
        crate::program::ManifestError::InvalidPerpsOperation,
        "Liquidation buffer must be < maintenance margin",
    )?;

    trace!("process_create_market accs={accounts:?}");
    let create_market_context: CreateMarketContext = CreateMarketContext::load(accounts)?;

    let CreateMarketContext {
        market,
        payer,
        quote_mint,
        quote_vault,
        system_program,
        token_program,
        token_program_22,
        associated_token_program,
        ephemeral_vault_ata,
        ephemeral_spl_token,
    } = create_market_context;

    // Check token-2022 extensions on quote mint
    if *quote_mint.as_ref().owner == spl_token_2022::id() {
        let mint_data: Ref<'_, &mut [u8]> = quote_mint.as_ref().data.borrow();
        let pool_mint: StateWithExtensions<'_, Mint> =
            StateWithExtensions::<Mint>::unpack(&mint_data)?;
        if let Ok(extension) = pool_mint.get_extension::<MintCloseAuthority>() {
            let close_authority: Option<Pubkey> = extension.close_authority.into();
            if close_authority.is_some() {
                solana_program::msg!(
                    "Warning, you are creating a market with a close authority."
                );
            }
        }
        if let Ok(extension) = pool_mint.get_extension::<PermanentDelegate>() {
            let permanent_delegate: Option<Pubkey> = extension.delegate.into();
            if permanent_delegate.is_some() {
                solana_program::msg!(
                    "Warning, you are creating a market with a permanent delegate. There is no loss of funds protection for funds on this market"
                );
            }
        }
    }

    {
        let rent: Rent = Rent::get()?;

        solana_program::msg!("base mint index: {}", params.base_mint_index);
        solana_program::msg!("quote mint: {}", quote_mint.info.key);

        // Create the market PDA account using base_mint_index seed
        let (_market_key, market_bump) =
            get_market_address(params.base_mint_index, quote_mint.info.key);
        require!(
            _market_key == *market.info.key,
            crate::program::ManifestError::InvalidMarketPubkey,
            "Market account is not at expected PDA address",
        )?;

        let market_seeds: Vec<Vec<u8>> = vec![
            b"market".to_vec(),
            vec![params.base_mint_index],
            quote_mint.info.key.as_ref().to_vec(),
            vec![market_bump],
        ];
        let total_size: u64 =
            (size_of::<MarketFixed>() + params.num_blocks as usize * MARKET_BLOCK_SIZE) as u64;
        create_account(
            payer.as_ref(),
            market.info,
            system_program.as_ref(),
            &crate::id(),
            &rent,
            total_size,
            market_seeds,
        )?;

        // Create the quote vault as an ATA owned by the market PDA.
        // vault = find_program_address([market, spl_token, mint], associated_token_program)
        {
            let mint = quote_mint.as_ref();
            let is_mint_22: bool = *mint.owner == spl_token_2022::id();
            let token_program_for_mint: Pubkey = if is_mint_22 {
                spl_token_2022::id()
            } else {
                spl_token::id()
            };
            invoke(
                &spl_associated_token_account::instruction::create_associated_token_account(
                    payer.info.key,
                    market.info.key,
                    mint.key,
                    &token_program_for_mint,
                ),
                &[
                    payer.as_ref().clone(),
                    quote_vault.as_ref().clone(),
                    market.info.clone(),
                    mint.clone(),
                    system_program.as_ref().clone(),
                    if is_mint_22 {
                        token_program_22.as_ref().clone()
                    } else {
                        token_program.as_ref().clone()
                    },
                    associated_token_program.as_ref().clone(),
                ],
            )?;
        }

        // Initialize the ephemeral vault ATA (owned by the market PDA, for use on the ER)
        {
            let (ephemeral_vault_ata_key, ephemeral_vault_bump) =
                Pubkey::find_program_address(
                    &[market.info.key.as_ref(), quote_mint.info.key.as_ref()],
                    ephemeral_spl_token.as_ref().key,
                );
            require!(
                ephemeral_vault_ata_key == *ephemeral_vault_ata.info.key,
                crate::program::ManifestError::InvalidMarketPubkey,
                "Ephemeral vault ATA is not at expected PDA address",
            )?;

            invoke(
                &Instruction {
                    program_id: *ephemeral_spl_token.as_ref().key,
                    accounts: vec![
                        AccountMeta::new(*ephemeral_vault_ata.info.key, false),
                        AccountMeta::new(*payer.info.key, true),
                        AccountMeta::new_readonly(*market.info.key, false),
                        AccountMeta::new_readonly(*quote_mint.info.key, false),
                        AccountMeta::new_readonly(*system_program.info.key, false),
                    ],
                    // disc=0 (InitializeEphemeralAta), then bump
                    data: vec![0u8, ephemeral_vault_bump],
                },
                &[
                    ephemeral_vault_ata.as_ref().clone(),
                    payer.as_ref().clone(),
                    market.info.clone(),
                    quote_mint.info.clone(),
                    system_program.as_ref().clone(),
                ],
            )?;
        }

        // Setup the empty market
        let mut empty_market_fixed: MarketFixed =
            MarketFixed::new_empty(params.base_mint_index, params.base_mint_decimals, &quote_mint);

        // Configure margin params
        empty_market_fixed.set_perps_params(
            params.initial_margin_bps,
            params.maintenance_margin_bps,
        );

        // Set the Pyth oracle feed account
        empty_market_fixed.set_pyth_feed(params.pyth_feed_account);

        // Configure insurance fund and liquidation params
        empty_market_fixed.set_taker_fee_bps(params.taker_fee_bps);
        empty_market_fixed.set_liquidation_buffer_bps(params.liquidation_buffer_bps);

        assert_eq!(
            market.info.data_len(),
            size_of::<MarketFixed>() + params.num_blocks as usize * MARKET_BLOCK_SIZE
        );

        {
            let market_bytes: &mut [u8] = &mut market.info.try_borrow_mut_data()?[..];
            *get_mut_helper::<MarketFixed>(market_bytes, 0_u32) = empty_market_fixed;
        }

        // Pre-expand blocks into the free list
        if params.num_blocks > 0 {
            let market_data: &mut std::cell::RefMut<&mut [u8]> =
                &mut market.info.try_borrow_mut_data()?;
            let mut dynamic_account: crate::state::MarketRefMut =
                get_mut_dynamic_account(market_data);
            dynamic_account.market_expand_n(params.num_blocks)?;
        }

        emit_stack(CreateMarketLog {
            market: *market.info.key,
            creator: *payer.key,
            base_mint: Pubkey::default(),
            quote_mint: *quote_mint.info.key,
        })?;
    }

    Ok(())
}
