use anyhow::{anyhow, Result};
use hypertree::HyperTreeValueIteratorTrait;
use manifest::quantities::WrapperU64;
use manifest::state::market::MarketFixed;
use manifest::state::{MarketValue, RestingOrder, MARKET_FIXED_SIZE};
use solana_client::rpc_client::RpcClient;
use solana_program::pubkey::Pubkey;

/// Parsed market state, wrapping the on-chain `MarketFixed` header and the
/// dynamic orderbook/seat data.
pub struct MarketState {
    pub key: Pubkey,
    pub fixed: MarketFixed,
    pub market: MarketValue,
}

impl MarketState {
    /// Fetch and parse a market account from an RPC endpoint.
    pub fn fetch(client: &RpcClient, market_key: &Pubkey) -> Result<Self> {
        let account = client.get_account(market_key)?;
        Self::from_account_data(*market_key, &account.data)
    }

    /// Parse from raw account data (no RPC needed).
    pub fn from_account_data(market_key: Pubkey, data: &[u8]) -> Result<Self> {
        if data.len() < MARKET_FIXED_SIZE {
            return Err(anyhow!(
                "Account data too small for MarketFixed ({} < {})",
                data.len(),
                MARKET_FIXED_SIZE
            ));
        }
        let fixed: &MarketFixed = bytemuck::from_bytes(&data[..MARKET_FIXED_SIZE]);
        let dynamic = &data[MARKET_FIXED_SIZE..];
        let market = MarketValue {
            fixed: *fixed,
            dynamic: dynamic.to_vec(),
        };
        Ok(Self {
            key: market_key,
            fixed: *fixed,
            market,
        })
    }

    /// Oracle price as a human-readable f64 (USD).
    pub fn oracle_price(&self) -> f64 {
        let mantissa = self.fixed.get_oracle_price_mantissa();
        let expo = self.fixed.get_oracle_price_expo();
        mantissa as f64 * 10f64.powi(expo)
    }

    /// Trader position: `(position_size_atoms, cost_basis_atoms)`.
    /// `position_size` is signed (positive = long, negative = short).
    pub fn get_trader_position(&self, trader: &Pubkey) -> (i64, u64) {
        self.market.get_trader_position(trader)
    }

    /// Trader quote balance (margin) in quote atoms.
    pub fn get_trader_balance(&self, trader: &Pubkey) -> u64 {
        let (_, quote) = self.market.get_trader_balance(trader);
        quote.as_u64()
    }

    /// Trader base_withdrawable_balance raw value (stores last_cumulative_funding
    /// between transactions in perps).
    pub fn get_trader_last_cumulative_funding(&self, trader: &Pubkey) -> i64 {
        let (base, _) = self.market.get_trader_balance(trader);
        base.as_u64() as i64
    }

    /// Base asset decimals.
    pub fn base_decimals(&self) -> u32 {
        self.fixed.get_base_mint_decimals() as u32
    }

    /// Quote asset decimals.
    pub fn quote_decimals(&self) -> u32 {
        self.fixed.get_quote_mint_decimals() as u32
    }

    /// Initial margin requirement in basis points.
    pub fn initial_margin_bps(&self) -> u64 {
        self.fixed.get_initial_margin_bps()
    }

    /// Maintenance margin requirement in basis points.
    pub fn maintenance_margin_bps(&self) -> u64 {
        self.fixed.get_maintenance_margin_bps()
    }

    /// Taker fee in basis points.
    pub fn taker_fee_bps(&self) -> u64 {
        self.fixed.get_taker_fee_bps()
    }

    /// Liquidation buffer above maintenance margin in basis points.
    pub fn liquidation_buffer_bps(&self) -> u64 {
        self.fixed.get_liquidation_buffer_bps()
    }

    /// Insurance fund balance in quote atoms.
    pub fn insurance_fund_balance(&self) -> u64 {
        self.fixed.get_insurance_fund_balance()
    }

    /// Global cumulative funding (scaled by 1e9).
    pub fn cumulative_funding(&self) -> i64 {
        self.fixed.get_cumulative_funding()
    }

    /// Get all resting bid orders (sorted highest price first).
    pub fn get_resting_bids(&self) -> Vec<RestingOrder> {
        self.market
            .get_bids()
            .iter::<RestingOrder>()
            .map(|(_, o)| *o)
            .collect()
    }

    /// Get all resting ask orders (sorted lowest price first).
    pub fn get_resting_asks(&self) -> Vec<RestingOrder> {
        self.market
            .get_asks()
            .iter::<RestingOrder>()
            .map(|(_, o)| *o)
            .collect()
    }

    /// Get all resting orders (bids then asks).
    pub fn get_resting_orders(&self) -> Vec<RestingOrder> {
        let mut orders = self.get_resting_bids();
        orders.extend(self.get_resting_asks());
        orders
    }
}
