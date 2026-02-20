//! Rust SDK for the Manifest Perps DEX.
//!
//! Provides a high-level [`ManifestClient`] for interacting with the on-chain
//! program, plus lower-level modules for oracle parsing, position analytics,
//! and MagicBlock Ephemeral Rollup helpers.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use manifest_sdk::client::ManifestClient;
//! use solana_program::pubkey::Pubkey;
//! use std::str::FromStr;
//!
//! let client = ManifestClient::new("https://devnet.magicblock.app");
//! let market = Pubkey::from_str("CCkhp6HH9GSp81dj31xGcYgVsmBErsWiEvdbZ1ed6ouU").unwrap();
//! let trader = Pubkey::from_str("7LknxjREbmNfap5dg2TYE5mU4RXGB2hg8ScXLcZRHMf8").unwrap();
//!
//! let pos = client.fetch_position(&market, &trader).unwrap();
//! println!("{} {:.4} base @ ${:.2}", pos.direction, pos.position_base, pos.entry_price);
//! ```

pub mod client;
pub mod config;
pub mod ephemeral;
pub mod error;
pub mod market;
pub mod oracle;
pub mod position;

// ── Re-exports from manifest-dex ────────────────────────────────────────────

/// Instruction builders for constructing on-chain transactions.
pub mod instructions {
    pub use manifest::program::{
        batch_update_instruction,
        batch_update::{CancelOrderParams, PlaceOrderParams},
        claim_seat_instruction::claim_seat_instruction,
        create_market_instructions,
        crank_funding_instruction,
        deposit_instruction, deposit_instruction_with_vault,
        expand_market_instruction, expand_market_n_instruction,
        liquidate_instruction,
        release_seat_instruction,
        swap_instruction::{swap_instruction, swap_instruction_with_vaults},
        withdraw_instruction, withdraw_instruction_with_vault,
        ManifestInstruction,
    };
}

/// On-chain state types.
pub mod state {
    pub use manifest::state::{
        MarketFixed, MarketValue, OrderType, RestingOrder, MARKET_FIXED_SIZE, MARKET_BLOCK_SIZE,
    };
}

/// Quantity types with the `WrapperU64` trait for `.as_u64()`.
pub mod quantities {
    pub use manifest::quantities::{BaseAtoms, QuoteAtoms, WrapperU64};
}

/// PDA derivation helpers.
pub mod validation {
    pub use manifest::validation::{get_market_address, get_vault_address};
}

/// The Manifest program ID.
pub fn id() -> solana_program::pubkey::Pubkey {
    manifest::id()
}
