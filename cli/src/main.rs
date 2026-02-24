//! manifest-cli — command-line interface for the Manifest Perps DEX.
//!
//! Usage:
//!   manifest-cli [--url <RPC>] [--keypair <PATH>] <COMMAND> [OPTIONS]
//!
//! Commands: create-mint  mint-to  create-market  expand  claim-seat
//!           deposit  withdraw  place-order  cancel-order  delegate
//!           crank-funding  liquidate  fetch-price  market-info  setup
//!           create-escrow  delegate-escrow  fund-escrow
use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use manifest::deps::hypertree::HyperTreeValueIteratorTrait;
use manifest::{
    program::{
        batch_update::{CancelOrderParams, PlaceOrderParams},
        batch_update_instruction,
        claim_seat_instruction::claim_seat_instruction,
        crank_funding_instruction, create_market_instructions,
        deposit_instruction, deposit_instruction_with_vault, expand_market_n_instruction,
        liquidate_instruction, release_seat_instruction, withdraw_instruction, withdraw_instruction_with_vault,
        swap_instruction::swap_instruction_with_vaults,
        ManifestInstruction,
    },
    quantities::{BaseAtoms, WrapperU64},
    state::{market::MarketFixed, OrderType, RestingOrder, MARKET_FIXED_SIZE},
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
const PYTH_SOL_USD_DEVNET: &str = "ENYwebBThHzmzwPLAQvCucUTsjyfBSZdD9ViXksS4jPu";
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

    /// Expand a market's free block capacity (uses lamport escrow in ER)
    Expand {
        /// Market PDA address
        #[arg(long)]
        market: String,
        /// Number of blocks to add
        #[arg(long, default_value = "10")]
        blocks: u32,
        /// ER validator pubkey for escrow PDA derivation
        #[arg(long)]
        escrow_validator: String,
        /// Escrow slot for PDA derivation
        #[arg(long, default_value = "0")]
        escrow_slot: u64,
    },

    /// Create a lamport escrow PDA in ephemeral-rollups-spl and fund it
    CreateEscrow {
        /// ER validator pubkey
        #[arg(long)]
        validator: String,
        /// Escrow slot
        #[arg(long, default_value = "0")]
        slot: u64,
        /// Lamports to fund the escrow with
        #[arg(long)]
        lamports: u64,
    },

    /// Delegate a lamport escrow PDA to the ER
    DelegateEscrow {
        /// ER validator pubkey
        #[arg(long)]
        validator: String,
        /// Escrow slot
        #[arg(long, default_value = "0")]
        slot: u64,
    },

    /// Fund an existing lamport escrow PDA with more SOL
    FundEscrow {
        /// ER validator pubkey
        #[arg(long)]
        validator: String,
        /// Escrow slot
        #[arg(long, default_value = "0")]
        slot: u64,
        /// Lamports to transfer
        #[arg(long)]
        lamports: u64,
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

    /// Swap via the Swap instruction (IOC taker fill with token transfer).
    /// Uses ephemeral ATAs on ER. Fetches oracle price automatically.
    Swap {
        /// Market PDA address
        #[arg(long)]
        market: String,
        /// Quote mint address
        #[arg(long)]
        quote_mint: String,
        /// Amount of quote atoms to spend (for long) or base atoms to sell (for short)
        #[arg(long)]
        in_atoms: u64,
        /// Minimum output atoms (0 = accept any)
        #[arg(long, default_value = "0")]
        min_out_atoms: u64,
        /// Direction: true = short (sell base), false = long (buy base)
        #[arg(long, default_value = "false")]
        is_base_in: bool,
    },

    /// Show basic info about a market account
    MarketInfo {
        /// Market PDA address
        #[arg(long)]
        market: String,
    },

    /// List all resting orders on the order book (bids and asks)
    Orderbook {
        /// Market PDA address
        #[arg(long)]
        market: String,
    },

    /// Display the current user's position, margin, equity, leverage, liquidation price, and more
    Position {
        /// Market PDA address
        #[arg(long)]
        market: String,
        /// Trader address (defaults to payer)
        #[arg(long)]
        trader: Option<String>,
    },

    /// Onboard a user to an existing market: mint → ephemeral-init-ata →
    /// ephemeral-deposit-spl → ephemeral-delegate-ata → claim-seat → ephemeral-manifest-deposit
    Setup {
        /// Market PDA address
        #[arg(long)]
        market: String,
        /// Quote mint address (e.g. USDC)
        #[arg(long)]
        quote_mint: String,
        /// Amount of quote tokens to deposit (in atoms)
        #[arg(long)]
        amount: u64,
        /// Mint authority keypair path (for minting tokens to the user)
        #[arg(long, default_value = "~/.config/solana/mint-authority.json")]
        mint_authority: String,
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

    /// Delegate the payer'e ephemeral ATA to the MagicBlock ER.
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

const EPHEMERAL_SPL_TOKEN_ID: &str = "SPLxh1LVZzEkX99H6rqYizhytLWPZVV296zyYDPagv2";

fn ephemeral_spl_token_id() -> Pubkey {
    Pubkey::from_str(EPHEMERAL_SPL_TOKEN_ID).unwrap()
}

fn get_ephemeral_ata(owner: &Pubkey, mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[owner.as_ref(), mint.as_ref()],
        &ephemeral_spl_token_id(),
    )
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
    let ephemeral_vault_ata = get_associated_token_address(market, quote_mint);
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

/// Global vault data account: PDA([mint], ephemeral_spl_token)
fn get_global_vault(mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[mint.as_ref()], &ephemeral_spl_token_id())
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

fn cmd_expand(
    client: &RpcClient,
    payer: &Keypair,
    market: &Pubkey,
    blocks: u32,
    validator: &Pubkey,
    escrow_slot: u64,
) -> Result<()> {
    let er_spl_program = Pubkey::from_str(EPHEMERAL_ROLLUPS_SPL_ID)?;
    let (escrow_pda, _) =
        manifest::program::expand_market_instruction::get_escrow_address(
            &payer.pubkey(),
            validator,
            escrow_slot,
        );
    println!("Expanding market {market} by {blocks} block(s)…");
    println!("Escrow PDA  : {escrow_pda}");
    // Solana realloc limit is 10 KB per instruction → max ~125 blocks (80 bytes each).
    // Use chunks of 100 blocks per tx to stay safe.
    const CHUNK: u32 = 100;
    let mut remaining = blocks;
    let mut allocated = 0u32;
    let mut batch = 1u32;
    while remaining > 0 {
        let n = remaining.min(CHUNK);
        allocated += n;
        let ix = expand_market_n_instruction(
            market,
            &payer.pubkey(),
            &escrow_pda,
            &er_spl_program,
            allocated,
            validator,
            escrow_slot,
        );
        let sig = send(client, &[ix], &[payer])?;
        println!("  batch {batch}: +{n} blocks (total {allocated})  sig={sig}");
        remaining -= n;
        batch += 1;
    }
    Ok(())
}

// ─── ephemeral-rollups-spl lamport escrow helpers ───────────────────────────

const EPHEMERAL_ROLLUPS_SPL_ID: &str = "DL2q6XaUpXsPsYrDpbieiXG6UisaUpzMSZCTkSvzn2Am";

fn cmd_create_escrow(
    client: &RpcClient,
    payer: &Keypair,
    validator: &Pubkey,
    slot: u64,
    lamports: u64,
) -> Result<()> {
    let er_spl = parse_pubkey(EPHEMERAL_ROLLUPS_SPL_ID)?;
    let (escrow_pda, _) = manifest::program::expand_market_instruction::get_escrow_address(
        &payer.pubkey(),
        validator,
        slot,
    );
    println!("Creating lamport escrow…");
    println!("  Authority : {}", payer.pubkey());
    println!("  Validator : {validator}");
    println!("  Slot      : {slot}");
    println!("  Escrow PDA: {escrow_pda}");
    println!("  Lamports  : {lamports}");

    // Build create instruction
    // Discriminant: [0x1A, 0x92, 0xB7, 0x8B, 0x57, 0xAD, 0x99, 0x02]
    // Args (Borsh): authority: Pubkey, validator: Pubkey, slot: u64
    let mut create_data = Vec::with_capacity(8 + 32 + 32 + 8);
    create_data.extend_from_slice(&[0x1A, 0x92, 0xB7, 0x8B, 0x57, 0xAD, 0x99, 0x02]);
    create_data.extend_from_slice(&payer.pubkey().to_bytes()); // authority
    create_data.extend_from_slice(&validator.to_bytes());
    create_data.extend_from_slice(&slot.to_le_bytes());

    let create_ix = Instruction::new_with_bytes(
        er_spl,
        &create_data,
        vec![
            AccountMeta::new(payer.pubkey(), true),  // authority (signer, writable)
            AccountMeta::new(escrow_pda, false),     // escrow PDA
            AccountMeta::new_readonly(system_program::id(), false),
        ],
    );

    // Fund escrow with SOL transfer
    let fund_ix = system_instruction::transfer(&payer.pubkey(), &escrow_pda, lamports);

    let sig = send(client, &[create_ix, fund_ix], &[payer])?;
    println!("  Signature : {sig}");
    Ok(())
}

fn cmd_delegate_escrow(
    client: &RpcClient,
    payer: &Keypair,
    validator: &Pubkey,
    slot: u64,
) -> Result<()> {
    let er_spl = parse_pubkey(EPHEMERAL_ROLLUPS_SPL_ID)?;
    let delegation_program = parse_pubkey(DELEGATION_PROGRAM_ID)?;
    let (escrow_pda, _) = manifest::program::expand_market_instruction::get_escrow_address(
        &payer.pubkey(),
        validator,
        slot,
    );
    println!("Delegating lamport escrow {escrow_pda} to ER…");

    // Build delegate instruction
    // Discriminant: [0x98, 0xE4, 0x41, 0xD1, 0x81, 0xB6, 0xC9, 0x3B]
    // Args (Borsh): validator: Pubkey, slot: u64
    let mut delegate_data = Vec::with_capacity(8 + 32 + 8);
    delegate_data.extend_from_slice(&[0x98, 0xE4, 0x41, 0xD1, 0x81, 0xB6, 0xC9, 0x3B]);
    delegate_data.extend_from_slice(&validator.to_bytes());
    delegate_data.extend_from_slice(&slot.to_le_bytes());

    // Accounts per source: [payer, authority, escrow_pda, buffer, record, metadata,
    //   delegation_program, owner_program(er_spl), system_program]
    // buffer PDA: seeds=[b"buffer", escrow_pda] derived from owner_program (er_spl)
    // record PDA: seeds=[b"delegation", escrow_pda] derived from delegation_program
    // metadata PDA: seeds=[b"delegation-metadata", escrow_pda] derived from delegation_program
    let delegation_buffer_pda = Pubkey::find_program_address(
        &[b"buffer", escrow_pda.as_ref()],
        &er_spl,
    ).0;
    let delegation_record_pda = Pubkey::find_program_address(
        &[b"delegation", escrow_pda.as_ref()],
        &delegation_program,
    ).0;
    let delegation_metadata_pda = Pubkey::find_program_address(
        &[b"delegation-metadata", escrow_pda.as_ref()],
        &delegation_program,
    ).0;

    let delegate_ix = Instruction::new_with_bytes(
        er_spl,
        &delegate_data,
        vec![
            AccountMeta::new(payer.pubkey(), true),                   // payer (writable, signer)
            AccountMeta::new_readonly(payer.pubkey(), true),          // authority (signer)
            AccountMeta::new(escrow_pda, false),                      // escrow PDA
            AccountMeta::new(delegation_buffer_pda, false),           // buffer PDA
            AccountMeta::new(delegation_record_pda, false),           // delegation record
            AccountMeta::new(delegation_metadata_pda, false),         // delegation metadata
            AccountMeta::new_readonly(delegation_program, false),     // delegation program
            AccountMeta::new_readonly(er_spl, false),                 // owner program
            AccountMeta::new_readonly(system_program::id(), false),   // system program
        ],
    );

    let sig = send(client, &[delegate_ix], &[payer])?;
    println!("  Signature : {sig}");
    Ok(())
}

fn cmd_fund_escrow(
    client: &RpcClient,
    payer: &Keypair,
    validator: &Pubkey,
    slot: u64,
    lamports: u64,
) -> Result<()> {
    let (escrow_pda, _) = manifest::program::expand_market_instruction::get_escrow_address(
        &payer.pubkey(),
        validator,
        slot,
    );
    println!("Funding escrow {escrow_pda} with {lamports} lamports…");
    let ix = system_instruction::transfer(&payer.pubkey(), &escrow_pda, lamports);
    let sig = send(client, &[ix], &[payer])?;
    println!("  Signature : {sig}");
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

fn cmd_swap(
    client: &RpcClient,
    payer: &Keypair,
    market: &Pubkey,
    quote_mint: &Pubkey,
    in_atoms: u64,
    min_out_atoms: u64,
    is_base_in: bool,
) -> Result<()> {
    let direction = if is_base_in { "SHORT (sell base)" } else { "LONG (buy base)" };
    println!("Swap {direction} on market {market}");
    println!("  in_atoms     : {in_atoms}");
    println!("  min_out_atoms: {min_out_atoms}");

    let trader_ata = get_associated_token_address(&payer.pubkey(), quote_mint);
    let vault_ata = get_associated_token_address(market, quote_mint);
    println!("  Trader ATA   : {trader_ata}");
    println!("  Vault ATA    : {vault_ata}");

    let ix = swap_instruction_with_vaults(
        market,
        &payer.pubkey(),
        &Pubkey::default(),  // base_mint (virtual, unused)
        quote_mint,
        &Pubkey::default(),  // trader_base (virtual, unused)
        &trader_ata,
        &Pubkey::default(),  // vault_base (virtual, unused)
        &vault_ata,
        in_atoms,
        min_out_atoms,
        is_base_in,
        true, // is_exact_in
        Pubkey::default(),   // token_program_base (unused)
        spl_token::id(),
        false,
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

fn cmd_orderbook(client: &RpcClient, market_key: &Pubkey) -> Result<()> {
    let account = client.get_account(market_key)?;
    let data = &account.data;
    if data.len() < MARKET_FIXED_SIZE {
        return Err(anyhow!("Account data too small for MarketFixed"));
    }
    let fixed: &MarketFixed = bytemuck::from_bytes(&data[..MARKET_FIXED_SIZE]);
    let dynamic = &data[MARKET_FIXED_SIZE..];
    let market = manifest::state::MarketValue {
        fixed: *fixed,
        dynamic: dynamic.to_vec(),
    };

    let base_decimals = fixed.get_base_mint_decimals() as u32;
    let quote_decimals = fixed.get_quote_mint_decimals() as u32;
    let base_factor = 10f64.powi(base_decimals as i32);

    // Helper: price per 1 base unit in USD
    let price_usd = |order: &RestingOrder| -> f64 {
        let one_base_unit = BaseAtoms::new(10u64.pow(base_decimals));
        match order.get_price().checked_quote_for_base(one_base_unit, false) {
            Ok(quote) => quote.as_u64() as f64 / 10f64.powi(quote_decimals as i32),
            Err(_) => 0.0,
        }
    };

    // Collect bids: (base_atoms, price_usd, seq, trader_index)
    let mut bids: Vec<(u64, f64, u64, u32)> = Vec::new();
    for (_, order) in market.get_bids().iter::<RestingOrder>() {
        bids.push((
            order.get_num_base_atoms().as_u64(),
            price_usd(order),
            order.get_sequence_number(),
            order.get_trader_index(),
        ));
    }

    // Collect asks
    let mut asks: Vec<(u64, f64, u64, u32)> = Vec::new();
    for (_, order) in market.get_asks().iter::<RestingOrder>() {
        asks.push((
            order.get_num_base_atoms().as_u64(),
            price_usd(order),
            order.get_sequence_number(),
            order.get_trader_index(),
        ));
    }

    println!("═══════════════════════════════════════════════════════════");
    println!("  Order Book for {market_key}");
    println!("═══════════════════════════════════════════════════════════");
    println!();

    // Resolve trader_index → pubkey
    let resolve_trader = |ti: u32| -> String {
        use manifest::deps::hypertree::{get_helper, RBNode};
        use manifest::state::claimed_seat::ClaimedSeat;
        let node = get_helper::<RBNode<ClaimedSeat>>(&dynamic, ti);
        let seat = node.get_value();
        seat.trader.to_string()
    };

    println!("── ASKS ({} orders) ────────────────────────────────", asks.len());
    if asks.is_empty() {
        println!("  (none)");
    } else {
        println!("  {:>4}  {:>12}  {:>14}  {:>12}  {:>10}  {}", "#", "Price (USD)", "Size (atoms)", "Size (base)", "Seq#", "Owner");
        for (i, (base_atoms, pusd, seq, ti)) in asks.iter().enumerate() {
            let size_base = *base_atoms as f64 / base_factor;
            let owner = resolve_trader(*ti);
            println!(
                "  {:>4}  ${:>11.4}  {:>14}  {:>12.6}  {:>10}  {}",
                i + 1, pusd, base_atoms, size_base, seq, &owner[..8],
            );
        }
    }

    println!();
    println!("── BIDS ({} orders) ────────────────────────────────", bids.len());
    if bids.is_empty() {
        println!("  (none)");
    } else {
        println!("  {:>4}  {:>12}  {:>14}  {:>12}  {:>10}  {}", "#", "Price (USD)", "Size (atoms)", "Size (base)", "Seq#", "Owner");
        for (i, (base_atoms, pusd, seq, ti)) in bids.iter().enumerate() {
            let size_base = *base_atoms as f64 / base_factor;
            let owner = resolve_trader(*ti);
            println!(
                "  {:>4}  ${:>11.4}  {:>14}  {:>12.6}  {:>10}  {}",
                i + 1, pusd, base_atoms, size_base, seq, &owner[..8],
            );
        }
    }

    // Summarize owners
    let mut owner_counts: std::collections::HashMap<String, (usize, usize)> = std::collections::HashMap::new();
    for (_, _, _, ti) in &bids {
        let owner = resolve_trader(*ti);
        let e = owner_counts.entry(owner).or_insert((0, 0));
        e.0 += 1;
    }
    for (_, _, _, ti) in &asks {
        let owner = resolve_trader(*ti);
        let e = owner_counts.entry(owner).or_insert((0, 0));
        e.1 += 1;
    }
    println!();
    println!("── Owners ─────────────────────────────────────────");
    for (owner, (bid_count, ask_count)) in &owner_counts {
        println!("  {} : {} bids, {} asks", owner, bid_count, ask_count);
    }

    println!();
    println!("Total: {} bids, {} asks", bids.len(), asks.len());

    // List all claimed seats with positions
    {
        use manifest::deps::hypertree::{RBNode, get_helper};
        use manifest::state::claimed_seat::ClaimedSeat;

        println!();
        println!("── All Seats ──────────────────────────────────────");
        println!("  {:>44}  {:>10}  {:>14}  {:>12}", "Trader", "Direction", "Position", "Margin");

        let root = fixed.get_claimed_seats_root_index();
        if root != manifest::deps::hypertree::NIL {
            // Iterate the claimed seats tree
            let seats_tree: manifest::state::market::ClaimedSeatTreeReadOnly =
                manifest::state::market::ClaimedSeatTreeReadOnly::new(dynamic, root, manifest::deps::hypertree::NIL);
            let mut net_position: i64 = 0;
            for (_, seat) in seats_tree.iter::<ClaimedSeat>() {
                let pos = seat.get_position_size();
                let margin = seat.quote_withdrawable_balance.as_u64();
                let dir = if pos > 0 { "LONG" } else if pos < 0 { "SHORT" } else { "FLAT" };
                let pos_base = pos.unsigned_abs() as f64 / base_factor;
                let margin_usd = margin as f64 / 10f64.powi(quote_decimals as i32);
                println!(
                    "  {}  {:>10}  {:>10.6} base  ${:>10.4}",
                    seat.trader, dir, pos_base, margin_usd,
                );
                net_position += pos;
            }
            let net_base = net_position.unsigned_abs() as f64 / base_factor;
            let net_dir = if net_position > 0 { "LONG" } else if net_position < 0 { "SHORT" } else { "ZERO" };
            println!("  ────────────────────────────────────────────────────────────────────────────────");
            println!("  Net position: {} {:.6} base ({} atoms)", net_dir, net_base, net_position);
        }
    }

    Ok(())
}

fn cmd_position(client: &RpcClient, market_key: &Pubkey, trader: &Pubkey) -> Result<()> {
    let account = client.get_account(market_key)?;
    let data = &account.data;
    if data.len() < MARKET_FIXED_SIZE {
        return Err(anyhow!("Account data too small for MarketFixed"));
    }

    let fixed: &MarketFixed = bytemuck::from_bytes(&data[..MARKET_FIXED_SIZE]);
    let dynamic = &data[MARKET_FIXED_SIZE..];
    let market = manifest::state::MarketValue {
        fixed: *fixed,
        dynamic: dynamic.to_vec(),
    };

    // ── Market parameters ───────────────────────────────────────────────
    let oracle_mantissa = fixed.get_oracle_price_mantissa();
    let oracle_expo = fixed.get_oracle_price_expo();
    let oracle_price = oracle_mantissa as f64 * 10f64.powi(oracle_expo);
    let initial_margin_bps = fixed.get_initial_margin_bps();
    let maintenance_margin_bps = fixed.get_maintenance_margin_bps();
    let max_leverage = 10_000.0 / initial_margin_bps as f64;
    let maint_leverage = 10_000.0 / maintenance_margin_bps as f64;
    let taker_fee_bps = fixed.get_taker_fee_bps();
    let insurance_fund = fixed.get_insurance_fund_balance();
    let liq_buffer_bps = fixed.get_liquidation_buffer_bps();
    let cumulative_funding = fixed.get_cumulative_funding();

    let base_decimals = fixed.get_base_mint_decimals() as u32;
    let quote_decimals = fixed.get_quote_mint_decimals() as u32;
    let base_factor = 10f64.powi(base_decimals as i32);
    let quote_factor = 10f64.powi(quote_decimals as i32);

    // ── Trader position ─────────────────────────────────────────────────
    let (position_size, cost_basis) = market.get_trader_position(trader);
    let (_, quote_balance) = market.get_trader_balance(trader);
    let margin_atoms = quote_balance.as_u64();

    let pos_base = position_size as f64 / base_factor;
    let is_long = position_size > 0;
    let is_short = position_size < 0;
    let direction = if is_long {
        "LONG"
    } else if is_short {
        "SHORT"
    } else {
        "FLAT"
    };

    let abs_pos = position_size.unsigned_abs() as f64 / base_factor;
    let notional = abs_pos * oracle_price;
    let margin = margin_atoms as f64 / quote_factor;
    let cost_usd = cost_basis as f64 / quote_factor;
    let entry_price = if position_size != 0 {
        cost_usd / abs_pos
    } else {
        0.0
    };

    // PnL: LONG = value - cost, SHORT = cost - value
    let current_value = abs_pos * oracle_price;
    let unrealized_pnl = if is_long {
        current_value - cost_usd
    } else if is_short {
        cost_usd - current_value
    } else {
        0.0
    };

    let equity = margin + unrealized_pnl;
    let leverage = if equity > 0.0 && position_size != 0 {
        notional / equity
    } else {
        0.0
    };

    // ── Liquidation price ───────────────────────────────────────────────
    // Liquidation when: equity <= notional * maintenance_margin_bps / 10000
    // equity = margin + pnl
    // LONG:  margin + (pos * liq_price - cost) = pos * liq_price * maint_bps / 10000
    //   margin - cost = pos * liq_price * (maint_bps/10000 - 1)
    //   liq_price = (margin - cost) / (pos * (maint_bps/10000 - 1))
    //   But since maint_bps < 10000, the denominator is negative. Rearranging:
    //   liq_price = (cost - margin) / (pos * (1 - maint_bps/10000))
    // SHORT: margin + (cost - pos * liq_price) = pos * liq_price * maint_bps / 10000
    //   margin + cost = pos * liq_price * (1 + maint_bps/10000)
    //   liq_price = (margin + cost) / (pos * (1 + maint_bps/10000))
    let maint_ratio = maintenance_margin_bps as f64 / 10_000.0;
    let liq_price = if is_long {
        (cost_usd - margin) / (abs_pos * (1.0 - maint_ratio))
    } else if is_short {
        (margin + cost_usd) / (abs_pos * (1.0 + maint_ratio))
    } else {
        0.0
    };
    let distance_to_liq = if position_size != 0 {
        ((oracle_price - liq_price) / oracle_price * 100.0).abs()
    } else {
        0.0
    };

    // ── Max position at current equity ──────────────────────────────────
    let max_notional = equity * max_leverage;
    let max_position_base = if oracle_price > 0.0 {
        max_notional / oracle_price
    } else {
        0.0
    };

    // ── Pending funding ─────────────────────────────────────────────────
    // The on-chain settle hasn't run, so compute what the next settle would do
    let last_cumul = {
        // Read last_cumulative_funding from the seat directly
        let (base_bal, _) = market.get_trader_balance(trader);
        base_bal.as_u64() as i64
    };
    let funding_delta = cumulative_funding - last_cumul;
    let pending_funding = if position_size != 0 && funding_delta != 0 {
        (position_size as i128 * funding_delta as i128 / 1_000_000_000i128) as f64
            / quote_factor
    } else {
        0.0
    };

    // ── Display ─────────────────────────────────────────────────────────
    println!("═══════════════════════════════════════════════════════");
    println!("  Market    : {market_key}");
    println!("  Trader    : {trader}");
    println!("═══════════════════════════════════════════════════════");
    println!();
    println!("── Oracle ─────────────────────────────────────────────");
    println!("  Price           : ${oracle_price:.4}");
    println!("  Mantissa        : {oracle_mantissa}");
    println!("  Exponent        : {oracle_expo}");
    println!();
    println!("── Position ───────────────────────────────────────────");
    println!("  Direction       : {direction}");
    println!(
        "  Size            : {abs_pos:.6} base ({position_size} atoms)"
    );
    println!("  Entry Price     : ${entry_price:.4}");
    println!("  Cost Basis      : ${cost_usd:.4}");
    println!("  Notional        : ${notional:.4}");
    println!();
    println!("── Margin & Equity ────────────────────────────────────");
    println!("  Margin (deposit): ${margin:.4} ({margin_atoms} atoms)");
    println!("  Unrealized PnL  : ${unrealized_pnl:+.4}");
    println!(
        "  Pending Funding : ${pending_funding:+.4}{}",
        if pending_funding < 0.0 {
            " (will reduce equity)"
        } else if pending_funding > 0.0 {
            " (will increase equity)"
        } else {
            ""
        }
    );
    println!("  Equity          : ${equity:.4}");
    println!();
    println!("── Leverage & Liquidation ─────────────────────────────");
    println!("  Effective Leverage : {leverage:.2}x");
    println!(
        "  Max Leverage       : {max_leverage:.1}x (initial margin {initial_margin_bps} bps = {}%)",
        initial_margin_bps as f64 / 100.0
    );
    println!(
        "  Maint. Leverage    : {maint_leverage:.1}x (maintenance {maintenance_margin_bps} bps = {}%)",
        maintenance_margin_bps as f64 / 100.0
    );
    if position_size != 0 {
        println!("  Liquidation Price  : ${liq_price:.4} ({distance_to_liq:.2}% away)");
    } else {
        println!("  Liquidation Price  : N/A (no position)");
    }
    println!();
    println!("── Max Position (at current equity) ───────────────────");
    println!("  Max Notional     : ${max_notional:.2}");
    println!("  Max Size         : {max_position_base:.6} base");
    println!();
    println!("── Market Parameters ──────────────────────────────────");
    println!("  Taker Fee        : {} bps ({:.3}%)", taker_fee_bps, taker_fee_bps as f64 / 100.0);
    println!(
        "  Liq. Buffer      : {liq_buffer_bps} bps ({:.1}%)",
        liq_buffer_bps as f64 / 100.0
    );
    println!("  Insurance Fund   : ${:.4} ({insurance_fund} atoms)", insurance_fund as f64 / quote_factor);
    println!(
        "  Cumul. Funding   : {cumulative_funding} (scaled by 1e9)"
    );
    println!();

    Ok(())
}

fn cmd_setup(
    devnet: &RpcClient,
    er: &RpcClient,
    payer: &Keypair,
    market: &Pubkey,
    quote_mint: &Pubkey,
    amount: u64,
    mint_authority: &Keypair,
) -> Result<()> {
    println!("═══════════════════════════════════════════════");
    println!("  User onboarding for stress testing");
    println!("  Market     : {market}");
    println!("  Quote mint : {quote_mint}");
    println!("  User       : {}", payer.pubkey());
    println!("  Amount     : {amount} atoms");
    println!("═══════════════════════════════════════════════\n");

    // ── Step 1: Mint tokens to user ─────────────────────────────────────
    println!("── Step 1: Minting {amount} quote tokens to user…");
    let user_ata = get_associated_token_address(&payer.pubkey(), quote_mint);
    let create_ata_ix = create_associated_token_account_idempotent(
        &payer.pubkey(),
        &payer.pubkey(),
        quote_mint,
        &spl_token::id(),
    );
    let mint_ix = spl_token::instruction::mint_to(
        &spl_token::id(),
        quote_mint,
        &user_ata,
        &mint_authority.pubkey(),
        &[&mint_authority.pubkey()],
        amount,
    )?;
    let sig = send(devnet, &[create_ata_ix, mint_ix], &[payer, mint_authority])?;
    println!("  ATA: {user_ata}");
    println!("  Minted: {sig}");

    // ── Step 2: Initialize ephemeral ATA ────────────────────────────────
    println!("\n── Step 2: Initializing ephemeral ATA…");
    let (eph_ata, _) = get_ephemeral_ata(&payer.pubkey(), quote_mint);
    println!("  Ephemeral ATA: {eph_ata}");
    let ix = ix_init_ephemeral_ata(&payer.pubkey(), &payer.pubkey(), quote_mint);
    match send(devnet, &[ix], &[payer]) {
        Ok(sig) => println!("  Initialized: {sig}"),
        Err(_) => println!("  Already exists (skipping)"),
    }

    // ── Step 3: Deposit SPL tokens into ephemeral ATA ───────────────────
    println!("\n── Step 3: Depositing {amount} atoms into ephemeral ATA…");
    let ix = ix_deposit_spl_tokens(&payer.pubkey(), &payer.pubkey(), quote_mint, &user_ata, amount);
    match send(devnet, &[ix], &[payer]) {
        Ok(sig) => println!("  Deposited: {sig}"),
        Err(_) => println!("  Already deposited or delegated (skipping)"),
    }

    // ── Step 4: Delegate ephemeral ATA to ER ────────────────────────────
    println!("\n── Step 4: Delegating ephemeral ATA to MagicBlock ER…");
    let ix = ix_delegate_ephemeral_ata(&payer.pubkey(), quote_mint);
    match send(devnet, &[ix], &[payer]) {
        Ok(sig) => println!("  Delegated: {sig}"),
        Err(_) => println!("  Already delegated (skipping)"),
    }

    // ── Step 5: Claim seat on ER ────────────────────────────────────────
    println!("\n── Step 5: Claiming seat on market (ER)…");
    let ix = claim_seat_instruction(market, &payer.pubkey());
    match send(er, &[ix], &[payer]) {
        Ok(sig) => println!("  Seat claimed: {sig}"),
        Err(_) => println!("  Seat already claimed (skipping)"),
    }

    // ── Step 6: Ephemeral manifest deposit on ER ────────────────────────
    println!("\n── Step 6: Depositing {amount} atoms into Manifest market (ER)…");
    let trader_ata = get_associated_token_address(&payer.pubkey(), quote_mint);
    let vault_ata = get_associated_token_address(market, quote_mint);
    println!("  Trader ephemeral ATA : {trader_ata}");
    println!("  Market vault ATA     : {vault_ata}");
    let ix = deposit_instruction_with_vault(
        market,
        &payer.pubkey(),
        quote_mint,
        amount,
        &trader_ata,
        &vault_ata,
        spl_token::id(),
        None,
    );
    let sig = send(er, &[ix], &[payer])?;
    println!("  Deposited: {sig}");

    println!("\n═══════════════════════════════════════════════");
    println!("  User {} is ready to trade!", payer.pubkey());
    println!("  Market  : {market}");
    println!("  Margin  : {amount} quote atoms deposited");
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

        Commands::Expand {
            market,
            blocks,
            escrow_validator,
            escrow_slot,
        } => {
            let market = parse_pubkey(&market)?;
            let validator = parse_pubkey(&escrow_validator)?;
            cmd_expand(&client, &payer, &market, blocks, &validator, escrow_slot)?;
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

        Commands::Swap {
            market,
            quote_mint,
            in_atoms,
            min_out_atoms,
            is_base_in,
        } => {
            let market = parse_pubkey(&market)?;
            let quote_mint = parse_pubkey(&quote_mint)?;
            cmd_swap(&client, &payer, &market, &quote_mint, in_atoms, min_out_atoms, is_base_in)?;
        }

        Commands::MarketInfo { market } => {
            let market = parse_pubkey(&market)?;
            cmd_market_info(&client, &market)?;
        }

        Commands::Orderbook { market } => {
            let market = parse_pubkey(&market)?;
            cmd_orderbook(&client, &market)?;
        }

        Commands::Position { market, trader } => {
            let market = parse_pubkey(&market)?;
            let trader = trader.as_deref().map(parse_pubkey).transpose()?.unwrap_or(payer.pubkey());
            cmd_position(&client, &market, &trader)?;
        }

        Commands::Setup {
            market,
            quote_mint,
            amount,
            mint_authority,
        } => {
            let market = parse_pubkey(&market)?;
            let quote_mint = parse_pubkey(&quote_mint)?;
            let mint_auth_path = if mint_authority.starts_with("~/") {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                format!("{}{}", home, &mint_authority[1..])
            } else {
                mint_authority
            };
            let mint_auth = read_keypair_file(&mint_auth_path)
                .map_err(|e| anyhow!("Failed to load mint authority from {}: {}", mint_auth_path, e))?;
            cmd_setup(
                &client,
                &er,
                &payer,
                &market,
                &quote_mint,
                amount,
                &mint_auth,
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

        Commands::CreateEscrow {
            validator,
            slot,
            lamports,
        } => {
            let validator = parse_pubkey(&validator)?;
            cmd_create_escrow(&client, &payer, &validator, slot, lamports)?;
        }

        Commands::DelegateEscrow { validator, slot } => {
            let validator = parse_pubkey(&validator)?;
            cmd_delegate_escrow(&client, &payer, &validator, slot)?;
        }

        Commands::FundEscrow {
            validator,
            slot,
            lamports,
        } => {
            let validator = parse_pubkey(&validator)?;
            cmd_fund_escrow(&client, &payer, &validator, slot, lamports)?;
        }

        // Handled above before the network/keypair setup; unreachable here.
        Commands::Config { .. } => unreachable!(),
    }

    Ok(())
}
