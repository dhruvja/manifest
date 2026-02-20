//! perp-mm — Market maker bot for the Manifest Perps DEX.
//!
//! Polls the orderbook on the MagicBlock Ephemeral Rollup and sweeps any
//! resting orders by taking the other side via swap.
//!
//! Usage:
//!   perp-mm --market <MARKET_PUBKEY> --quote-mint <QUOTE_MINT> [OPTIONS]

use anyhow::Result;
use clap::Parser;
use manifest_sdk::{
    client::ManifestClient,
    config::ManifestConfig,
    market::MarketState,
    quantities::WrapperU64,
};
use solana_program::pubkey::Pubkey;
use solana_sdk::signature::{read_keypair_file, Keypair};
use solana_sdk::signer::Signer;
use std::str::FromStr;
use std::thread;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "perp-mm", about = "Market maker bot for Manifest Perps DEX")]
struct Cli {
    /// Market pubkey.
    #[arg(long)]
    market: String,

    /// Quote mint pubkey (e.g. USDC).
    #[arg(long)]
    quote_mint: String,

    /// Path to the market maker's keypair file.
    #[arg(long, default_value = "~/.config/solana/id.json")]
    keypair: String,

    /// MagicBlock Ephemeral Rollup RPC URL.
    #[arg(long, default_value = "https://devnet.magicblock.app")]
    er_url: String,

    /// Poll interval in seconds.
    #[arg(long, default_value_t = 2)]
    interval: u64,

    /// Only sweep asks (buy base). Mutually exclusive with --sells-only.
    #[arg(long, default_value_t = false)]
    buys_only: bool,

    /// Only sweep bids (sell base). Mutually exclusive with --buys-only.
    #[arg(long, default_value_t = false)]
    sells_only: bool,

    /// Run once instead of looping.
    #[arg(long, default_value_t = false)]
    once: bool,

    /// Dry run — print what would be done without sending transactions.
    #[arg(long, default_value_t = false)]
    dry_run: bool,
}

fn expand_tilde(path: &str) -> String {
    if path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{}{}", home, &path[1..]);
        }
    }
    path.to_string()
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let market_key = Pubkey::from_str(&cli.market)?;
    let quote_mint = Pubkey::from_str(&cli.quote_mint)?;
    let keypair_path = expand_tilde(&cli.keypair);
    let payer = read_keypair_file(&keypair_path)
        .map_err(|e| anyhow::anyhow!("Failed to read keypair {}: {}", keypair_path, e))?;

    let config = ManifestConfig::builder()
        .er_url(&cli.er_url)
        .build();
    let client = ManifestClient::init(config);

    println!("perp-mm starting");
    println!("  Market:     {}", market_key);
    println!("  Quote mint: {}", quote_mint);
    println!("  Payer:      {}", payer.pubkey());
    println!("  ER URL:     {}", cli.er_url);
    println!("  Interval:   {}s", cli.interval);
    if cli.dry_run {
        println!("  Mode:       DRY RUN");
    }
    println!();

    loop {
        match run_cycle(&client, &payer, &market_key, &quote_mint, &cli) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Error in cycle: {e:#}");
            }
        }

        if cli.once {
            break;
        }
        thread::sleep(Duration::from_secs(cli.interval));
    }

    Ok(())
}

fn run_cycle(
    client: &ManifestClient,
    payer: &Keypair,
    market_key: &Pubkey,
    quote_mint: &Pubkey,
    cli: &Cli,
) -> Result<()> {
    let state = client.fetch_market(market_key)?;

    let base_decimals = state.base_decimals();
    let base_factor = 10f64.powi(base_decimals as i32);

    // Sweep resting asks: someone is selling base, we buy it
    if !cli.sells_only {
        sweep_asks(client, payer, market_key, quote_mint, &state, base_factor, cli)?;
    }

    // Sweep resting bids: someone is buying base, we sell it
    if !cli.buys_only {
        sweep_bids(client, payer, market_key, quote_mint, &state, base_factor, cli)?;
    }

    Ok(())
}

/// Sweep resting asks by placing a swap that buys base (quote in, base out).
fn sweep_asks(
    client: &ManifestClient,
    payer: &Keypair,
    market_key: &Pubkey,
    quote_mint: &Pubkey,
    state: &MarketState,
    base_factor: f64,
    cli: &Cli,
) -> Result<()> {
    let asks = state.get_resting_asks();
    if asks.is_empty() {
        return Ok(());
    }

    let total_base_atoms: u64 = asks.iter().map(|o| o.get_num_base_atoms().as_u64()).sum();
    let total_base = total_base_atoms as f64 / base_factor;

    // Compute total quote needed (sum of base * price for each ask)
    let total_quote_atoms: u64 = asks
        .iter()
        .map(|o| {
            o.get_num_base_atoms()
                .checked_mul(o.get_price(), true)
                .map(|q| q.as_u64())
                .unwrap_or(0)
        })
        .sum();

    println!(
        "  ASKS: {} order(s), {:.4} base ({} atoms), ~{} quote atoms",
        asks.len(),
        total_base,
        total_base_atoms,
        total_quote_atoms,
    );

    if cli.dry_run {
        println!("    [dry run] Would swap {} quote atoms in (buy base)", total_quote_atoms);
        return Ok(());
    }

    // Swap: quote in, base out (is_base_in = false)
    let params = manifest_sdk::client::SwapParams {
        quote_mint: *quote_mint,
        in_atoms: total_quote_atoms,
        min_out_atoms: 0,
        is_base_in: false,
    };
    match client.swap(payer, market_key, params) {
        Ok(sig) => println!("    Swept asks: {sig}"),
        Err(e) => eprintln!("    Failed to sweep asks: {e:#}"),
    }

    Ok(())
}

/// Sweep resting bids by placing a swap that sells base (base in, quote out).
fn sweep_bids(
    client: &ManifestClient,
    payer: &Keypair,
    market_key: &Pubkey,
    quote_mint: &Pubkey,
    state: &MarketState,
    base_factor: f64,
    cli: &Cli,
) -> Result<()> {
    let bids = state.get_resting_bids();
    if bids.is_empty() {
        return Ok(());
    }

    let total_base_atoms: u64 = bids.iter().map(|o| o.get_num_base_atoms().as_u64()).sum();
    let total_base = total_base_atoms as f64 / base_factor;

    println!(
        "  BIDS: {} order(s), {:.4} base ({} atoms)",
        bids.len(),
        total_base,
        total_base_atoms,
    );

    if cli.dry_run {
        println!("    [dry run] Would swap {} base atoms in (sell base)", total_base_atoms);
        return Ok(());
    }

    // Swap: base in, quote out (is_base_in = true)
    let params = manifest_sdk::client::SwapParams {
        quote_mint: *quote_mint,
        in_atoms: total_base_atoms,
        min_out_atoms: 0,
        is_base_in: true,
    };
    match client.swap(payer, market_key, params) {
        Ok(sig) => println!("    Swept bids: {sig}"),
        Err(e) => eprintln!("    Failed to sweep bids: {e:#}"),
    }

    Ok(())
}
