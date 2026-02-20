use solana_program::pubkey::Pubkey;

use crate::market::MarketState;

/// Position direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Long,
    Short,
    Flat,
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Direction::Long => write!(f, "LONG"),
            Direction::Short => write!(f, "SHORT"),
            Direction::Flat => write!(f, "FLAT"),
        }
    }
}

/// Full position analytics for a trader on a perps market.
#[derive(Debug, Clone)]
pub struct PositionInfo {
    pub direction: Direction,
    /// Signed position size in base atoms (positive = long).
    pub position_atoms: i64,
    /// Absolute position in human-readable base units.
    pub position_base: f64,
    /// Total cost basis in USD.
    pub cost_basis: f64,
    /// Average entry price in USD per base unit.
    pub entry_price: f64,
    /// Current notional value in USD.
    pub notional: f64,
    /// Deposited margin in USD.
    pub margin: f64,
    /// Unrealized PnL in USD.
    pub unrealized_pnl: f64,
    /// Pending funding payment in USD (negative = reduces equity).
    pub pending_funding: f64,
    /// Equity = margin + unrealized PnL.
    pub equity: f64,
    /// Effective leverage = notional / equity.
    pub effective_leverage: f64,
    /// Liquidation price in USD.
    pub liquidation_price: f64,
    /// Distance to liquidation as a percentage of current price.
    pub distance_to_liq_pct: f64,
    /// Maximum notional at current equity and initial margin.
    pub max_notional: f64,
    /// Maximum position size in base units at current equity.
    pub max_position_base: f64,
}

impl PositionInfo {
    /// Compute full position analytics from a parsed market state and trader pubkey.
    pub fn compute(market: &MarketState, trader: &Pubkey) -> Self {
        let oracle_price = market.oracle_price();
        let (position_atoms, cost_basis_atoms) = market.get_trader_position(trader);
        let margin_atoms = market.get_trader_balance(trader);
        let cumulative_funding = market.cumulative_funding();
        let last_cumulative_funding = market.get_trader_last_cumulative_funding(trader);

        Self::from_raw(
            oracle_price,
            position_atoms,
            cost_basis_atoms,
            margin_atoms,
            cumulative_funding,
            last_cumulative_funding,
            market.base_decimals(),
            market.quote_decimals(),
            market.initial_margin_bps(),
            market.maintenance_margin_bps(),
        )
    }

    /// Compute from raw values (no RPC needed). Useful for simulations or
    /// when you already have the on-chain data deserialized.
    pub fn from_raw(
        oracle_price: f64,
        position_atoms: i64,
        cost_basis_atoms: u64,
        margin_atoms: u64,
        cumulative_funding: i64,
        last_cumulative_funding: i64,
        base_decimals: u32,
        quote_decimals: u32,
        initial_margin_bps: u64,
        maintenance_margin_bps: u64,
    ) -> Self {
        let base_factor = 10f64.powi(base_decimals as i32);
        let quote_factor = 10f64.powi(quote_decimals as i32);

        let is_long = position_atoms > 0;
        let is_short = position_atoms < 0;
        let direction = if is_long {
            Direction::Long
        } else if is_short {
            Direction::Short
        } else {
            Direction::Flat
        };

        let abs_pos = position_atoms.unsigned_abs() as f64 / base_factor;
        let notional = abs_pos * oracle_price;
        let margin = margin_atoms as f64 / quote_factor;
        let cost_basis = cost_basis_atoms as f64 / quote_factor;
        let entry_price = if position_atoms != 0 {
            cost_basis / abs_pos
        } else {
            0.0
        };

        // PnL: LONG = value - cost, SHORT = cost - value
        let current_value = abs_pos * oracle_price;
        let unrealized_pnl = if is_long {
            current_value - cost_basis
        } else if is_short {
            cost_basis - current_value
        } else {
            0.0
        };

        let equity = margin + unrealized_pnl;
        let effective_leverage = if equity > 0.0 && position_atoms != 0 {
            notional / equity
        } else {
            0.0
        };

        // Liquidation price
        let maint_ratio = maintenance_margin_bps as f64 / 10_000.0;
        let liquidation_price = if is_long {
            (cost_basis - margin) / (abs_pos * (1.0 - maint_ratio))
        } else if is_short {
            (margin + cost_basis) / (abs_pos * (1.0 + maint_ratio))
        } else {
            0.0
        };
        let distance_to_liq_pct = if position_atoms != 0 {
            ((oracle_price - liquidation_price) / oracle_price * 100.0).abs()
        } else {
            0.0
        };

        // Max position at current equity
        let max_leverage = 10_000.0 / initial_margin_bps as f64;
        let max_notional = equity * max_leverage;
        let max_position_base = if oracle_price > 0.0 {
            max_notional / oracle_price
        } else {
            0.0
        };

        // Pending funding
        let funding_delta = cumulative_funding - last_cumulative_funding;
        let pending_funding = if position_atoms != 0 && funding_delta != 0 {
            (position_atoms as i128 * funding_delta as i128 / 1_000_000_000i128) as f64
                / quote_factor
        } else {
            0.0
        };

        Self {
            direction,
            position_atoms,
            position_base: abs_pos,
            cost_basis,
            entry_price,
            notional,
            margin,
            unrealized_pnl,
            pending_funding,
            equity,
            effective_leverage,
            liquidation_price,
            distance_to_liq_pct,
            max_notional,
            max_position_base,
        }
    }
}
