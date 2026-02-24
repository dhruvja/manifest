use anyhow::Result;
use solana_client::{rpc_client::RpcClient, rpc_config::RpcSendTransactionConfig};
use solana_program::pubkey::Pubkey;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    instruction::Instruction,
    signature::Keypair,
    signer::Signer,
    transaction::Transaction,
};
use spl_associated_token_account::get_associated_token_address;

use manifest::program::{
    batch_update::{CancelOrderParams, PlaceOrderParams},
    batch_update_instruction, claim_seat_instruction::claim_seat_instruction,
    create_market_instructions, crank_funding_instruction, deposit_instruction,
    deposit_instruction_with_vault, expand_market_instruction, liquidate_instruction,
    release_seat_instruction, swap_instruction::swap_instruction_with_vaults,
    withdraw_instruction, withdraw_instruction_with_vault,
};
use manifest::validation::get_market_address;

use crate::config::ManifestConfig;
use crate::ephemeral;
use crate::market::MarketState;
use crate::oracle;
use crate::position::PositionInfo;

/// Parameters for creating a new perps market.
pub struct CreateMarketParams {
    pub base_mint_index: u8,
    pub base_mint_decimals: u8,
    pub quote_mint: Pubkey,
    pub initial_margin_bps: u64,
    pub maintenance_margin_bps: u64,
    pub pyth_feed: Pubkey,
    pub taker_fee_bps: u64,
    pub liquidation_buffer_bps: u64,
    pub num_blocks: u32,
}

/// Parameters for a swap (IOC taker fill with token transfer).
pub struct SwapParams {
    pub quote_mint: Pubkey,
    pub in_atoms: u64,
    pub min_out_atoms: u64,
    /// true = selling base (short), false = buying base (long).
    pub is_base_in: bool,
}

/// High-level client for the Manifest Perps DEX.
///
/// Wraps an `RpcClient` and a [`ManifestConfig`] with typed methods for every
/// on-chain operation.
///
/// # Example
/// ```rust,no_run
/// use manifest_sdk::client::ManifestClient;
/// use manifest_sdk::config::ManifestConfig;
///
/// // Default devnet config
/// let client = ManifestClient::init(ManifestConfig::default());
///
/// // Custom config
/// let config = ManifestConfig::builder()
///     .er_url("https://my-er.example.com")
///     .build();
/// let client = ManifestClient::init(config);
/// ```
pub struct ManifestClient {
    pub rpc: RpcClient,
    pub config: ManifestConfig,
}

impl ManifestClient {
    /// Initialize with a config. Connects to the ER URL from config.
    pub fn init(config: ManifestConfig) -> Self {
        let rpc = RpcClient::new_with_commitment(
            config.er_url.clone(),
            CommitmentConfig::confirmed(),
        );
        Self { rpc, config }
    }

    /// Initialize with a config and explicit RPC URL override (e.g. base chain).
    pub fn init_with_url(config: ManifestConfig, url: &str) -> Self {
        let rpc = RpcClient::new_with_commitment(
            url.to_string(),
            CommitmentConfig::confirmed(),
        );
        Self { rpc, config }
    }

    /// Initialize with a config, explicit URL, and commitment level.
    pub fn init_with_options(
        config: ManifestConfig,
        url: &str,
        commitment: CommitmentConfig,
    ) -> Self {
        let rpc = RpcClient::new_with_commitment(url.to_string(), commitment);
        Self { rpc, config }
    }

    /// Simple constructor using default config and a URL.
    pub fn new(url: &str) -> Self {
        Self::init_with_url(ManifestConfig::default(), url)
    }

    /// Get the config.
    pub fn config(&self) -> &ManifestConfig {
        &self.config
    }

    // ── Read operations ─────────────────────────────────────────────────

    /// Fetch and parse a market's on-chain state.
    pub fn fetch_market(&self, market: &Pubkey) -> Result<MarketState> {
        MarketState::fetch(&self.rpc, market)
    }

    /// Fetch a trader's full position analytics.
    pub fn fetch_position(&self, market: &Pubkey, trader: &Pubkey) -> Result<PositionInfo> {
        let state = self.fetch_market(market)?;
        Ok(PositionInfo::compute(&state, trader))
    }

    /// Fetch oracle price. Tries Pyth V2, then falls back to V3.
    pub fn fetch_oracle_price(
        &self,
        feed: &Pubkey,
        quote_decimals: u8,
        base_decimals: u8,
    ) -> Result<(u32, i8, f64)> {
        oracle::fetch_price(&self.rpc, feed, quote_decimals, base_decimals)
    }

    // ── Write operations ────────────────────────────────────────────────

    /// Create a new perps market. Returns `(market_pubkey, signature)`.
    pub fn create_market(
        &self,
        payer: &Keypair,
        params: CreateMarketParams,
    ) -> Result<(Pubkey, String)> {
        let (market, _) = get_market_address(params.base_mint_index, &params.quote_mint);
        let ixs = create_market_instructions(
            params.base_mint_index,
            params.base_mint_decimals,
            &params.quote_mint,
            &payer.pubkey(),
            params.initial_margin_bps,
            params.maintenance_margin_bps,
            params.pyth_feed,
            params.taker_fee_bps,
            params.liquidation_buffer_bps,
            params.num_blocks,
        );
        let sig = self.send(&ixs, &[payer])?;
        Ok((market, sig))
    }

    /// Claim a trading seat on a market.
    pub fn claim_seat(&self, payer: &Keypair, market: &Pubkey) -> Result<String> {
        let ix = claim_seat_instruction(market, &payer.pubkey());
        self.send(&[ix], &[payer])
    }

    /// Release a trading seat.
    pub fn release_seat(&self, payer: &Keypair, market: &Pubkey) -> Result<String> {
        let ix = release_seat_instruction(market, &payer.pubkey());
        self.send(&[ix], &[payer])
    }

    // /// Expand a market's capacity by one block.
    // pub fn expand_market(&self, payer: &Keypair, market: &Pubkey) -> Result<String> {
    //     let ix = expand_market_instruction(market, &payer.pubkey());
    //     self.send(&[ix], &[payer])
    // }

    /// Deposit USDC margin (on base chain, using standard SPL ATAs).
    pub fn deposit(
        &self,
        payer: &Keypair,
        market: &Pubkey,
        quote_mint: &Pubkey,
        amount: u64,
    ) -> Result<String> {
        let ata = get_associated_token_address(&payer.pubkey(), quote_mint);
        let ix = deposit_instruction(
            market,
            &payer.pubkey(),
            quote_mint,
            amount,
            &ata,
            spl_token::id(),
            None,
        );
        self.send(&[ix], &[payer])
    }

    /// Withdraw USDC margin (on base chain, using standard SPL ATAs).
    pub fn withdraw(
        &self,
        payer: &Keypair,
        market: &Pubkey,
        quote_mint: &Pubkey,
        amount: u64,
    ) -> Result<String> {
        let ata = get_associated_token_address(&payer.pubkey(), quote_mint);
        let ix = withdraw_instruction(
            market,
            &payer.pubkey(),
            quote_mint,
            amount,
            &ata,
            spl_token::id(),
            None,
        );
        self.send(&[ix], &[payer])
    }

    /// Place a single order via BatchUpdate.
    pub fn place_order(
        &self,
        payer: &Keypair,
        market: &Pubkey,
        order: PlaceOrderParams,
    ) -> Result<String> {
        let ix = batch_update_instruction(
            market,
            &payer.pubkey(),
            None,
            vec![],
            vec![order],
            None,
            None,
            None,
            None,
        );
        self.send(&[ix], &[payer])
    }

    /// Cancel a resting order by sequence number via BatchUpdate.
    pub fn cancel_order(
        &self,
        payer: &Keypair,
        market: &Pubkey,
        sequence_number: u64,
    ) -> Result<String> {
        let ix = batch_update_instruction(
            market,
            &payer.pubkey(),
            None,
            vec![CancelOrderParams::new(sequence_number)],
            vec![],
            None,
            None,
            None,
            None,
        );
        self.send(&[ix], &[payer])
    }

    /// Execute a swap (IOC taker fill with token transfer).
    /// Uses ephemeral ATAs — call on the ER client.
    pub fn swap(&self, payer: &Keypair, market: &Pubkey, params: SwapParams) -> Result<String> {
        let (trader_ata, _) =
            ephemeral::get_ephemeral_ata(&self.config, &payer.pubkey(), &params.quote_mint);
        let (vault_ata, _) =
            ephemeral::get_ephemeral_ata(&self.config, market, &params.quote_mint);

        let ix = swap_instruction_with_vaults(
            market,
            &payer.pubkey(),
            &Pubkey::default(),
            &params.quote_mint,
            &Pubkey::default(),
            &trader_ata,
            &Pubkey::default(),
            &vault_ata,
            params.in_atoms,
            params.min_out_atoms,
            params.is_base_in,
            true,
            Pubkey::default(),
            self.config.ephemeral_spl_token_id,
            false,
        );
        self.send(&[ix], &[payer])
    }

    /// Liquidate an underwater trader.
    pub fn liquidate(
        &self,
        liquidator: &Keypair,
        market: &Pubkey,
        trader: &Pubkey,
    ) -> Result<String> {
        let ix = liquidate_instruction(market, &liquidator.pubkey(), trader);
        self.send(&[ix], &[liquidator])
    }

    /// Crank the funding rate (update oracle cache + global cumulative funding).
    pub fn crank_funding(
        &self,
        payer: &Keypair,
        market: &Pubkey,
        pyth_feed: &Pubkey,
    ) -> Result<String> {
        let ix = crank_funding_instruction(market, &payer.pubkey(), pyth_feed);
        self.send(&[ix], &[payer])
    }

    // ── Ephemeral ER operations ─────────────────────────────────────────

    /// Delegate a market account to the MagicBlock ER.
    pub fn delegate_market(
        &self,
        payer: &Keypair,
        market: &Pubkey,
        quote_mint: &Pubkey,
    ) -> Result<String> {
        let ix = ephemeral::delegate_market_ix(&self.config, &payer.pubkey(), market, quote_mint);
        self.send(&[ix], &[payer])
    }

    /// Deposit into a Manifest market using ephemeral ATAs (run on ER).
    pub fn ephemeral_deposit(
        &self,
        payer: &Keypair,
        market: &Pubkey,
        quote_mint: &Pubkey,
        amount: u64,
    ) -> Result<String> {
        let (trader_ata, _) =
            ephemeral::get_ephemeral_ata(&self.config, &payer.pubkey(), quote_mint);
        let (vault_ata, _) = ephemeral::get_ephemeral_ata(&self.config, market, quote_mint);
        let ix = deposit_instruction_with_vault(
            market,
            &payer.pubkey(),
            quote_mint,
            amount,
            &trader_ata,
            &vault_ata,
            self.config.ephemeral_spl_token_id,
            None,
        );
        self.send(&[ix], &[payer])
    }

    /// Withdraw from a Manifest market using ephemeral ATAs (run on ER).
    pub fn ephemeral_withdraw(
        &self,
        payer: &Keypair,
        market: &Pubkey,
        quote_mint: &Pubkey,
        amount: u64,
    ) -> Result<String> {
        let (trader_ata, _) =
            ephemeral::get_ephemeral_ata(&self.config, &payer.pubkey(), quote_mint);
        let (vault_ata, _) = ephemeral::get_ephemeral_ata(&self.config, market, quote_mint);
        let ix = withdraw_instruction_with_vault(
            market,
            &payer.pubkey(),
            quote_mint,
            amount,
            &trader_ata,
            &vault_ata,
            self.config.ephemeral_spl_token_id,
            None,
        );
        self.send(&[ix], &[payer])
    }

    // ── Utility ─────────────────────────────────────────────────────────

    /// Sign and send a transaction. Returns the signature string.
    pub fn send(&self, ixs: &[Instruction], signers: &[&Keypair]) -> Result<String> {
        let blockhash = self.rpc.get_latest_blockhash()?;
        let tx = Transaction::new_signed_with_payer(
            ixs,
            Some(&signers[0].pubkey()),
            signers,
            blockhash,
        );
        let sig = self
            .rpc
            .send_and_confirm_transaction_with_spinner_and_config(
                &tx,
                CommitmentConfig::processed(),
                RpcSendTransactionConfig {
                    skip_preflight: true,
                    ..Default::default()
                },
            )?;
        Ok(sig.to_string())
    }
}
