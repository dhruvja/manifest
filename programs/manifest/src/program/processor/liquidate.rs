use crate::{
    logs::{emit_stack, LiquidateLog},
    program::{get_mut_dynamic_account, ManifestError},
    quantities::{BaseAtoms, QuoteAtoms, QuoteAtomsPerBaseAtom, WrapperU64},
    require,
    state::{claimed_seat::ClaimedSeat, MarketRefMut, RestingOrder},
    validation::loaders::LiquidateContext,
};
use borsh::{BorshDeserialize, BorshSerialize};
use hypertree::{get_helper, get_mut_helper, DataIndex, RBNode};
use solana_program::{
    account_info::AccountInfo, entrypoint::ProgramResult, program_error::ProgramError,
    pubkey::Pubkey,
};
use std::cell::RefMut;

#[derive(BorshDeserialize, BorshSerialize)]
pub struct LiquidateParams {
    pub trader_to_liquidate: Pubkey,
}

impl LiquidateParams {
    pub fn new(trader_to_liquidate: Pubkey) -> Self {
        LiquidateParams {
            trader_to_liquidate,
        }
    }
}

pub(crate) fn process_liquidate(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    let params = LiquidateParams::try_from_slice(data)?;
    let liquidate_context: LiquidateContext = LiquidateContext::load(accounts)?;

    let LiquidateContext {
        market,
        liquidator,
    } = liquidate_context;

    let market_data: &mut RefMut<&mut [u8]> = &mut market.try_borrow_mut_data()?;
    let mut dynamic_account: MarketRefMut = get_mut_dynamic_account(market_data);

    // Find the trader's seat
    let trader_index: DataIndex =
        dynamic_account.get_trader_index(&params.trader_to_liquidate);
    require!(
        trader_index != hypertree::NIL,
        ProgramError::InvalidArgument,
        "Trader not found on market",
    )?;

    let claimed_seat: &ClaimedSeat = get_helper::<RBNode<ClaimedSeat>>(
        &dynamic_account.dynamic,
        trader_index,
    )
    .get_value();

    let position_size: i64 = claimed_seat.get_position_size();
    require!(
        position_size != 0,
        ManifestError::NotLiquidatable,
        "Trader has no open position",
    )?;

    let quote_cost_basis: u64 = claimed_seat.get_quote_cost_basis();
    let margin_balance: u64 = claimed_seat.quote_withdrawable_balance.as_u64();

    // Compute mark price from best bid/ask midpoint
    let mark_price: QuoteAtomsPerBaseAtom = compute_mark_price(&dynamic_account)?;

    // Compute current market value of position: mark_price * |position_size|
    let abs_position: u64 = position_size.unsigned_abs();
    let current_value: u64 = mark_price
        .checked_quote_for_base(BaseAtoms::new(abs_position), false)?
        .as_u64();

    // Compute unrealized PnL
    // For longs: pnl = current_value - cost_basis
    // For shorts: pnl = cost_basis - current_value
    let unrealized_pnl: i64 = if position_size > 0 {
        (current_value as i64).wrapping_sub(quote_cost_basis as i64)
    } else {
        (quote_cost_basis as i64).wrapping_sub(current_value as i64)
    };

    // Equity = margin + unrealized_pnl
    let equity: i128 = (margin_balance as i128) + (unrealized_pnl as i128);

    // Maintenance margin = current_value * maintenance_margin_bps / 10000
    let maintenance_margin_bps: u64 = dynamic_account.fixed.get_maintenance_margin_bps();
    let required_maintenance: u64 = current_value
        .checked_mul(maintenance_margin_bps)
        .unwrap_or(u64::MAX)
        / 10000;

    require!(
        equity < required_maintenance as i128,
        ManifestError::NotLiquidatable,
        "Trader equity {} >= maintenance margin {}, not liquidatable",
        equity,
        required_maintenance,
    )?;

    // Liquidate: settle position at mark price
    let settlement_pnl: i64 = unrealized_pnl;

    // Update the trader's seat: close position, settle PnL
    {
        let claimed_seat_mut: &mut ClaimedSeat = get_mut_helper::<RBNode<ClaimedSeat>>(
            &mut dynamic_account.dynamic,
            trader_index,
        )
        .get_mut_value();

        // Close position
        claimed_seat_mut.set_position_size(0);
        claimed_seat_mut.set_quote_cost_basis(0);

        // Settle PnL into margin balance
        let new_margin = if settlement_pnl >= 0 {
            margin_balance.saturating_add(settlement_pnl as u64)
        } else {
            margin_balance.saturating_sub(settlement_pnl.unsigned_abs())
        };
        claimed_seat_mut.quote_withdrawable_balance = QuoteAtoms::new(new_margin);
    }

    // Update global position tracking
    #[cfg(not(feature = "certora"))]
    {
        if position_size > 0 {
            let current = dynamic_account.fixed.get_total_long_base_atoms();
            dynamic_account
                .fixed
                .set_total_long_base_atoms(current.saturating_sub(abs_position));
        } else {
            let current = dynamic_account.fixed.get_total_short_base_atoms();
            dynamic_account
                .fixed
                .set_total_short_base_atoms(current.saturating_sub(abs_position));
        }
    }

    emit_stack(LiquidateLog {
        market: *market.key,
        liquidator: *liquidator.key,
        trader: params.trader_to_liquidate,
        position_size: position_size as u64,
        settlement_price: current_value,
        pnl: settlement_pnl as u64,
        _padding: [0; 8],
    })?;

    Ok(())
}

/// Compute mark price from the best bid/ask midpoint.
pub(crate) fn compute_mark_price(market: &MarketRefMut) -> Result<QuoteAtomsPerBaseAtom, ProgramError> {
    let best_bid_index = market.fixed.get_bids_best_index();
    let best_ask_index = market.fixed.get_asks_best_index();

    // Need at least one side of the book to determine price
    require!(
        best_bid_index != hypertree::NIL || best_ask_index != hypertree::NIL,
        ManifestError::InvalidPerpsOperation,
        "Cannot compute mark price: empty orderbook",
    )?;

    if best_bid_index != hypertree::NIL && best_ask_index != hypertree::NIL {
        let best_bid: &RestingOrder =
            get_helper::<RBNode<RestingOrder>>(&market.dynamic, best_bid_index).get_value();
        let best_ask: &RestingOrder =
            get_helper::<RBNode<RestingOrder>>(&market.dynamic, best_ask_index).get_value();
        // Use the bid price as a conservative mark price for liquidation
        // (A proper midpoint would need u128 arithmetic on QuoteAtomsPerBaseAtom)
        if best_bid.get_price() <= best_ask.get_price() {
            Ok(best_bid.get_price())
        } else {
            Ok(best_ask.get_price())
        }
    } else if best_bid_index != hypertree::NIL {
        let best_bid: &RestingOrder =
            get_helper::<RBNode<RestingOrder>>(&market.dynamic, best_bid_index).get_value();
        Ok(best_bid.get_price())
    } else {
        let best_ask: &RestingOrder =
            get_helper::<RBNode<RestingOrder>>(&market.dynamic, best_ask_index).get_value();
        Ok(best_ask.get_price())
    }
}
