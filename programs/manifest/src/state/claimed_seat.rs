use std::mem::size_of;

use crate::quantities::WrapperU64;
use crate::quantities::{BaseAtoms, QuoteAtoms};
use bytemuck::{Pod, Zeroable};
use shank::ShankType;
use solana_program::pubkey::Pubkey;
use static_assertions::const_assert_eq;
use std::cmp::Ordering;

use super::constants::CLAIMED_SEAT_SIZE;

#[repr(C)]
#[derive(Default, Debug, Copy, Clone, Zeroable, Pod, ShankType)]
pub struct ClaimedSeat {
    pub trader: Pubkey,
    // Balances are withdrawable on the exchange. They do not include funds in
    // open orders. When moving funds over to open orders, use the worst case
    // rounding.
    pub base_withdrawable_balance: BaseAtoms,
    pub quote_withdrawable_balance: QuoteAtoms,
    /// Quote volume traded over lifetime, can overflow. Double counts self
    /// trades. This is for informational and monitoring purposes only. This is
    /// not guaranteed to be maintained. It does not secure any value in
    /// manifest. Use at your own risk.
    pub quote_volume: QuoteAtoms,
    _padding: [u8; 8],
}
// 32 + // trader
//  8 + // base_balance
//  8 + // quote_balance
//  8 + // quote_volume
//  8   // padding
// = 64
const_assert_eq!(size_of::<ClaimedSeat>(), CLAIMED_SEAT_SIZE);
const_assert_eq!(size_of::<ClaimedSeat>() % 8, 0);

impl ClaimedSeat {
    pub fn new_empty(trader: Pubkey) -> Self {
        ClaimedSeat {
            trader,
            ..Default::default()
        }
    }

    /// Get position size for perps markets.
    /// Positive = long, negative = short. Stored as i64 in the quote_volume field.
    pub fn get_position_size(&self) -> i64 {
        self.quote_volume.as_u64() as i64
    }

    /// Set position size for perps markets.
    pub fn set_position_size(&mut self, size: i64) {
        self.quote_volume = QuoteAtoms::new(size as u64);
    }

    /// Get quote cost basis for perps (total USDC spent to acquire position).
    /// Stored in the _padding field as u64 little-endian.
    pub fn get_quote_cost_basis(&self) -> u64 {
        u64::from_le_bytes(self._padding)
    }

    /// Set quote cost basis for perps.
    pub fn set_quote_cost_basis(&mut self, cost_basis: u64) {
        self._padding = cost_basis.to_le_bytes();
    }
}

#[cfg(feature = "certora")]
impl nondet::Nondet for ClaimedSeat {
    fn nondet() -> Self {
        ClaimedSeat {
            trader: nondet::nondet(),
            base_withdrawable_balance: BaseAtoms::new(nondet::nondet()),
            quote_withdrawable_balance: QuoteAtoms::new(nondet::nondet()),
            quote_volume: QuoteAtoms::new(nondet::nondet()),
            _padding: [0; 8],
        }
    }
}

impl Ord for ClaimedSeat {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.trader).cmp(&(other.trader))
    }
}

impl PartialOrd for ClaimedSeat {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for ClaimedSeat {
    fn eq(&self, other: &Self) -> bool {
        (self.trader) == (other.trader)
    }
}

impl Eq for ClaimedSeat {}

impl std::fmt::Display for ClaimedSeat {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.trader)
    }
}

#[test]
fn test_display() {
    let claimed_seat: ClaimedSeat = ClaimedSeat::new_empty(Pubkey::default());
    format!("{}", claimed_seat);
}
