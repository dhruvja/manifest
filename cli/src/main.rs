//! manifest-cli — command-line interface for the Manifest Perps DEX.
//!
//! Usage:
//!   manifest-cli [--url <RPC>] [--keypair <PATH>] <COMMAND> [OPTIONS]
//!
//! Commands: create-mint  mint-to  create-market  expand  claim-seat
//!           deposit  withdraw  place-order  cancel-order  delegate
//!           crank-funding  liquidate  fetch-price  market-info  setup
use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use manifest::{
    program::{
        batch_update::{CancelOrderParams, PlaceOrderParams},
        batch_update_instruction,
        claim_seat_instruction::claim_seat_instruction,
        crank_funding_instruction, create_market_instructions,
        deposit_instruction, deposit_instruction_with_vault, expand_market_instruction, expand_market_n_instruction,
        liquidate_instruction, release_seat_instruction, withdraw_instruction, withdraw_instruction_with_vault,
        ManifestInstruction,
    },
    state::OrderType,
    validation::{get_market_address, get_vault_address},
};
use solana_client::{rpc_client::RpcClient, rpc_config::RpcSendTransactionConfig};
use solana_program::{pubkey::Pubkey, system_program};
use solana_sdk::{
    commitment_config::CommitmentConfig,
    instruction::{AccountMeta, Instruction},
    signature::{read_keypair_file, Keypair},
    signer::Signer,
    system_instruction,
    transaction::Transaction,
};
use spl_associated_token_account::{
    get_associated_token_address, instruction::create_associated_token_account_idempotent,
};
use std::str::FromStr;

// ─── default constants ───────────────────────────────────────────────────────

const DEFAULT_URL: &str = "https://api.devnet.solana.com";
const ER_URL: &str = "https://devnet.magicblock.app";
const DELEGATION_PROGRAM_ID: &str = "DELeGGvXpWV2fqJUhqcF5ZSYMS4JTLjteaAMARRSaeSh";
/// magicblock-labs ephemeral-spl-token program
const EPHEMERAL_SPL_TOKEN: &str = "SPLxh1LVZzEkX99H6rqYizhytLWPZVV296zyYDPagv2";
/// Pyth V2 SOL/USD devnet price account
const PYTH_SOL_USD_DEVNET: &str = "J83w4HKfqxwcq3BEMMkPFSppX3gqekLyLJBexebFVkix";
/// Pyth PriceUpdateV3 SOL/USD on MagicBlock ER
const PYTH_SOL_USD_ER: &str = "ENYwebBThHzmzwPLAQvCucUTsjyfBSZdD9ViXksS4jPu";

// ─── persisted config ────────────────────────────────────────────────────────

/// Persisted config file at ~/.config/manifest-cli/config (key=value, one per line).
#[derive(Debug, Default)]
struct CliConfig {
    url: Option<String>,
    keypair: Option<String>,
}

fn config_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home)
        .join(".config")
        .join("manifest-cli")
        .join("config")
}

impl CliConfig {
    fn load() -> Self {
        let path = config_path();
        let mut cfg = CliConfig::default();
        let Ok(contents) = std::fs::read_to_string(&path) else {
            return cfg;
        };
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                match k.trim() {
                    "url" => cfg.url = Some(v.trim().to_string()),
                    "keypair" => cfg.keypair = Some(v.trim().to_string()),
                    _ => {}
                }
            }
        }
        cfg
    }

    fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = String::new();
        if let Some(u) = &self.url {
            out.push_str(&format!("url={u}\n"));
        }
        if let Some(k) = &self.keypair {
            out.push_str(&format!("keypair={k}\n"));
        }
        std::fs::write(&path, out)?;
        Ok(())
    }

    /// Effective URL: CLI flag > config file > hardcoded default.
    fn resolve_url<'a>(&'a self, flag: Option<&'a str>) -> &'a str {
        flag.or(self.url.as_deref()).unwrap_or(DEFAULT_URL)
    }

    /// Effective keypair path: CLI flag > config file > default.
    fn resolve_keypair<'a>(&'a self, flag: Option<&'a str>) -> Option<&'a str> {
        flag.or(self.keypair.as_deref())
    }
}

// ─── CLI definition ──────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "manifest-cli",
    about = "CLI for the Manifest Perps DEX",
    version
)]
struct Cli {
    /// Solana RPC URL — overrides config. Use 'er' for MagicBlock ER.
    #[arg(short, long)]
    url: Option<String>,

    /// Path to the payer keypair JSON file — overrides config.
    #[arg(short, long)]
    keypair: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new SPL token mint (returns mint address)
    CreateMint {
        /// Token decimals
        #[arg(long, default_value = "6")]
        decimals: u8,
    },

    /// Create an ATA for <recipient> and mint <amount> tokens into it
    MintTo {
        /// Mint address
        #[arg(long)]
        mint: String,
        /// Token amount in base units (atoms)
        #[arg(long)]
        amount: u64,
        /// Recipient address — ATA owner who receives the tokens (defaults to payer)
        #[arg(long)]
        recipient: Option<String>,
    },

    /// Create a new perps market
    CreateMarket {
        /// Quote mint address (USDC)
        #[arg(long)]
        quote_mint: String,
        /// Base asset index (unique per base asset, e.g. 0 = SOL)
        #[arg(long, default_value = "0")]
        base_mint_index: u8,
        /// Base asset decimals
        #[arg(long, default_value = "9")]
        base_decimals: u8,
        /// Initial margin in bps (e.g. 1000 = 10%)
        #[arg(long, default_value = "1000")]
        initial_margin_bps: u64,
        /// Maintenance margin in bps (e.g. 500 = 5%)
        #[arg(long, default_value = "500")]
        maintenance_margin_bps: u64,
        /// Pyth price feed account (defaults to SOL/USD devnet feed)
        #[arg(long)]
        pyth_feed: Option<String>,
        /// Taker fee in bps (e.g. 5 = 0.05%)
        #[arg(long, default_value = "5")]
        taker_fee_bps: u64,
        /// Liquidation buffer above maintenance margin in bps
        #[arg(long, default_value = "200")]
        liquidation_buffer_bps: u64,
        /// Number of blocks to pre-allocate (each block = 80 bytes for a seat or order)
        #[arg(long, default_value = "0")]
        num_blocks: u32,
    },

    /// Expand a market's free block capacity
    Expand {
        /// Market PDA address
        #[arg(long)]
        market: String,
        /// Number of blocks to add
        #[arg(long, default_value = "10")]
        blocks: u32,
    },

    /// Claim a trading seat on a market
    ClaimSeat {
        /// Market PDA address
        #[arg(long)]
        market: String,
        /// Trader to claim seat for (defaults to payer)
        #[arg(long)]
        trader: Option<String>,
    },

    /// Release a trading seat (must have zero balances and no position)
    ReleaseSeat {
        /// Market PDA address
        #[arg(long)]
        market: String,
    },

    /// Deposit USDC margin into a market seat
    Deposit {
        /// Market PDA address
        #[arg(long)]
        market: String,
        /// Quote mint address
        #[arg(long)]
        quote_mint: String,
        /// Amount in quote atoms (e.g. 1000000 = 1 USDC with 6 decimals)
        #[arg(long)]
        amount: u64,
    },

    /// Withdraw USDC margin from a market seat
    Withdraw {
        /// Market PDA address
        #[arg(long)]
        market: String,
        /// Quote mint address
        #[arg(long)]
        quote_mint: String,
        /// Amount in quote atoms
        #[arg(long)]
        amount: u64,
    },

    /// Place a limit or IOC order via BatchUpdate
    PlaceOrder {
        /// Market PDA address
        #[arg(long)]
        market: String,
        /// Size in base atoms (e.g. 1000000 = 0.001 SOL)
        #[arg(long)]
        base_atoms: u64,
        /// Price mantissa (price = mantissa * 10^exponent quote/base atoms)
        #[arg(long)]
        price_mantissa: u32,
        /// Price exponent (typically negative, e.g. -3 for USDC/SOL)
        #[arg(long)]
        price_exponent: i8,
        /// true = buy (bid), false = sell (ask)
        #[arg(long)]
        is_bid: bool,
        /// Order type: limit | ioc | post-only
        #[arg(long, default_value = "limit")]
        order_type: String,
        /// Slot after which order expires (0 = no expiry)
        #[arg(long, default_value = "0")]
        last_valid_slot: u32,
    },

    /// Cancel a resting order by sequence number via BatchUpdate
    CancelOrder {
        /// Market PDA address
        #[arg(long)]
        market: String,
        /// Order sequence number returned when the order was placed
        #[arg(long)]
        sequence_number: u64,
    },

    /// Delegate a market account to the MagicBlock Ephemeral Rollup
    Delegate {
        /// Market PDA address
        #[arg(long)]
        market: String,
        /// Quote mint address (needed to derive the ephemeral vault ATA)
        #[arg(long)]
        quote_mint: String,
    },

    /// Crank the funding rate (updates oracle cache + global cumulative funding)
    CrankFunding {
        /// Market PDA address
        #[arg(long)]
        market: String,
        /// Pyth price feed account (defaults to SOL/USD devnet)
        #[arg(long)]
        pyth_feed: Option<String>,
    },

    /// Liquidate an underwater trader
    Liquidate {
        /// Market PDA address
        #[arg(long)]
        market: String,
        /// Address of the trader to liquidate
        #[arg(long)]
        trader: String,
    },

    /// Fetch and display the live Pyth oracle price
    FetchPrice {
        /// Pyth price feed address (defaults to SOL/USD devnet)
        #[arg(long)]
        feed: Option<String>,
        /// Quote token decimals
        #[arg(long, default_value = "6")]
        quote_decimals: u8,
        /// Base token decimals
        #[arg(long, default_value = "9")]
        base_decimals: u8,
    },

    /// Open a leveraged long via the ER. Fetches oracle price automatically.
    OpenLong {
        /// Market PDA address
        #[arg(long)]
        market: String,
        /// Leverage multiplier (e.g. 10 for 10x)
        #[arg(long)]
        leverage: u32,
        /// Margin already deposited, in quote atoms (e.g. 5000000 = 5 USDC)
        #[arg(long)]
        margin_atoms: u64,
        /// Quote token decimals (default 6 for USDC)
        #[arg(long, default_value = "6")]
        quote_decimals: u8,
        /// Base token decimals (default 9 for SOL)
        #[arg(long, default_value = "9")]
        base_decimals: u8,
    },

    /// Show basic info about a market account
    MarketInfo {
        /// Market PDA address
        #[arg(long)]
        market: String,
    },

    /// Run the full demo setup: mint → market → expand → seat → deposit → delegate
    ///   → maker ASK → taker BID → open long → close long → PnL
    Setup {
        /// Base asset index
        #[arg(long, default_value = "0")]
        base_mint_index: u8,
        /// Initial margin bps
        #[arg(long, default_value = "1000")]
        initial_margin_bps: u64,
        /// Maintenance margin bps
        #[arg(long, default_value = "500")]
        maintenance_margin_bps: u64,
        /// Taker fee bps
        #[arg(long, default_value = "5")]
        taker_fee_bps: u64,
    },

    /// Read or write the persistent CLI config (~/.config/manifest-cli/config)
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    // ── Ephemeral SPL Token commands ────────────────────────────────────────
    /// Initialize the ephemeral-spl-token global vault for a mint.
    /// Must be called once per mint before any ephemeral deposits.
    EphemeralInitGlobalVault {
        /// Quote mint address
        #[arg(long)]
        mint: String,
    },

    /// Initialize an ephemeral ATA for an owner.
    /// For the user's ATA use `--owner <user>` (defaults to payer).
    /// For the market vault ATA use `--owner <market>`.
    EphemeralInitAta {
        /// Quote mint address
        #[arg(long)]
        mint: String,
        /// Owner of the ephemeral ATA (defaults to payer)
        #[arg(long)]
        owner: Option<String>,
    },

    /// Deposit SPL tokens from the payer's regular ATA into their ephemeral ATA.
    /// Locks real USDC in the global vault and credits the ephemeral balance.
    EphemeralDepositSpl {
        /// Quote mint address
        #[arg(long)]
        mint: String,
        /// Amount in token atoms
        #[arg(long)]
        amount: u64,
        /// Recipient address — ATA owner who receives the tokens (defaults to payer)
        #[arg(long)]
        recipient: Option<String>,
    },

    /// Delegate the payer's ephemeral ATA to the MagicBlock ER.
    EphemeralDelegateAta {
        /// Quote mint address
        #[arg(long)]
        mint: String,
    },

    /// Deposit into a Manifest market using ephemeral ATAs (run on ER).
    /// Transfers from the payer's ephemeral ATA to the market's vault ephemeral ATA.
    EphemeralManifestDeposit {
        /// Market PDA address
        #[arg(long)]
        market: String,
        /// Quote mint address
        #[arg(long)]
        quote_mint: String,
        /// Amount in quote atoms
        #[arg(long)]
        amount: u64,
    },

    /// Withdraw from a Manifest market using ephemeral ATAs (run on ER).
    /// Transfers from the market's vault ephemeral ATA to the payer's ephemeral ATA.
    EphemeralManifestWithdraw {
        /// Market PDA address
        #[arg(long)]
        market: String,
        /// Quote mint address
        #[arg(long)]
        quote_mint: String,
        /// Amount in quote atoms
        #[arg(long)]
        amount: u64,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print current config (file values + active defaults)
    Get,
    /// Set config values (omit a flag to leave it unchanged)
    Set {
        /// Default RPC URL (e.g. https://api.devnet.solana.com or 'er')
        #[arg(long)]
        url: Option<String>,
        /// Default keypair path
        #[arg(long)]
        keypair: Option<String>,
    },
}

// ─── helpers ─────────────────────────────────────────────────────────────────

fn load_keypair(path: Option<&str>) -> Result<Keypair> {
    let p = match path {
        Some(p) => p.to_string(),
        None => {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            format!("{}/.config/solana/id.json", home)
        }
    };
    read_keypair_file(&p).map_err(|e| anyhow!("Failed to load keypair from {p}: {e}"))
}

fn resolve_url(url: &str) -> &str {
    if url == "er" || url == "ephemeral" {
        ER_URL
    } else {
        url
    }
}

fn rpc(url: &str) -> RpcClient {
    RpcClient::new_with_commitment(resolve_url(url).to_string(), CommitmentConfig::confirmed())
}

fn send(client: &RpcClient, ixs: &[Instruction], signers: &[&Keypair]) -> Result<String> {
    let blockhash = client.get_latest_blockhash()?;
    let tx =
        Transaction::new_signed_with_payer(ixs, Some(&signers[0].pubkey()), signers, blockhash);
    let sig = client.send_and_confirm_transaction_with_spinner_and_config(
        &tx,
        CommitmentConfig::processed(),
        RpcSendTransactionConfig {
            skip_preflight: true,
            ..Default::default()
        },
    )?;
    Ok(sig.to_string())
}

fn parse_pubkey(s: &str) -> Result<Pubkey> {
    Pubkey::from_str(s).map_err(|e| anyhow!("Invalid pubkey '{s}': {e}"))
}

fn parse_order_type(s: &str) -> Result<OrderType> {
    match s.to_lowercase().as_str() {
        "limit" => Ok(OrderType::Limit),
        "ioc" | "immediate-or-cancel" | "immediateorcancel" => Ok(OrderType::ImmediateOrCancel),
        "post-only" | "postonly" => Ok(OrderType::PostOnly),
        other => Err(anyhow!(
            "Unknown order type '{other}'. Use: limit | ioc | post-only"
        )),
    }
}

/// Fetch live price from a Pyth V2 price account.
/// Returns (mantissa: u32, exponent: i8, price_usd: f64).
fn fetch_pyth_price(
    client: &RpcClient,
    feed: &Pubkey,
    quote_decimals: u8,
    base_decimals: u8,
) -> Result<(u32, i8, f64)> {
    const PYTH_MAGIC: u32 = 0xa1b2c3d4;
    const EXPO_OFF: usize = 20;
    const PRICE_OFF: usize = 208;
    const STATUS_OFF: usize = 224;
    const STATUS_TRADING: u32 = 1;

    let data = client.get_account_data(feed)?;
    if data.len() < 240 {
        return Err(anyhow!(
            "Pyth account too small ({} bytes). Is this really a Pyth V2 price account?",
            data.len()
        ));
    }
    let magic = u32::from_le_bytes(data[0..4].try_into().unwrap());
    if magic != PYTH_MAGIC {
        return Err(anyhow!(
            "Pyth magic mismatch: got {:#010x}, expected {:#010x}",
            magic,
            PYTH_MAGIC
        ));
    }
    let expo = i32::from_le_bytes(data[EXPO_OFF..EXPO_OFF + 4].try_into().unwrap());
    let price = i64::from_le_bytes(data[PRICE_OFF..PRICE_OFF + 8].try_into().unwrap());
    let status = u32::from_le_bytes(data[STATUS_OFF..STATUS_OFF + 4].try_into().unwrap());

    if status != STATUS_TRADING {
        return Err(anyhow!("Pyth price not in Trading status: {status}"));
    }
    if price <= 0 {
        return Err(anyhow!("Pyth price non-positive: {price}"));
    }

    let price_usd = price as f64 * 10f64.powi(expo);

    // Convert to order price: quote_atoms / base_atom
    let combined_expo = expo + quote_decimals as i32 - base_decimals as i32;
    let mut mantissa = price;
    let mut order_expo = combined_expo;

    while mantissa > u32::MAX as i64 {
        mantissa /= 10;
        order_expo += 1;
    }
    while mantissa > 0 && mantissa % 10 == 0 {
        mantissa /= 10;
        order_expo += 1;
    }
    if order_expo < i8::MIN as i32 || order_expo > i8::MAX as i32 {
        return Err(anyhow!("Order exponent {order_expo} out of i8 range"));
    }

    Ok((mantissa as u32, order_expo as i8, price_usd))
}

/// Parse a live price from a Pyth `PriceUpdateV3` account (used on MagicBlock ER).
///
/// Layout: disc(8) + authority(32) + verification_level(1) + PriceFeedMessage
///   - verification_level 0x01 (Full)    → message starts at byte 41
///   - verification_level 0x00 (Partial) → byte[41] = num_signatures, message at byte 42
/// PriceFeedMessage: feed_id(32) + price(8) + conf(8) + expo(4) + ...
/// The exponent is stored as a positive number of decimal places:
///   human_price = price / 10^expo
fn parse_price_v3(data: &[u8]) -> Result<f64> {
    if data.len() < 93 {
        return Err(anyhow!(
            "PriceUpdateV3 account too small ({} bytes)",
            data.len()
        ));
    }
    let msg_start: usize = match data[40] {
        0x01 => 41,
        0x00 => 42,
        b => return Err(anyhow!("Unknown VerificationLevel byte: {:#04x}", b)),
    };
    if data.len() < msg_start + 52 {
        return Err(anyhow!("PriceUpdateV3 truncated at message payload"));
    }
    let price = i64::from_le_bytes(data[msg_start + 32..msg_start + 40].try_into().unwrap());
    let expo = i32::from_le_bytes(data[msg_start + 48..msg_start + 52].try_into().unwrap());
    if price <= 0 {
        return Err(anyhow!("PriceUpdateV3 price non-positive: {price}"));
    }
    Ok(price as f64 / 10f64.powi(expo))
}

/// Fetch live SOL/USD price from the ER oracle (PriceUpdateV3 format).
/// Returns (mantissa: u32, exponent: i8, price_usd: f64).
fn fetch_er_price(
    er_client: &RpcClient,
    quote_decimals: u8,
    base_decimals: u8,
) -> Result<(u32, i8, f64)> {
    let feed = Pubkey::from_str(PYTH_SOL_USD_ER).unwrap();
    let data = er_client.get_account_data(&feed)?;
    let price_usd = parse_price_v3(&data)?;
    let (m, e) = usd_to_order_price(price_usd, quote_decimals, base_decimals);
    Ok((m, e, price_usd))
}

/// Convert a human USD/token price to PlaceOrderParams mantissa+exponent.
fn usd_to_order_price(price_usd: f64, quote_decimals: u8, base_decimals: u8) -> (u32, i8) {
    let mut m = price_usd;
    let mut e: i32 = quote_decimals as i32 - base_decimals as i32;
    while (m - m.floor()).abs() > 1e-9 && m < u32::MAX as f64 / 10.0 {
        m *= 10.0;
        e -= 1;
    }
    while m > u32::MAX as f64 {
        m /= 10.0;
        e += 1;
    }
    (m.round() as u32, e as i8)
}

fn delegate_market_ix(payer: &Pubkey, market: &Pubkey, quote_mint: &Pubkey) -> Instruction {
    let dlp = Pubkey::from_str(DELEGATION_PROGRAM_ID).unwrap();
    let e_spl = Pubkey::from_str(EPHEMERAL_SPL_TOKEN).unwrap();
    let owner = manifest::id();

    // Market delegation PDAs
    let (delegation_record, _) =
        Pubkey::find_program_address(&[b"delegation", market.as_ref()], &dlp);
    let (delegation_metadata, _) =
        Pubkey::find_program_address(&[b"delegation-metadata", market.as_ref()], &dlp);
    let (buffer, _) = Pubkey::find_program_address(&[b"buffer", market.as_ref()], &owner);

    // Ephemeral vault ATA and its delegation PDAs
    let (ephemeral_vault_ata, _) =
        Pubkey::find_program_address(&[market.as_ref(), quote_mint.as_ref()], &e_spl);
    let (vault_ata_buffer, _) =
        Pubkey::find_program_address(&[b"buffer", ephemeral_vault_ata.as_ref()], &e_spl);
    let (vault_ata_delegation_record, _) =
        Pubkey::find_program_address(&[b"delegation", ephemeral_vault_ata.as_ref()], &dlp);
    let (vault_ata_delegation_metadata, _) = Pubkey::find_program_address(
        &[b"delegation-metadata", ephemeral_vault_ata.as_ref()],
        &dlp,
    );

    Instruction {
        program_id: manifest::id(),
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(*market, false),
            AccountMeta::new_readonly(owner, false),
            AccountMeta::new_readonly(dlp, false),
            AccountMeta::new(delegation_record, false),
            AccountMeta::new(delegation_metadata, false),
            AccountMeta::new_readonly(system_program::id(), false),
            AccountMeta::new(buffer, false),
            AccountMeta::new(ephemeral_vault_ata, false),
            AccountMeta::new_readonly(e_spl, false),
            AccountMeta::new(vault_ata_buffer, false),
            AccountMeta::new(vault_ata_delegation_record, false),
            AccountMeta::new(vault_ata_delegation_metadata, false),
        ],
        data: ManifestInstruction::DelegateMarket.to_vec(),
    }
}

fn create_mint_keypair(client: &RpcClient, payer: &Keypair, decimals: u8) -> Result<Keypair> {
    use solana_program::program_pack::Pack;
    use spl_token::{instruction as token_ix, state::Mint};

    let mint_kp = Keypair::new();
    let lamports = client.get_minimum_balance_for_rent_exemption(Mint::LEN)?;
    let ixs = vec![
        system_instruction::create_account(
            &payer.pubkey(),
            &mint_kp.pubkey(),
            lamports,
            Mint::LEN as u64,
            &spl_token::id(),
        ),
        token_ix::initialize_mint(
            &spl_token::id(),
            &mint_kp.pubkey(),
            &payer.pubkey(),
            None,
            decimals,
        )?,
    ];
    send(client, &ixs, &[payer, &mint_kp])?;
    Ok(mint_kp)
}

fn create_ata_and_mint(
    client: &RpcClient,
    payer: &Keypair,
    mint: &Pubkey,
    owner: &Pubkey,
    atoms: u64,
) -> Result<Pubkey> {
    use spl_token::instruction as token_ix;

    let ata = get_associated_token_address(owner, mint);
    let mut ixs = vec![create_associated_token_account_idempotent(
        &payer.pubkey(),
        owner,
        mint,
        &spl_token::id(),
    )];
    if atoms > 0 {
        ixs.push(token_ix::mint_to(
            &spl_token::id(),
            mint,
            &ata,
            &payer.pubkey(),
            &[&payer.pubkey()],
            atoms,
        )?);
    }
    send(client, &ixs, &[payer])?;
    Ok(ata)
}

// ─── ephemeral-spl-token helpers ─────────────────────────────────────────────

fn ephemeral_spl_token_id() -> Pubkey {
    Pubkey::from_str(EPHEMERAL_SPL_TOKEN).unwrap()
}

/// Global vault data account: PDA([mint], ephemeral_spl_token)
fn get_global_vault(mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[mint.as_ref()], &ephemeral_spl_token_id())
}

/// User's (or any owner's) ephemeral ATA: PDA([owner, mint], ephemeral_spl_token)
fn get_ephemeral_ata(owner: &Pubkey, mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[owner.as_ref(), mint.as_ref()], &ephemeral_spl_token_id())
}

/// The SPL token account (ATA) that the global vault PDA owns — holds real USDC.
fn get_vault_token_account(mint: &Pubkey) -> Pubkey {
    let (global_vault, _) = get_global_vault(mint);
    get_associated_token_address(&global_vault, mint)
}

/// InitializeGlobalVault (disc=1): create the per-mint global vault data account.
/// Accounts: [vault(PDA,writable), payer(signer), mint, system_program]
/// Data: [1u8, bump]
fn ix_init_global_vault(payer: &Pubkey, mint: &Pubkey) -> Instruction {
    let e_spl = ephemeral_spl_token_id();
    let (vault, bump) = get_global_vault(mint);
    Instruction {
        program_id: e_spl,
        accounts: vec![
            AccountMeta::new(vault, false),
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
        data: vec![1u8, bump],
    }
}

/// InitializeEphemeralAta (disc=0): create an ephemeral ATA for `owner`.
/// Accounts: [ephemeral_ata(PDA,writable), payer(signer), owner, mint, system_program]
/// Data: [0u8, bump]
fn ix_init_ephemeral_ata(payer: &Pubkey, owner: &Pubkey, mint: &Pubkey) -> Instruction {
    let e_spl = ephemeral_spl_token_id();
    let (ata, bump) = get_ephemeral_ata(owner, mint);
    Instruction {
        program_id: e_spl,
        accounts: vec![
            AccountMeta::new(ata, false),
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(*owner, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
        data: vec![0u8, bump],
    }
}

/// DepositSplTokens (disc=2): move real SPL tokens from payer's ATA into ephemeral ATA.
/// Locks USDC in the global vault and credits the user's ephemeral balance.
/// Accounts: [ephemeral_ata(writable), global_vault, mint, source_token(writable),
///            vault_token(writable), authority(signer), token_program]
/// Data: [2u8, amount_le_8_bytes]
fn ix_deposit_spl_tokens(
    authority: &Pubkey,
    recipient: &Pubkey,
    mint: &Pubkey,
    source_token: &Pubkey,
    amount: u64,
) -> Instruction {
    let e_spl = ephemeral_spl_token_id();
    let (ata, _) = get_ephemeral_ata(recipient, mint);
    let (global_vault, _) = get_global_vault(mint);
    let vault_token = get_vault_token_account(mint);

    let mut data = vec![2u8];
    data.extend_from_slice(&amount.to_le_bytes());

    Instruction {
        program_id: e_spl,
        accounts: vec![
            AccountMeta::new(ata, false),
            AccountMeta::new_readonly(global_vault, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new(*source_token, false),
            AccountMeta::new(vault_token, false),
            AccountMeta::new(*authority, true),
            AccountMeta::new_readonly(spl_token::id(), false),
        ],
        data,
    }
}

/// DelegateEphemeralAta (disc=4): delegate the payer's own ephemeral ATA to the ER.
/// payer MUST equal the owner of the ATA (seeds=[payer, mint]).
/// Data: [4u8, bump]
fn ix_delegate_ephemeral_ata(payer: &Pubkey, mint: &Pubkey) -> Instruction {
    let e_spl = ephemeral_spl_token_id();
    let dlp = Pubkey::from_str(DELEGATION_PROGRAM_ID).unwrap();
    let (ata, bump) = get_ephemeral_ata(payer, mint);
    let (buffer, _) = Pubkey::find_program_address(&[b"buffer", ata.as_ref()], &e_spl);
    let (delegation_record, _) = Pubkey::find_program_address(&[b"delegation", ata.as_ref()], &dlp);
    let (delegation_metadata, _) =
        Pubkey::find_program_address(&[b"delegation-metadata", ata.as_ref()], &dlp);

    Instruction {
        program_id: e_spl,
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(ata, false),
            AccountMeta::new_readonly(e_spl, false), // owner_program
            AccountMeta::new(buffer, false),
            AccountMeta::new(delegation_record, false),
            AccountMeta::new(delegation_metadata, false),
            AccountMeta::new_readonly(dlp, false),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
        data: vec![4u8, bump],
    }
}

// ─── command handlers ────────────────────────────────────────────────────────

fn cmd_create_mint(client: &RpcClient, payer: &Keypair, decimals: u8) -> Result<()> {
    println!("Creating mint with {decimals} decimals…");
    let mint_kp = create_mint_keypair(client, payer, decimals)?;
    println!("Mint: {}", mint_kp.pubkey());
    Ok(())
}

fn cmd_mint_to(
    client: &RpcClient,
    payer: &Keypair,
    mint: &Pubkey,
    owner: &Pubkey,
    amount: u64,
) -> Result<()> {
    println!("Minting {amount} atoms of {mint} to {owner}…");
    let ata = create_ata_and_mint(client, payer, mint, owner, amount)?;
    println!("ATA: {ata}");
    Ok(())
}

fn cmd_create_market(
    client: &RpcClient,
    payer: &Keypair,
    quote_mint: &Pubkey,
    base_mint_index: u8,
    base_decimals: u8,
    initial_margin_bps: u64,
    maintenance_margin_bps: u64,
    pyth_feed: Pubkey,
    taker_fee_bps: u64,
    liquidation_buffer_bps: u64,
    num_blocks: u32,
) -> Result<()> {
    let (market, _) = get_market_address(base_mint_index, quote_mint);
    let (vault, _) = get_vault_address(&market, quote_mint);
    println!("Market PDA  : {market}");
    println!("Quote vault : {vault}");
    if num_blocks > 0 {
        println!("Pre-expand  : {num_blocks} blocks");
    }

    let ixs = create_market_instructions(
        base_mint_index,
        base_decimals,
        quote_mint,
        &payer.pubkey(),
        initial_margin_bps,
        maintenance_margin_bps,
        pyth_feed,
        taker_fee_bps,
        liquidation_buffer_bps,
        num_blocks,
    );
    let sig = send(client, &ixs, &[payer])?;
    println!("Signature   : {sig}");
    Ok(())
}

fn cmd_release_seat(client: &RpcClient, payer: &Keypair, market: &Pubkey) -> Result<()> {
    println!("Releasing seat for {} on market {market}…", payer.pubkey());
    let ix = release_seat_instruction(market, &payer.pubkey());
    let sig = send(client, &[ix], &[payer])?;
    println!("Signature: {sig}");
    Ok(())
}

fn cmd_expand(client: &RpcClient, payer: &Keypair, market: &Pubkey, blocks: u32) -> Result<()> {
    println!("Expanding market {market} by {blocks} block(s)…");
    // Solana realloc limit is 10 KB per instruction → max ~125 blocks (80 bytes each).
    // Use chunks of 100 blocks per tx to stay safe.
    const CHUNK: u32 = 100;
    let mut remaining = blocks;
    let mut allocated = 0u32;
    let mut batch = 1u32;
    while remaining > 0 {
        let n = remaining.min(CHUNK);
        // Request enough free blocks so the program allocates exactly `n` more
        allocated += n;
        let ix = expand_market_n_instruction(market, &payer.pubkey(), allocated);
        let sig = send(client, &[ix], &[payer])?;
        println!("  batch {batch}: +{n} blocks (total {allocated})  sig={sig}");
        remaining -= n;
        batch += 1;
    }
    Ok(())
}

fn cmd_claim_seat(
    client: &RpcClient,
    payer: &Keypair,
    market: &Pubkey,
    trader: &Pubkey,
) -> Result<()> {
    println!("Claiming seat for {trader} on market {market}…");
    let ix = claim_seat_instruction(market, trader);
    let sig = send(client, &[ix], &[payer])?;
    println!("Signature: {sig}");
    Ok(())
}

fn cmd_deposit(
    client: &RpcClient,
    payer: &Keypair,
    market: &Pubkey,
    quote_mint: &Pubkey,
    amount: u64,
) -> Result<()> {
    let ata = get_associated_token_address(&payer.pubkey(), quote_mint);
    println!("Depositing {amount} atoms of {quote_mint} from {ata}…");
    let ix = deposit_instruction(
        market,
        &payer.pubkey(),
        quote_mint,
        amount,
        &ata,
        spl_token::id(),
        None,
    );
    let sig = send(client, &[ix], &[payer])?;
    println!("Signature: {sig}");
    Ok(())
}

fn cmd_withdraw(
    client: &RpcClient,
    payer: &Keypair,
    market: &Pubkey,
    quote_mint: &Pubkey,
    amount: u64,
) -> Result<()> {
    let ata = get_associated_token_address(&payer.pubkey(), quote_mint);
    println!("Withdrawing {amount} atoms of {quote_mint} to {ata}…");
    let ix = withdraw_instruction(
        market,
        &payer.pubkey(),
        quote_mint,
        amount,
        &ata,
        spl_token::id(),
        None,
    );
    let sig = send(client, &[ix], &[payer])?;
    println!("Signature: {sig}");
    Ok(())
}

fn cmd_place_order(
    client: &RpcClient,
    payer: &Keypair,
    market: &Pubkey,
    base_atoms: u64,
    price_mantissa: u32,
    price_exponent: i8,
    is_bid: bool,
    order_type: OrderType,
    last_valid_slot: u32,
) -> Result<()> {
    let side = if is_bid { "BID" } else { "ASK" };
    let price = price_mantissa as f64 * 10f64.powi(price_exponent as i32);
    println!("Placing {side} {base_atoms} base atoms @ price={price:.8} ({order_type:?})…");
    let ix = batch_update_instruction(
        market,
        &payer.pubkey(),
        None,
        vec![],
        vec![PlaceOrderParams::new(
            base_atoms,
            price_mantissa,
            price_exponent,
            is_bid,
            order_type,
            last_valid_slot,
        )],
        None,
        None,
        None,
        None,
    );
    let sig = send(client, &[ix], &[payer])?;
    println!("Signature: {sig}");
    Ok(())
}

fn cmd_open_long(
    client: &RpcClient,
    payer: &Keypair,
    market: &Pubkey,
    leverage: u32,
    margin_atoms: u64,
    quote_decimals: u8,
    base_decimals: u8,
) -> Result<()> {
    let (price_mantissa, price_exponent, price_usd) =
        fetch_er_price(client, quote_decimals, base_decimals)?;

    // notional = margin * leverage  (in quote atoms)
    // base_atoms = notional / price_usd  (accounting for decimal difference)
    // price_usd is human price (e.g. 139.82). Convert margin from quote atoms to USD:
    // margin_usd = margin_atoms / 10^quote_decimals
    // notional_usd = margin_usd * leverage
    // base_atoms = notional_usd / price_usd * 10^base_decimals
    let margin_usd = margin_atoms as f64 / 10f64.powi(quote_decimals as i32);
    let notional_usd = margin_usd * leverage as f64;
    let base_atoms = (notional_usd / price_usd * 10f64.powi(base_decimals as i32)) as u64;

    println!("Oracle price    : ${price_usd:.4}");
    println!("Margin          : {margin_atoms} atoms = ${margin_usd:.4}");
    println!("Leverage        : {leverage}x");
    println!("Notional        : ${notional_usd:.4}");
    println!("Order size      : {base_atoms} base atoms");
    println!("Order price     : {price_mantissa} × 10^{price_exponent}");

    let ix = batch_update_instruction(
        market,
        &payer.pubkey(),
        None,
        vec![],
        vec![PlaceOrderParams::new(
            base_atoms,
            price_mantissa,
            price_exponent,
            true, // bid = long
            OrderType::ImmediateOrCancel,
            0,
        )],
        None,
        None,
        None,
        None,
    );
    let sig = send(client, &[ix], &[payer])?;
    println!("Signature: {sig}");
    Ok(())
}

fn cmd_cancel_order(
    client: &RpcClient,
    payer: &Keypair,
    market: &Pubkey,
    sequence_number: u64,
) -> Result<()> {
    println!("Cancelling order #{sequence_number} on market {market}…");
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
    let sig = send(client, &[ix], &[payer])?;
    println!("Signature: {sig}");
    Ok(())
}

fn cmd_delegate(client: &RpcClient, payer: &Keypair, market: &Pubkey, quote_mint: &Pubkey) -> Result<()> {
    println!("Delegating market {market} to MagicBlock ER…");
    let ix = delegate_market_ix(&payer.pubkey(), market, quote_mint);
    let sig = send(client, &[ix], &[payer])?;
    println!("Signature: {sig}");
    println!("Market is now delegated. Post-delegation operations (deposit/withdraw) must run on base chain before this step, or order-only ops on ER.");
    Ok(())
}

fn cmd_ephemeral_init_global_vault(
    client: &RpcClient,
    payer: &Keypair,
    mint: &Pubkey,
) -> Result<()> {
    let (vault, _) = get_global_vault(mint);
    println!("Initializing ephemeral global vault for mint {mint}");
    println!("  Global vault PDA : {vault}");
    // ix1: init the global vault data account
    // ix2: create the SPL ATA that will hold real tokens (vault_token account)
    let ix1 = ix_init_global_vault(&payer.pubkey(), mint);
    let ix2 =
        create_associated_token_account_idempotent(&payer.pubkey(), &vault, mint, &spl_token::id());
    let vault_token = get_vault_token_account(mint);
    println!("  Vault token ATA  : {vault_token}");
    let sig = send(client, &[ix1, ix2], &[payer])?;
    println!("  Signature: {sig}");
    Ok(())
}

fn cmd_ephemeral_init_ata(
    client: &RpcClient,
    payer: &Keypair,
    owner: &Pubkey,
    mint: &Pubkey,
) -> Result<()> {
    let (ata, _) = get_ephemeral_ata(owner, mint);
    println!("Initializing ephemeral ATA for owner {owner}, mint {mint}");
    println!("  Ephemeral ATA : {ata}");
    let ix = ix_init_ephemeral_ata(&payer.pubkey(), owner, mint);
    let sig = send(client, &[ix], &[payer])?;
    println!("  Signature: {sig}");
    Ok(())
}

fn cmd_ephemeral_deposit_spl(
    client: &RpcClient,
    payer: &Keypair,
    mint: &Pubkey,
    recipient: &Pubkey,
    amount: u64,
) -> Result<()> {
    println!("Payer is {}", payer.pubkey());
    let source_token = get_associated_token_address(recipient, mint);
    let (ata, _) = get_ephemeral_ata(recipient, mint);
    println!("Depositing {amount} atoms from regular ATA into ephemeral ATA");
    println!("  Source (SPL ATA)  : {source_token}");
    println!("  Dest (ephemeral)  : {ata}");
    let ix = ix_deposit_spl_tokens(&payer.pubkey(), recipient, mint, &source_token, amount);
    let sig = send(client, &[ix], &[payer])?;
    println!("  Signature: {sig}");
    Ok(())
}

fn cmd_ephemeral_delegate_ata(client: &RpcClient, payer: &Keypair, mint: &Pubkey) -> Result<()> {
    let (ata, _) = get_ephemeral_ata(&payer.pubkey(), mint);
    println!("Delegating ephemeral ATA {ata} to ER…");
    let ix = ix_delegate_ephemeral_ata(&payer.pubkey(), mint);
    let sig = send(client, &[ix], &[payer])?;
    println!("  Signature: {sig}");
    Ok(())
}

fn cmd_ephemeral_manifest_deposit(
    client: &RpcClient,
    payer: &Keypair,
    market: &Pubkey,
    quote_mint: &Pubkey,
    amount: u64,
) -> Result<()> {
    let (trader_ata, _) = get_ephemeral_ata(&payer.pubkey(), quote_mint);
    let (vault_ata, _) = get_ephemeral_ata(market, quote_mint);
    println!("Depositing {amount} atoms into Manifest market via ephemeral ATAs (ER)");
    println!("  Trader ephemeral ATA : {trader_ata}");
    println!("  Market vault ATA     : {vault_ata}");

    let ix = deposit_instruction_with_vault(
        market,
        &payer.pubkey(),
        quote_mint,
        amount,
        &trader_ata,
        &vault_ata,
        ephemeral_spl_token_id(),
        None,
    );
    let sig = send(client, &[ix], &[payer])?;
    println!("  Signature: {sig}");
    Ok(())
}

fn cmd_ephemeral_manifest_withdraw(
    client: &RpcClient,
    payer: &Keypair,
    market: &Pubkey,
    quote_mint: &Pubkey,
    amount: u64,
) -> Result<()> {
    let (trader_ata, _) = get_ephemeral_ata(&payer.pubkey(), quote_mint);
    let (vault_ata, _) = get_ephemeral_ata(market, quote_mint);
    println!("Withdrawing {amount} atoms from Manifest market via ephemeral ATAs (ER)");
    println!("  Market vault ATA     : {vault_ata}");
    println!("  Trader ephemeral ATA : {trader_ata}");

    let ix = withdraw_instruction_with_vault(
        market,
        &payer.pubkey(),
        quote_mint,
        amount,
        &trader_ata,
        &vault_ata,
        ephemeral_spl_token_id(),
        None,
    );
    let sig = send(client, &[ix], &[payer])?;
    println!("  Signature: {sig}");
    Ok(())
}

fn cmd_crank_funding(
    client: &RpcClient,
    payer: &Keypair,
    market: &Pubkey,
    pyth_feed: &Pubkey,
) -> Result<()> {
    println!("Cranking funding for market {market}…");
    let ix = crank_funding_instruction(market, &payer.pubkey(), pyth_feed);
    let sig = send(client, &[ix], &[payer])?;
    println!("Signature: {sig}");
    Ok(())
}

fn cmd_liquidate(
    client: &RpcClient,
    liquidator: &Keypair,
    market: &Pubkey,
    trader: &Pubkey,
) -> Result<()> {
    println!("Liquidating {trader} on market {market}…");
    let ix = liquidate_instruction(market, &liquidator.pubkey(), trader);
    let sig = send(client, &[ix], &[liquidator])?;
    println!("Signature: {sig}");
    Ok(())
}

fn cmd_fetch_price(
    client: &RpcClient,
    feed: &Pubkey,
    quote_decimals: u8,
    base_decimals: u8,
) -> Result<()> {
    // Try V2 push oracle first; fall back to V3 pull oracle (PriceUpdateV3)
    let (mantissa, exponent, price_usd) =
        fetch_pyth_price(client, feed, quote_decimals, base_decimals).or_else(|_| {
            let data = client.get_account_data(feed)?;
            let price_usd = parse_price_v3(&data)?;
            let (m, e) = usd_to_order_price(price_usd, quote_decimals, base_decimals);
            Ok::<_, anyhow::Error>((m, e, price_usd))
        })?;
    println!("Feed        : {feed}");
    println!("Price (USD) : ${price_usd:.6}");
    println!("Order price : {mantissa} × 10^{exponent}  (quote atoms / base atom)");
    Ok(())
}

fn cmd_market_info(client: &RpcClient, market: &Pubkey) -> Result<()> {
    let account = client.get_account(market)?;
    println!("Market      : {market}");
    println!("Owner       : {}", account.owner);
    println!("Lamports    : {}", account.lamports);
    println!("Data length : {} bytes", account.data.len());
    println!("Executable  : {}", account.executable);
    Ok(())
}

fn cmd_setup(
    devnet: &RpcClient,
    er: &RpcClient,
    payer: &Keypair,
    base_mint_index: u8,
    initial_margin_bps: u64,
    maintenance_margin_bps: u64,
    taker_fee_bps: u64,
) -> Result<()> {
    const BASE_MINT_DECIMALS: u8 = 9;
    const QUOTE_MINT_DECIMALS: u8 = 6;
    const LIQUIDATION_BUFFER_BPS: u64 = 200;

    // ── Step 1: USDC mint ──────────────────────────────────────────────────
    println!("── Step 1: Creating USDC test mint…");
    let usdc_mint_kp = create_mint_keypair(devnet, payer, QUOTE_MINT_DECIMALS)?;
    let usdc_mint = usdc_mint_kp.pubkey();
    println!("  USDC mint: {usdc_mint}");

    // ── Step 2: Maker keypair ──────────────────────────────────────────────
    println!("\n── Step 2: Generating maker keypair and funding…");
    let maker = Keypair::new();
    println!("  Maker: {}", maker.pubkey());
    let fund_ix = system_instruction::transfer(&payer.pubkey(), &maker.pubkey(), 500_000_000);
    send(devnet, &[fund_ix], &[payer])?;
    println!("  Funded maker with 0.5 SOL");

    // ── Step 3: Create market ──────────────────────────────────────────────
    println!("\n── Step 3: Creating perps market…");
    let pyth_feed = parse_pubkey(PYTH_SOL_USD_DEVNET)?;
    let (market_key, _) = get_market_address(base_mint_index, &usdc_mint);
    let (quote_vault, _) = get_vault_address(&market_key, &usdc_mint);
    println!("  Market PDA : {market_key}");
    println!("  Quote vault: {quote_vault}");
    let create_ixs = create_market_instructions(
        base_mint_index,
        BASE_MINT_DECIMALS,
        &usdc_mint,
        &payer.pubkey(),
        initial_margin_bps,
        maintenance_margin_bps,
        pyth_feed,
        taker_fee_bps,
        LIQUIDATION_BUFFER_BPS,
        20, // pre-expand 20 blocks during creation
    );
    let sig = send(devnet, &create_ixs, &[payer])?;
    println!("  Created with 20 pre-expanded blocks: {sig}");

    // ── Step 5: Claim seats ───────────────────────────────────────────────
    println!("\n── Step 5: Claiming seats…");
    let sig = send(
        devnet,
        &[claim_seat_instruction(&market_key, &maker.pubkey())],
        &[&maker],
    )?;
    println!("  Maker: {sig}");
    let sig = send(
        devnet,
        &[claim_seat_instruction(&market_key, &payer.pubkey())],
        &[payer],
    )?;
    println!("  Payer: {sig}");

    // ── Step 6: Fund ATAs + deposit ───────────────────────────────────────
    println!("\n── Step 6: Funding ATAs and depositing margins…");
    let maker_ata = create_ata_and_mint(
        devnet,
        payer,
        &usdc_mint,
        &maker.pubkey(),
        5_000 * 10u64.pow(QUOTE_MINT_DECIMALS as u32),
    )?;
    let dep = deposit_instruction(
        &market_key,
        &maker.pubkey(),
        &usdc_mint,
        5_000 * 10u64.pow(QUOTE_MINT_DECIMALS as u32),
        &maker_ata,
        spl_token::id(),
        None,
    );
    let sig = send(devnet, &[dep], &[&maker])?;
    println!("  Maker deposited 5 000 USDC: {sig}");

    let payer_ata = create_ata_and_mint(
        devnet,
        payer,
        &usdc_mint,
        &payer.pubkey(),
        201 * 10u64.pow(QUOTE_MINT_DECIMALS as u32),
    )?;
    let dep = deposit_instruction(
        &market_key,
        &payer.pubkey(),
        &usdc_mint,
        200 * 10u64.pow(QUOTE_MINT_DECIMALS as u32),
        &payer_ata,
        spl_token::id(),
        None,
    );
    let sig = send(devnet, &[dep], &[payer])?;
    println!("  Payer deposited 200 USDC: {sig}");

    // ── Step 7: Delegate ──────────────────────────────────────────────────
    println!("\n── Step 7: Delegating market to MagicBlock…");
    let sig = send(
        devnet,
        &[delegate_market_ix(&payer.pubkey(), &market_key, &usdc_mint)],
        &[payer],
    )?;
    println!("  Delegated: {sig}");

    // ── Fetch oracle price ────────────────────────────────────────────────
    println!("\n── Fetching live SOL/USD price from Pyth…");
    let (entry_mantissa, entry_expo, entry_price_usd) =
        fetch_pyth_price(devnet, &pyth_feed, QUOTE_MINT_DECIMALS, BASE_MINT_DECIMALS)?;
    println!("  Oracle price : ${entry_price_usd:.4} USDC/SOL");
    let exit_price_usd = entry_price_usd * 1.05;
    let (exit_mantissa, exit_expo) =
        usd_to_order_price(exit_price_usd, QUOTE_MINT_DECIMALS, BASE_MINT_DECIMALS);
    println!("  Close price  : ${exit_price_usd:.4} USDC/SOL (+5%)");

    // ── Phase 2: ER ───────────────────────────────────────────────────────
    println!("\n  → Switched to ER: {ER_URL}");
    let position_base_atoms: u64 = 1_000_000; // 0.001 SOL

    println!("\n── Step 8: Maker places resting ASK 1 000 SOL @ ${entry_price_usd:.2} (ER)…");
    let ask_ix = batch_update_instruction(
        &market_key,
        &maker.pubkey(),
        None,
        vec![],
        vec![PlaceOrderParams::new(
            1_000 * 10u64.pow(BASE_MINT_DECIMALS as u32),
            entry_mantissa,
            entry_expo,
            false,
            OrderType::Limit,
            0,
        )],
        None,
        None,
        None,
        None,
    );
    let sig = send(er, &[ask_ix], &[&maker])?;
    println!("  Ask placed: {sig}");

    println!("\n── Step 9: Payer places IOC BID → opens LONG 0.001 SOL (ER)…");
    let bid_ix = batch_update_instruction(
        &market_key,
        &payer.pubkey(),
        None,
        vec![],
        vec![PlaceOrderParams::new(
            position_base_atoms,
            entry_mantissa,
            entry_expo,
            true,
            OrderType::ImmediateOrCancel,
            0,
        )],
        None,
        None,
        None,
        None,
    );
    let sig = send(er, &[bid_ix], &[payer])?;
    println!("  Long opened: {sig}");

    println!("\n── Step 10: Payer places ASK to close LONG @ ${exit_price_usd:.4} (ER)…");
    let close_ask_ix = batch_update_instruction(
        &market_key,
        &payer.pubkey(),
        None,
        vec![],
        vec![PlaceOrderParams::new(
            position_base_atoms,
            exit_mantissa,
            exit_expo,
            false,
            OrderType::Limit,
            0,
        )],
        None,
        None,
        None,
        None,
    );
    let sig = send(er, &[close_ask_ix], &[payer])?;
    println!("  Close ASK placed: {sig}");

    println!("\n── Step 11: Maker places IOC BID → closes payer's LONG (ER)…");
    let close_bid_ix = batch_update_instruction(
        &market_key,
        &maker.pubkey(),
        None,
        vec![],
        vec![PlaceOrderParams::new(
            position_base_atoms,
            exit_mantissa,
            exit_expo,
            true,
            OrderType::ImmediateOrCancel,
            0,
        )],
        None,
        None,
        None,
        None,
    );
    let sig = send(er, &[close_bid_ix], &[&maker])?;
    println!("  Close BID matched: {sig}");

    // ── PnL ───────────────────────────────────────────────────────────────
    let position_sol = position_base_atoms as f64 / 1e9;
    let entry_cost = entry_price_usd * position_sol;
    let exit_proceeds = exit_price_usd * position_sol;
    let raw_pnl = exit_proceeds - entry_cost;
    let fee_rate = taker_fee_bps as f64 / 10_000.0;
    let fees = (entry_cost + exit_proceeds) * fee_rate;
    let net_pnl = raw_pnl - fees;

    println!("\n── PnL Summary ──────────────────────────────────────────────────");
    println!("  Position   : {position_sol:.6} SOL");
    println!("  Entry      : ${entry_price_usd:.4}  →  cost  {entry_cost:.6} USDC");
    println!("  Exit       : ${exit_price_usd:.4}  →  proceeds {exit_proceeds:.6} USDC");
    println!("  Raw PnL    : {:+.6} USDC", raw_pnl);
    println!("  Fees       : -{fees:.6} USDC");
    println!("  ──────────────────────────────────────────────");
    println!(
        "  Net PnL    : {:+.6} USDC  ({:+.2}%)",
        net_pnl,
        net_pnl / entry_cost * 100.0
    );

    println!("\n═══════════════════════════════════════════════");
    println!("  Program : {}", manifest::id());
    println!("  USDC    : {usdc_mint}");
    println!("  Market  : {market_key}");
    println!("  Vault   : {quote_vault}");
    println!("  Explorer: https://explorer.solana.com/address/{market_key}?cluster=devnet");
    println!("═══════════════════════════════════════════════");
    Ok(())
}

// ─── main ────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = CliConfig::load();

    // Handle config commands before touching the network or keypair
    if let Commands::Config { action } = cli.command {
        match action {
            ConfigAction::Get => {
                let path = config_path();
                println!("Config file : {}", path.display());
                println!(
                    "url         : {}",
                    cfg.url.as_deref().unwrap_or("<default>")
                );
                println!(
                    "keypair     : {}",
                    cfg.keypair.as_deref().unwrap_or("<default>")
                );
                println!("\nActive values (flag > config > default):");
                println!("  url     = {}", cfg.resolve_url(cli.url.as_deref()));
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                let default_kp = format!("{home}/.config/solana/id.json");
                let kp = cfg
                    .resolve_keypair(cli.keypair.as_deref())
                    .unwrap_or(&default_kp);
                println!("  keypair = {kp}");
            }
            ConfigAction::Set { url, keypair } => {
                let mut updated = CliConfig {
                    url: cfg.url,
                    keypair: cfg.keypair,
                };
                if let Some(u) = url {
                    updated.url = Some(u);
                }
                if let Some(k) = keypair {
                    updated.keypair = Some(k);
                }
                updated.save()?;
                println!("Saved to {}", config_path().display());
                if let Some(u) = &updated.url {
                    println!("  url     = {u}");
                }
                if let Some(k) = &updated.keypair {
                    println!("  keypair = {k}");
                }
            }
        }
        return Ok(());
    }

    let url = cfg.resolve_url(cli.url.as_deref());
    let payer = load_keypair(cfg.resolve_keypair(cli.keypair.as_deref()))?;
    let client = rpc(url);

    // ER client (needed for Setup; individual commands use --url er)
    let er = RpcClient::new_with_commitment(ER_URL.to_string(), CommitmentConfig::confirmed());

    match cli.command {
        Commands::CreateMint { decimals } => {
            cmd_create_mint(&client, &payer, decimals)?;
        }

        Commands::MintTo {
            mint,
            amount,
            recipient,
        } => {
            let mint = parse_pubkey(&mint)?;
            let owner = recipient
                .as_deref()
                .map(parse_pubkey)
                .transpose()?
                .unwrap_or(payer.pubkey());
            cmd_mint_to(&client, &payer, &mint, &owner, amount)?;
        }

        Commands::CreateMarket {
            quote_mint,
            base_mint_index,
            base_decimals,
            initial_margin_bps,
            maintenance_margin_bps,
            pyth_feed,
            taker_fee_bps,
            liquidation_buffer_bps,
            num_blocks,
        } => {
            let quote_mint = parse_pubkey(&quote_mint)?;
            let pyth = pyth_feed
                .as_deref()
                .map(parse_pubkey)
                .transpose()?
                .unwrap_or_else(|| parse_pubkey(PYTH_SOL_USD_DEVNET).unwrap());
            cmd_create_market(
                &client,
                &payer,
                &quote_mint,
                base_mint_index,
                base_decimals,
                initial_margin_bps,
                maintenance_margin_bps,
                pyth,
                taker_fee_bps,
                liquidation_buffer_bps,
                num_blocks,
            )?;
        }

        Commands::Expand { market, blocks } => {
            let market = parse_pubkey(&market)?;
            cmd_expand(&client, &payer, &market, blocks)?;
        }

        Commands::ClaimSeat { market, trader } => {
            let market = parse_pubkey(&market)?;
            let trader = trader
                .as_deref()
                .map(parse_pubkey)
                .transpose()?
                .unwrap_or(payer.pubkey());
            cmd_claim_seat(&client, &payer, &market, &trader)?;
        }

        Commands::ReleaseSeat { market } => {
            let market = parse_pubkey(&market)?;
            cmd_release_seat(&client, &payer, &market)?;
        }

        Commands::Deposit {
            market,
            quote_mint,
            amount,
        } => {
            let market = parse_pubkey(&market)?;
            let quote_mint = parse_pubkey(&quote_mint)?;
            cmd_deposit(&client, &payer, &market, &quote_mint, amount)?;
        }

        Commands::Withdraw {
            market,
            quote_mint,
            amount,
        } => {
            let market = parse_pubkey(&market)?;
            let quote_mint = parse_pubkey(&quote_mint)?;
            cmd_withdraw(&client, &payer, &market, &quote_mint, amount)?;
        }

        Commands::PlaceOrder {
            market,
            base_atoms,
            price_mantissa,
            price_exponent,
            is_bid,
            order_type,
            last_valid_slot,
        } => {
            let market = parse_pubkey(&market)?;
            let ot = parse_order_type(&order_type)?;
            cmd_place_order(
                &client,
                &payer,
                &market,
                base_atoms,
                price_mantissa,
                price_exponent,
                is_bid,
                ot,
                last_valid_slot,
            )?;
        }

        Commands::CancelOrder {
            market,
            sequence_number,
        } => {
            let market = parse_pubkey(&market)?;
            cmd_cancel_order(&client, &payer, &market, sequence_number)?;
        }

        Commands::Delegate { market, quote_mint } => {
            let market = parse_pubkey(&market)?;
            let quote_mint = parse_pubkey(&quote_mint)?;
            cmd_delegate(&client, &payer, &market, &quote_mint)?;
        }

        Commands::CrankFunding { market, pyth_feed } => {
            let market = parse_pubkey(&market)?;
            let feed = pyth_feed
                .as_deref()
                .map(parse_pubkey)
                .transpose()?
                .unwrap_or_else(|| parse_pubkey(PYTH_SOL_USD_DEVNET).unwrap());
            cmd_crank_funding(&client, &payer, &market, &feed)?;
        }

        Commands::Liquidate { market, trader } => {
            let market = parse_pubkey(&market)?;
            let trader = parse_pubkey(&trader)?;
            cmd_liquidate(&client, &payer, &market, &trader)?;
        }

        Commands::FetchPrice {
            feed,
            quote_decimals,
            base_decimals,
        } => {
            let feed = feed
                .as_deref()
                .map(parse_pubkey)
                .transpose()?
                .unwrap_or_else(|| parse_pubkey(PYTH_SOL_USD_DEVNET).unwrap());
            cmd_fetch_price(&client, &feed, quote_decimals, base_decimals)?;
        }

        Commands::OpenLong {
            market,
            leverage,
            margin_atoms,
            quote_decimals,
            base_decimals,
        } => {
            let market = parse_pubkey(&market)?;
            cmd_open_long(
                &client,
                &payer,
                &market,
                leverage,
                margin_atoms,
                quote_decimals,
                base_decimals,
            )?;
        }

        Commands::MarketInfo { market } => {
            let market = parse_pubkey(&market)?;
            cmd_market_info(&client, &market)?;
        }

        Commands::Setup {
            base_mint_index,
            initial_margin_bps,
            maintenance_margin_bps,
            taker_fee_bps,
        } => {
            cmd_setup(
                &client,
                &er,
                &payer,
                base_mint_index,
                initial_margin_bps,
                maintenance_margin_bps,
                taker_fee_bps,
            )?;
        }

        Commands::EphemeralInitGlobalVault { mint } => {
            let mint = parse_pubkey(&mint)?;
            cmd_ephemeral_init_global_vault(&client, &payer, &mint)?;
        }

        Commands::EphemeralInitAta { mint, owner } => {
            let mint = parse_pubkey(&mint)?;
            let owner = owner
                .as_deref()
                .map(parse_pubkey)
                .transpose()?
                .unwrap_or(payer.pubkey());
            cmd_ephemeral_init_ata(&client, &payer, &owner, &mint)?;
        }

        Commands::EphemeralDepositSpl {
            mint,
            amount,
            recipient,
        } => {
            let mint = parse_pubkey(&mint)?;
            let recipient = recipient
                .as_deref()
                .map(parse_pubkey)
                .transpose()?
                .unwrap_or(payer.pubkey());
            cmd_ephemeral_deposit_spl(&client, &payer, &mint, &recipient, amount)?;
        }

        Commands::EphemeralDelegateAta { mint } => {
            let mint = parse_pubkey(&mint)?;
            cmd_ephemeral_delegate_ata(&client, &payer, &mint)?;
        }

        Commands::EphemeralManifestDeposit {
            market,
            quote_mint,
            amount,
        } => {
            let market = parse_pubkey(&market)?;
            let quote_mint = parse_pubkey(&quote_mint)?;
            cmd_ephemeral_manifest_deposit(&client, &payer, &market, &quote_mint, amount)?;
        }

        Commands::EphemeralManifestWithdraw {
            market,
            quote_mint,
            amount,
        } => {
            let market = parse_pubkey(&market)?;
            let quote_mint = parse_pubkey(&quote_mint)?;
            cmd_ephemeral_manifest_withdraw(&client, &payer, &market, &quote_mint, amount)?;
        }

        // Handled above before the network/keypair setup; unreachable here.
        Commands::Config { .. } => unreachable!(),
    }

    Ok(())
}
