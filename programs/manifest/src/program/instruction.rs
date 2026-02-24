use num_enum::TryFromPrimitive;
use shank::ShankInstruction;

/// Instructions available for the Manifest program
#[repr(u8)]
#[derive(TryFromPrimitive, Debug, Copy, Clone, ShankInstruction, PartialEq, Eq)]
#[rustfmt::skip]
pub enum ManifestInstruction {
    /// Create a perps market (quote-only, no base vault)
    #[account(0, writable, signer, name = "payer", desc = "Payer")]
    #[account(1, writable, name = "market", desc = "Market PDA, seeds are [b'market', &[base_mint_index], quote_mint]")]
    #[account(2, name = "system_program", desc = "System program")]
    #[account(3, name = "quote_mint", desc = "Quote mint (e.g. USDC)")]
    #[account(4, writable, name = "quote_vault", desc = "Quote vault PDA, seeds are [b'vault', market, quote_mint]")]
    #[account(5, name = "token_program", desc = "Token program")]
    #[account(6, name = "token_program_22", desc = "Token program 22")]
    #[account(7, name = "associated_token_program", desc = "Associated token program")]
    #[account(8, writable, name = "ephemeral_vault_ata", desc = "Ephemeral vault ATA for delegation")]
    #[account(9, name = "ephemeral_spl_token", desc = "Ephemeral SPL token program")]
    CreateMarket = 0,

    /// Allocate a seat
    #[account(0, writable, signer, name = "payer", desc = "Payer")]
    #[account(1, writable, name = "market", desc = "Account holding all market state")]
    #[account(2, name = "system_program", desc = "System program")]
    ClaimSeat = 1,

    /// Deposit quote tokens (USDC) into the market
    #[account(0, signer, name = "payer", desc = "Payer")]
    #[account(1, writable, name = "market", desc = "Account holding all market state")]
    #[account(2, writable, name = "trader_token", desc = "Trader quote token account")]
    #[account(3, writable, name = "vault", desc = "Quote vault PDA, seeds are [b'vault', market, quote_mint]")]
    #[account(4, name = "token_program", desc = "Token program(22)")]
    #[account(5, name = "quote_mint", desc = "Quote mint")]
    Deposit = 2,

    /// Withdraw quote tokens (USDC) from the market
    #[account(0, signer, name = "payer", desc = "Payer")]
    #[account(1, writable, name = "market", desc = "Account holding all market state")]
    #[account(2, writable, name = "trader_token", desc = "Trader quote token account")]
    #[account(3, writable, name = "vault", desc = "Quote vault PDA, seeds are [b'vault', market, quote_mint]")]
    #[account(4, name = "token_program", desc = "Token program(22)")]
    #[account(5, name = "quote_mint", desc = "Quote mint")]
    Withdraw = 3,

    /// Swap (perps): place an IOC order against the orderbook
    #[account(0, signer, name = "payer", desc = "Payer / trader")]
    #[account(1, writable, name = "market", desc = "Account holding all market state")]
    #[account(2, name = "system_program", desc = "System program")]
    #[account(3, optional, name = "session_token", desc = "Session token for delegated signing")]
    #[account(4, writable, name = "trader_quote", desc = "Trader quote token account")]
    #[account(5, writable, name = "quote_vault", desc = "Quote vault PDA, seeds are [b'vault', market, quote_mint]")]
    #[account(6, name = "token_program_quote", desc = "Token program(22) for quote")]
    #[account(7, optional, name = "quote_mint", desc = "Quote mint, required if Token22")]
    Swap = 4,

    /// Expand a market using lamport escrow from ephemeral-rollups-spl.
    #[account(0, signer, name = "payer", desc = "Payer (authority for escrow claim)")]
    #[account(1, writable, name = "market", desc = "Account holding all market state")]
    #[account(2, writable, name = "escrow", desc = "Lamport escrow PDA from ephemeral-rollups-spl")]
    #[account(3, name = "er_spl_program", desc = "Ephemeral-rollups-spl program")]
    Expand = 5,

    /// Batch update with multiple place orders and cancels.
    #[account(0, signer, name = "payer", desc = "Payer")]
    #[account(1, writable, name = "market", desc = "Account holding all market state")]
    #[account(2, name = "system_program", desc = "System program")]
    #[account(3, optional, name = "session_token", desc = "Session token for delegated signing")]
    #[account(4, optional, name = "quote_mint", desc = "Quote mint for global account")]
    #[account(5, optional, writable, name = "quote_global", desc = "Quote global account")]
    #[account(6, optional, name = "quote_global_vault", desc = "Quote global vault")]
    #[account(7, optional, name = "quote_market_vault", desc = "Quote market vault")]
    #[account(8, optional, name = "quote_token_program", desc = "Token program(22) for quote")]
    BatchUpdate = 6,

    /// Create global account for a given token.
    #[account(0, writable, signer, name = "payer", desc = "Payer")]
    #[account(1, writable, name = "global", desc = "Global account")]
    #[account(2, name = "system_program", desc = "System program")]
    #[account(3, name = "mint", desc = "Mint for this global account")]
    #[account(4, writable, name = "global_vault", desc = "Global vault")]
    #[account(5, name = "token_program", desc = "Token program(22)")]
    GlobalCreate = 7,

    /// Add a trader to the global account.
    #[account(0, writable, signer, name = "payer", desc = "Payer")]
    #[account(1, writable, name = "global", desc = "Global account")]
    #[account(2, name = "system_program", desc = "System program")]
    GlobalAddTrader = 8,

    /// Deposit into global account for a given token.
    #[account(0, writable, signer, name = "payer", desc = "Payer")]
    #[account(1, writable, name = "global", desc = "Global account")]
    #[account(2, name = "mint", desc = "Mint for this global account")]
    #[account(3, writable, name = "global_vault", desc = "Global vault")]
    #[account(4, writable, name = "trader_token", desc = "Trader token account")]
    #[account(5, name = "token_program", desc = "Token program(22)")]
    GlobalDeposit = 9,

    /// Withdraw from global account for a given token.
    #[account(0, writable, signer, name = "payer", desc = "Payer")]
    #[account(1, writable, name = "global", desc = "Global account")]
    #[account(2, name = "mint", desc = "Mint for this global account")]
    #[account(3, writable, name = "global_vault", desc = "Global vault")]
    #[account(4, writable, name = "trader_token", desc = "Trader token account")]
    #[account(5, name = "token_program", desc = "Token program(22)")]
    GlobalWithdraw = 10,

    /// Evict another trader from the global account.
    #[account(0, writable, signer, name = "payer", desc = "Payer")]
    #[account(1, writable, name = "global", desc = "Global account")]
    #[account(2, name = "mint", desc = "Mint for this global account")]
    #[account(3, writable, name = "global_vault", desc = "Global vault")]
    #[account(4, name = "trader_token", desc = "Trader token account")]
    #[account(5, name = "evictee_token", desc = "Evictee token account")]
    #[account(6, name = "token_program", desc = "Token program(22)")]
    GlobalEvict = 11,

    /// Removes an order from a market that cannot be filled. There are 3
    /// reasons. It is expired, the global trader got evicted, or the global
    /// trader no longer has deposited the funds to back the order. This
    /// function results in cleaner orderbooks which helps reduce variance and
    /// thus compute unit estimates for traders. It is incentivized by receiving
    /// gas prepayment deposits. This is not required for normal operation of
    /// market. It exists as a deterrent to unfillable and unmaintained global
    /// spam.
    #[account(0, writable, signer, name = "payer", desc = "Payer for this tx, receiver of rent deposit")]
    #[account(1, writable, name = "market", desc = "Account holding all market state")]
    #[account(2, name = "system_program", desc = "System program")]
    #[account(3, writable, name = "global", desc = "Global account")]
    GlobalClean = 12,

    
    /// SwapV2 (perps): swap with separate owner and payer
    #[account(0, signer, name = "payer", desc = "Payer")]
    #[account(1, signer, name = "owner", desc = "Owner / trader authority")]
    #[account(2, writable, name = "market", desc = "Account holding all market state")]
    #[account(3, name = "system_program", desc = "System program")]
    #[account(4, optional, name = "session_token", desc = "Session token for delegated signing")]
    #[account(5, writable, name = "trader_quote", desc = "Trader quote token account")]
    #[account(6, writable, name = "quote_vault", desc = "Quote vault PDA, seeds are [b'vault', market, quote_mint]")]
    #[account(7, name = "token_program_quote", desc = "Token program(22) for quote")]
    #[account(8, optional, name = "quote_mint", desc = "Quote mint, required if Token22")]
    SwapV2 = 13,

    /// Delegate market account to MagicBlock ephemeral rollups.
    #[account(0, writable, signer, name = "payer", desc = "Payer and market creator")]
    #[account(1, writable, name = "market", desc = "Market PDA to delegate")]
    #[account(2, name = "owner_program", desc = "Manifest program (owner of the PDA)")]
    #[account(3, name = "delegation_program", desc = "MagicBlock delegation program")]
    #[account(4, writable, name = "delegation_record", desc = "Delegation record PDA")]
    #[account(5, writable, name = "delegation_metadata", desc = "Delegation metadata PDA")]
    #[account(6, name = "system_program", desc = "System program")]
    #[account(7, writable, name = "buffer", desc = "Buffer account for delegation")]
    DelegateMarket = 14,

    /// Commit delegated market state back to mainnet.
    #[account(0, writable, signer, name = "payer", desc = "Payer")]
    #[account(1, writable, name = "market", desc = "Delegated market account")]
    #[account(2, name = "magic_program", desc = "MagicBlock magic program")]
    #[account(3, name = "magic_context", desc = "MagicBlock magic context")]
    CommitMarket = 15,

    /// Liquidate an underwater perps position.
    #[account(0, writable, signer, name = "liquidator", desc = "Liquidator")]
    #[account(1, writable, name = "market", desc = "Perps market account")]
    #[account(2, name = "system_program", desc = "System program")]
    Liquidate = 16,

    /// Crank funding rate using oracle price.
    #[account(0, writable, signer, name = "payer", desc = "Payer / cranker")]
    #[account(1, writable, name = "market", desc = "Perps market account")]
    #[account(2, name = "pyth_price_feed", desc = "Pyth price feed account")]
    CrankFunding = 17,

    /// Release a claimed seat, freeing the block back to the free list.
    /// Trader must have zero balances and no open position.
    #[account(0, writable, signer, name = "payer", desc = "Payer / trader releasing seat")]
    #[account(1, writable, name = "market", desc = "Account holding all market state")]
    #[account(2, name = "system_program", desc = "System program")]
    ReleaseSeat = 18,
}

impl ManifestInstruction {
    pub fn to_vec(&self) -> Vec<u8> {
        vec![*self as u8]
    }
}

#[test]
fn test_instruction_serialization() {
    let num_instructions: u8 = 18;
    for i in 0..=255 {
        let instruction: ManifestInstruction = match ManifestInstruction::try_from(i) {
            Ok(j) => {
                assert!(i <= num_instructions);
                j
            }
            Err(_) => {
                assert!(i > num_instructions);
                continue;
            }
        };
        assert_eq!(instruction as u8, i);
    }
}

#[test]
fn test_to_vec() {
    let create_market_ix = ManifestInstruction::CreateMarket;
    let vec = create_market_ix.to_vec();
    assert_eq!(*vec.first().unwrap(), 0);
}
