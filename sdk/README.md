# manifest-sdk

Rust SDK for the Manifest Perps DEX on Solana + MagicBlock Ephemeral Rollup.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
manifest-sdk = { path = "../sdk" }
# or if published:
# manifest-sdk = "0.1.0"
```

## Quick Start

```rust
use manifest_sdk::client::ManifestClient;
use manifest_sdk::config::ManifestConfig;
use manifest_sdk::instructions::PlaceOrderParams;
use manifest_sdk::state::OrderType;
use solana_program::pubkey::Pubkey;
use solana_sdk::signature::read_keypair_file;
use solana_sdk::signer::Signer;
use std::str::FromStr;

fn main() -> anyhow::Result<()> {
    let payer = read_keypair_file("~/.config/solana/id.json")?;
    let market = Pubkey::from_str("CCkhp6HH9GSp81dj31xGcYgVsmBErsWiEvdbZ1ed6ouU")?;

    // Initialize with default devnet config (connects to ER)
    let client = ManifestClient::init(ManifestConfig::default());

    // Read position
    let pos = client.fetch_position(&market, &payer.pubkey())?;
    println!("{} {:.4} SOL @ ${:.2}", pos.direction, pos.position_base, pos.entry_price);
    println!("  Leverage : {:.2}x", pos.effective_leverage);
    println!("  PnL      : ${:+.4}", pos.unrealized_pnl);
    println!("  Liq price: ${:.2}", pos.liquidation_price);

    // Place an IOC bid (go long)
    let sig = client.place_order(&payer, &market, PlaceOrderParams::new(
        1_000_000_000, // 1 SOL in base atoms
        14000,         // price mantissa
        -5,            // price exponent
        true,          // is_bid (long)
        OrderType::ImmediateOrCancel,
        0,             // no expiry
    ))?;
    println!("Order placed: {sig}");

    Ok(())
}
```

## Configuration

Use `ManifestConfig` to customize program IDs, RPC URLs, and other settings.
All fields have sensible devnet defaults.

```rust
use manifest_sdk::config::ManifestConfig;
use manifest_sdk::client::ManifestClient;

// Default devnet config — connects to MagicBlock ER
let client = ManifestClient::init(ManifestConfig::default());

// Custom ER endpoint
let config = ManifestConfig::builder()
    .er_url("https://my-er.example.com")
    .build();
let client = ManifestClient::init(config);

// Fully custom (e.g. different deployment)
let config = ManifestConfig::builder()
    .base_url("https://api.mainnet-beta.solana.com")
    .er_url("https://mainnet-er.example.com")
    .manifest_program_id("MyProgram111111111111111111111111111111111")
    .ephemeral_spl_token_id("MySplToken11111111111111111111111111111111")
    .delegation_program_id("MyDelegation111111111111111111111111111111")
    .pyth_feed("MyPythFeed1111111111111111111111111111111111")
    .build();
let client = ManifestClient::init(config);

// Connect to base chain instead of ER
let config = ManifestConfig::default();
let devnet_client = ManifestClient::init_with_url(config, "https://api.devnet.solana.com");
```

### Config fields

| Field | Default | Description |
|-------|---------|-------------|
| `base_url` | `https://api.devnet.solana.com` | Base chain RPC URL |
| `er_url` | `https://devnet.magicblock.app` | MagicBlock ER RPC URL |
| `manifest_program_id` | `3TN9efyWfeG3s1ZDZdbYtLJwMdWRRtM2xPGsM2T9QrUa` | Manifest DEX program |
| `ephemeral_spl_token_id` | `SPLxh1LVZzEkX99H6rqYizhytLWPZVV296zyYDPagv2` | Ephemeral SPL Token program |
| `delegation_program_id` | `DELeGGvXpWV2fqJUhqcF5ZSYMS4JTLjteaAMARRSaeSh` | MagicBlock Delegation program |
| `pyth_feed` | `ENYwebBThHzmzwPLAQvCucUTsjyfBSZdD9ViXksS4jPu` | Default Pyth SOL/USD feed |

## Modules

### `client` — High-level RPC client

`ManifestClient` wraps `RpcClient` + `ManifestConfig` with typed methods:

```rust
let client = ManifestClient::init(ManifestConfig::default());

// Read
client.fetch_market(&market)?;                    // → MarketState
client.fetch_position(&market, &trader)?;          // → PositionInfo
client.fetch_oracle_price(&feed, 6, 9)?;           // → (mantissa, exponent, price_usd)

// Write (base chain)
client.create_market(&payer, params)?;             // → (market_pubkey, sig)
client.claim_seat(&payer, &market)?;
client.deposit(&payer, &market, &quote_mint, amount)?;
client.withdraw(&payer, &market, &quote_mint, amount)?;
client.place_order(&payer, &market, order_params)?;
client.cancel_order(&payer, &market, seq_num)?;
client.liquidate(&liquidator, &market, &trader)?;
client.crank_funding(&payer, &market, &pyth_feed)?;

// Write (ephemeral rollup)
client.swap(&payer, &market, swap_params)?;
client.delegate_market(&payer, &market, &quote_mint)?;
client.ephemeral_deposit(&payer, &market, &quote_mint, amount)?;
client.ephemeral_withdraw(&payer, &market, &quote_mint, amount)?;
```

### `market` — Parsed market state

```rust
use manifest_sdk::market::MarketState;

let state = MarketState::fetch(&rpc_client, &market_key)?;
// or from raw bytes:
let state = MarketState::from_account_data(market_key, &account_data)?;

state.oracle_price();              // f64 USD
state.get_trader_position(&trader); // (i64 position_atoms, u64 cost_basis)
state.get_trader_balance(&trader);  // u64 margin in quote atoms
state.initial_margin_bps();
state.maintenance_margin_bps();
state.cumulative_funding();
```

### `position` — Position analytics

```rust
use manifest_sdk::position::{PositionInfo, Direction};

let pos = PositionInfo::compute(&market_state, &trader);
// or from raw values (no RPC needed):
let pos = PositionInfo::from_raw(
    oracle_price, position_atoms, cost_basis_atoms, margin_atoms,
    cumulative_funding, last_cumulative_funding,
    base_decimals, quote_decimals,
    initial_margin_bps, maintenance_margin_bps,
);

pos.direction;           // Direction::Long | Short | Flat
pos.entry_price;         // USD
pos.notional;            // USD
pos.margin;              // USD
pos.unrealized_pnl;      // USD
pos.equity;              // margin + pnl
pos.effective_leverage;   // notional / equity
pos.liquidation_price;    // USD
pos.distance_to_liq_pct; // % from current price
pos.max_position_base;    // max size at current equity
pos.pending_funding;      // USD (unsettled)
```

### `oracle` — Pyth price parsing

```rust
use manifest_sdk::oracle;

// Pyth V2 push oracle (devnet)
let (mantissa, expo, price_usd) = oracle::fetch_pyth_v2_price(&client, &feed, 6, 9)?;

// Pyth V3 PriceUpdateV3 (MagicBlock ER)
let (mantissa, expo, price_usd) = oracle::fetch_er_price(&client, &feed, 6, 9)?;

// Auto-detect (tries V2 then V3)
let (mantissa, expo, price_usd) = oracle::fetch_price(&client, &feed, 6, 9)?;

// Convert USD price to order mantissa+exponent
let (m, e) = oracle::usd_to_order_price(142.50, 6, 9);
```

### `ephemeral` — MagicBlock ER helpers

All ephemeral functions take a `&ManifestConfig` for program IDs:

```rust
use manifest_sdk::ephemeral;
use manifest_sdk::config::ManifestConfig;

let cfg = ManifestConfig::default();

// PDA derivation
let (ata, bump) = ephemeral::get_ephemeral_ata(&cfg, &owner, &mint);
let (vault, bump) = ephemeral::get_global_vault(&cfg, &mint);

// Instruction builders
let ix = ephemeral::ix_init_global_vault(&cfg, &payer, &mint);
let ix = ephemeral::ix_init_ephemeral_ata(&cfg, &payer, &owner, &mint);
let ix = ephemeral::ix_deposit_spl_tokens(&cfg, &authority, &recipient, &mint, &source, amount);
let ix = ephemeral::ix_delegate_ephemeral_ata(&cfg, &payer, &mint);
let ix = ephemeral::delegate_market_ix(&cfg, &payer, &market, &quote_mint);
```

### `instructions` — Re-exported instruction builders

All instruction builders from `manifest-dex` are re-exported for convenience:

```rust
use manifest_sdk::instructions::*;

let ix = create_market_instructions(index, decimals, &quote_mint, &creator, ...);
let ix = batch_update_instruction(&market, &payer, None, cancels, orders, ...);
let ix = swap_instruction_with_vaults(&market, &payer, ...);
```

### `state` / `quantities` / `validation` — Re-exported types

```rust
use manifest_sdk::state::{MarketFixed, OrderType, MARKET_FIXED_SIZE};
use manifest_sdk::quantities::{QuoteAtoms, WrapperU64};
use manifest_sdk::validation::get_market_address;

let (market_pda, bump) = get_market_address(0, &quote_mint);
```

## Two-client pattern (base chain + ER)

Most workflows need two clients — one for the Solana base chain and one for the MagicBlock ER.
Both share the same config:

```rust
use manifest_sdk::client::ManifestClient;
use manifest_sdk::config::ManifestConfig;

let config = ManifestConfig::default();

// ER client (default — init connects to er_url)
let er = ManifestClient::init(config.clone());

// Base chain client
let devnet = ManifestClient::init_with_url(config, &config.base_url);

// Setup on base chain
devnet.create_market(&payer, params)?;
devnet.claim_seat(&payer, &market)?;
devnet.deposit(&payer, &market, &quote_mint, 100_000_000)?;
devnet.delegate_market(&payer, &market, &quote_mint)?;

// Trade on ER
er.place_order(&payer, &market, order)?;
let pos = er.fetch_position(&market, &payer.pubkey())?;
```
