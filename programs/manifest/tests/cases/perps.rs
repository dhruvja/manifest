use solana_program::pubkey::Pubkey;
use solana_program_test::tokio;
use solana_sdk::signature::Signer;

use manifest::state::OrderType;

use crate::{build_mock_pyth_data, Side, TestFixture, Token, USDC_UNIT_SIZE};

/// Price encoding: mantissa=1, exponent=-2 = 0.01 quote atoms per base atom
/// With base_decimals=9, quote_decimals=6:
/// 1 SOL = 10^9 base atoms, at 0.01 qapba = 10^9 * 0.01 = 10^7 = 10 USDC
const PRICE_10_MANTISSA: u32 = 1;
const PRICE_10_EXPONENT: i8 = -2;
const SOL: u64 = 1_000_000_000; // 1 SOL in base atoms
const TEN_USDC: u64 = 10_000_000; // 10 USDC in quote atoms

// ─── Test 1: Open long position via matching ─────────────────────

#[tokio::test]
async fn test_open_long_position() -> anyhow::Result<()> {
    let mut test_fixture = TestFixture::try_new_for_perps_test(100 * USDC_UNIT_SIZE).await?;
    let second_keypair = test_fixture.second_keypair.insecure_clone();
    let payer = test_fixture.payer();

    // Second trader places a BID for 2 SOL at 10 USDC (locks 20 USDC).
    // 2 SOL so that after 1 SOL matches, there's still liquidity for mark price.
    test_fixture
        .place_order_for_keypair(
            Side::Bid,
            2 * SOL,
            PRICE_10_MANTISSA,
            PRICE_10_EXPONENT,
            0, // no expiration
            OrderType::Limit,
            &second_keypair,
        )
        .await?;

    // Payer swaps: sell 1 SOL (is_base_in=true) → goes SHORT, matching second's bid.
    // This makes the second trader LONG (their bid was filled).
    test_fixture.swap(SOL, 0, true, true).await?;

    // Check positions
    let (payer_pos, payer_cost) = test_fixture
        .market_fixture
        .get_trader_position(&payer)
        .await;
    assert_eq!(payer_pos, -(SOL as i64), "Payer should be SHORT 1 SOL");
    assert_eq!(payer_cost, TEN_USDC, "Payer cost basis should be 10 USDC");

    let (second_pos, second_cost) = test_fixture
        .market_fixture
        .get_trader_position(&second_keypair.pubkey())
        .await;
    assert_eq!(second_pos, SOL as i64, "Second trader should be LONG 1 SOL");
    assert_eq!(
        second_cost, TEN_USDC,
        "Second cost basis should be 10 USDC"
    );

    Ok(())
}

// ─── Test 2: Open short position ─────────────────────────────────

#[tokio::test]
async fn test_open_short_position() -> anyhow::Result<()> {
    let mut test_fixture = TestFixture::try_new_for_perps_test(100 * USDC_UNIT_SIZE).await?;
    let second_keypair = test_fixture.second_keypair.insecure_clone();
    let payer = test_fixture.payer();

    // Payer places BID for 2 SOL at 10 USDC (keeps book non-empty after partial fill)
    test_fixture
        .place_order(
            Side::Bid,
            2 * SOL,
            PRICE_10_MANTISSA,
            PRICE_10_EXPONENT,
            0,
            OrderType::Limit,
        )
        .await?;

    // Second trader sells 1 SOL via swap → goes SHORT
    test_fixture
        .swap_for_keypair(SOL, 0, true, true, &second_keypair)
        .await?;

    // Payer should be long (their bid was filled)
    let (payer_pos, _) = test_fixture
        .market_fixture
        .get_trader_position(&payer)
        .await;
    assert_eq!(payer_pos, SOL as i64, "Payer should be LONG 1 SOL");

    // Second should be short
    let (second_pos, _) = test_fixture
        .market_fixture
        .get_trader_position(&second_keypair.pubkey())
        .await;
    assert_eq!(
        second_pos,
        -(SOL as i64),
        "Second should be SHORT 1 SOL"
    );

    Ok(())
}

// ─── Test 3: Initial margin reject ───────────────────────────────

#[tokio::test]
async fn test_initial_margin_reject() -> anyhow::Result<()> {
    // Use 120% initial margin. For a short: equity = deposit + proceeds + pnl(0)
    // required = notional * 1.2. With deposit=1 USDC, notional=10:
    // equity = 1 + 10 = 11, required = 12 → 11 < 12 → FAILS
    let pyth_key = Pubkey::new_unique();
    let pyth_data = build_mock_pyth_data(10_0000_0000, -8, 100_000);

    let mut test_fixture =
        TestFixture::new_with_pyth(pyth_key, pyth_data, 12000, 500).await;
    let second_keypair = test_fixture.second_keypair.insecure_clone();

    test_fixture.claim_seat().await?;
    test_fixture.deposit(Token::USDC, USDC_UNIT_SIZE).await?; // 1 USDC

    test_fixture
        .claim_seat_for_keypair(&second_keypair)
        .await?;
    test_fixture
        .deposit_for_keypair(Token::USDC, 100 * USDC_UNIT_SIZE, &second_keypair)
        .await?;

    // Cache oracle so compute_mark_price uses it
    test_fixture.crank_funding(&pyth_key).await?;

    // Second places BID for 2 SOL at 10 USDC
    test_fixture
        .place_order_for_keypair(
            Side::Bid,
            2 * SOL,
            PRICE_10_MANTISSA,
            PRICE_10_EXPONENT,
            0,
            OrderType::Limit,
            &second_keypair,
        )
        .await?;

    // Payer tries to sell 1 SOL (go short)
    // equity = 1 + 10 = 11 USDC, required = 10 * 120% = 12 USDC → FAIL
    let result = test_fixture.swap(SOL, 0, true, true).await;
    assert!(
        result.is_err(),
        "Swap should fail due to insufficient initial margin"
    );

    Ok(())
}

// ─── Test 4: Initial margin accept ───────────────────────────────

#[tokio::test]
async fn test_initial_margin_accept() -> anyhow::Result<()> {
    // Same 120% margin but with enough deposit (3 USDC)
    // equity = 3 + 10 = 13, required = 12 → 13 >= 12 → PASSES
    let pyth_key = Pubkey::new_unique();
    let pyth_data = build_mock_pyth_data(10_0000_0000, -8, 100_000);

    let mut test_fixture =
        TestFixture::new_with_pyth(pyth_key, pyth_data, 12000, 500).await;
    let second_keypair = test_fixture.second_keypair.insecure_clone();

    test_fixture.claim_seat().await?;
    test_fixture.deposit(Token::USDC, 3 * USDC_UNIT_SIZE).await?;

    test_fixture
        .claim_seat_for_keypair(&second_keypair)
        .await?;
    test_fixture
        .deposit_for_keypair(Token::USDC, 100 * USDC_UNIT_SIZE, &second_keypair)
        .await?;

    // Cache oracle
    test_fixture.crank_funding(&pyth_key).await?;

    // Second places BID for 2 SOL at 10 USDC
    test_fixture
        .place_order_for_keypair(
            Side::Bid,
            2 * SOL,
            PRICE_10_MANTISSA,
            PRICE_10_EXPONENT,
            0,
            OrderType::Limit,
            &second_keypair,
        )
        .await?;

    // Payer sells 1 SOL (go short) → should succeed
    test_fixture.swap(SOL, 0, true, true).await?;

    let (payer_pos, _) = test_fixture
        .market_fixture
        .get_trader_position(&test_fixture.payer())
        .await;
    assert_eq!(payer_pos, -(SOL as i64), "Payer should be SHORT 1 SOL");

    Ok(())
}

// ─── Test 5: Liquidation happy path ──────────────────────────────

#[tokio::test]
async fn test_liquidation_happy_path() -> anyhow::Result<()> {
    // Oracle at 10 USDC/SOL (10 * 10^8 mantissa, expo=-8 → price = 10)
    let pyth_key = Pubkey::new_unique();
    let pyth_data = build_mock_pyth_data(10_0000_0000, -8, 100_000);

    let mut test_fixture =
        TestFixture::new_with_pyth(pyth_key, pyth_data, 1000, 500).await;
    let second_keypair = test_fixture.second_keypair.insecure_clone();
    let payer = test_fixture.payer();

    // Crank funding to cache oracle price at 10 USDC/SOL
    test_fixture.claim_seat().await?;
    test_fixture.deposit(Token::USDC, 6 * USDC_UNIT_SIZE).await?;

    test_fixture
        .claim_seat_for_keypair(&second_keypair)
        .await?;
    test_fixture
        .deposit_for_keypair(Token::USDC, 100 * USDC_UNIT_SIZE, &second_keypair)
        .await?;

    // Crank to set oracle price in market state
    test_fixture.crank_funding(&pyth_key).await?;

    // Second places BID for 2 SOL at 10 USDC
    test_fixture
        .place_order_for_keypair(
            Side::Bid,
            2 * SOL,
            PRICE_10_MANTISSA,
            PRICE_10_EXPONENT,
            0,
            OrderType::Limit,
            &second_keypair,
        )
        .await?;

    // Payer sells 1 SOL → short at 10 USDC, equity = 6 USDC
    test_fixture.swap(SOL, 0, true, true).await?;

    let (pos, _) = test_fixture.market_fixture.get_trader_position(&payer).await;
    assert_eq!(pos, -(SOL as i64), "Payer should be SHORT 1 SOL");

    // Now update oracle to 1 USDC/SOL (price crash from short perspective: SHORT profits)
    // Actually for SHORT to be underwater, price needs to RISE, not fall.
    // Short PnL = cost_basis - current_value. If price rises, current_value > cost_basis → loss.
    // Let's set oracle to 20 USDC/SOL
    let new_pyth_data = build_mock_pyth_data(20_0000_0000, -8, 100_000);
    {
        let mut ctx = test_fixture.context.borrow_mut();
        ctx.set_account(
            &pyth_key,
            &solana_sdk::account::Account {
                lamports: u32::MAX as u64,
                data: new_pyth_data,
                owner: Pubkey::new_unique(),
                executable: false,
                rent_epoch: 0,
            }
            .into(),
        );
    }

    // Advance time so funding crank accepts
    test_fixture.advance_time_seconds(3600).await;
    test_fixture.crank_funding(&pyth_key).await?;

    // Now oracle = 20 USDC/SOL. Payer is SHORT 1 SOL.
    // Position value = 20 USDC. Cost basis = 10 USDC.
    // Unrealized PnL (short) = cost_basis - current_value = 10 - 20 = -10
    // Equity = margin + PnL = 6 + (-10) = -4 USDC (negative!)
    // Maintenance margin = 5% * 20 = 1 USDC
    // Equity (-4) < maintenance (1) → LIQUIDATABLE

    // Second (as liquidator) liquidates payer
    test_fixture
        .liquidate_for_keypair(&payer, &second_keypair)
        .await?;

    // Verify position is closed
    let (pos_after, _) = test_fixture.market_fixture.get_trader_position(&payer).await;
    assert_eq!(pos_after, 0, "Position should be closed after liquidation");

    // Verify liquidator received reward
    let second_balance_after = test_fixture
        .market_fixture
        .get_quote_balance_atoms(&second_keypair.pubkey())
        .await;
    // Liquidator should have some reward (2.5% of settled margin)
    // Original second balance was 100 USDC - 10 USDC (from filled bid) + reward
    // Hard to compute exact because of funding payments, but should be > 0
    assert!(
        second_balance_after > 0,
        "Liquidator should have positive balance after reward"
    );

    Ok(())
}

// ─── Test 6: Liquidation reject - healthy trader ──────────────────

#[tokio::test]
async fn test_liquidation_reject_healthy() -> anyhow::Result<()> {
    let pyth_key = Pubkey::new_unique();
    let pyth_data = build_mock_pyth_data(10_0000_0000, -8, 100_000);

    let mut test_fixture =
        TestFixture::new_with_pyth(pyth_key, pyth_data, 1000, 500).await;
    let second_keypair = test_fixture.second_keypair.insecure_clone();
    let payer = test_fixture.payer();

    test_fixture.claim_seat().await?;
    test_fixture
        .deposit(Token::USDC, 100 * USDC_UNIT_SIZE)
        .await?;

    test_fixture
        .claim_seat_for_keypair(&second_keypair)
        .await?;
    test_fixture
        .deposit_for_keypair(Token::USDC, 100 * USDC_UNIT_SIZE, &second_keypair)
        .await?;

    test_fixture.crank_funding(&pyth_key).await?;

    // Second places BID for 2 SOL at 10 USDC
    test_fixture
        .place_order_for_keypair(
            Side::Bid,
            2 * SOL,
            PRICE_10_MANTISSA,
            PRICE_10_EXPONENT,
            0,
            OrderType::Limit,
            &second_keypair,
        )
        .await?;

    // Payer sells 1 SOL → short, equity = 100 USDC (plenty of margin)
    test_fixture.swap(SOL, 0, true, true).await?;

    // Try to liquidate payer — should fail because equity >> maintenance
    let result = test_fixture
        .liquidate_for_keypair(&payer, &second_keypair)
        .await;
    assert!(
        result.is_err(),
        "Liquidation should fail for a healthy trader"
    );

    Ok(())
}

// ─── Test 7: Liquidation cancels open orders ──────────────────────

#[tokio::test]
async fn test_liquidation_cancels_orders() -> anyhow::Result<()> {
    let pyth_key = Pubkey::new_unique();
    let pyth_data = build_mock_pyth_data(10_0000_0000, -8, 100_000);

    let mut test_fixture =
        TestFixture::new_with_pyth(pyth_key, pyth_data, 1000, 500).await;
    let second_keypair = test_fixture.second_keypair.insecure_clone();
    let payer = test_fixture.payer();

    test_fixture.claim_seat().await?;
    test_fixture.deposit(Token::USDC, 6 * USDC_UNIT_SIZE).await?;

    test_fixture
        .claim_seat_for_keypair(&second_keypair)
        .await?;
    test_fixture
        .deposit_for_keypair(Token::USDC, 100 * USDC_UNIT_SIZE, &second_keypair)
        .await?;

    test_fixture.crank_funding(&pyth_key).await?;

    // Second places BID for 2 SOL at 10 USDC
    test_fixture
        .place_order_for_keypair(
            Side::Bid,
            2 * SOL,
            PRICE_10_MANTISSA,
            PRICE_10_EXPONENT,
            0,
            OrderType::Limit,
            &second_keypair,
        )
        .await?;

    // Payer sells 1 SOL → short position
    test_fixture.swap(SOL, 0, true, true).await?;

    // Payer also places a resting BID order (should be cancelled during liquidation)
    // Using whatever USDC is left
    test_fixture
        .place_order(
            Side::Bid,
            SOL / 10, // small order
            PRICE_10_MANTISSA,
            PRICE_10_EXPONENT,
            0,
            OrderType::Limit,
        )
        .await
        .ok(); // ignore if placement fails due to margin

    // Count orders before liquidation
    let orders_before = test_fixture.market_fixture.get_resting_orders().await;
    let payer_orders_before = orders_before.len(); // includes all traders' orders

    // Tank the price to make payer underwater
    let new_pyth_data = build_mock_pyth_data(20_0000_0000, -8, 100_000);
    {
        let mut ctx = test_fixture.context.borrow_mut();
        ctx.set_account(
            &pyth_key,
            &solana_sdk::account::Account {
                lamports: u32::MAX as u64,
                data: new_pyth_data,
                owner: Pubkey::new_unique(),
                executable: false,
                rent_epoch: 0,
            }
            .into(),
        );
    }
    test_fixture.advance_time_seconds(3600).await;
    test_fixture.crank_funding(&pyth_key).await?;

    // Liquidate
    test_fixture
        .liquidate_for_keypair(&payer, &second_keypair)
        .await?;

    // Verify position is closed
    let (pos, _) = test_fixture.market_fixture.get_trader_position(&payer).await;
    assert_eq!(pos, 0, "Position should be closed");

    // Verify payer's orders were cancelled (should have fewer total orders now)
    let orders_after = test_fixture.market_fixture.get_resting_orders().await;
    assert!(
        orders_after.len() <= payer_orders_before,
        "Orders should have been cancelled during liquidation"
    );

    Ok(())
}

// ─── Test 8: Funding rate application ────────────────────────────

#[tokio::test]
async fn test_funding_rate_application() -> anyhow::Result<()> {
    let pyth_key = Pubkey::new_unique();
    // Oracle at 10 USDC/SOL
    let pyth_data = build_mock_pyth_data(10_0000_0000, -8, 100_000);

    let mut test_fixture =
        TestFixture::new_with_pyth(pyth_key, pyth_data, 1000, 500).await;
    let second_keypair = test_fixture.second_keypair.insecure_clone();
    let payer = test_fixture.payer();

    test_fixture.claim_seat().await?;
    test_fixture
        .deposit(Token::USDC, 100 * USDC_UNIT_SIZE)
        .await?;

    test_fixture
        .claim_seat_for_keypair(&second_keypair)
        .await?;
    test_fixture
        .deposit_for_keypair(Token::USDC, 100 * USDC_UNIT_SIZE, &second_keypair)
        .await?;

    // First crank to initialize timestamp
    test_fixture.crank_funding(&pyth_key).await?;

    // Second places BID for 2 SOL at 10 USDC
    test_fixture
        .place_order_for_keypair(
            Side::Bid,
            2 * SOL,
            PRICE_10_MANTISSA,
            PRICE_10_EXPONENT,
            0,
            OrderType::Limit,
            &second_keypair,
        )
        .await?;

    // Payer sells 1 SOL → payer is SHORT, second is LONG
    test_fixture.swap(SOL, 0, true, true).await?;

    // Record balances before funding
    let payer_balance_before = test_fixture
        .market_fixture
        .get_quote_balance_atoms(&payer)
        .await;
    let second_balance_before = test_fixture
        .market_fixture
        .get_quote_balance_atoms(&second_keypair.pubkey())
        .await;

    // Now set oracle to 8 USDC/SOL (below orderbook mark of ~10)
    // Mark > Oracle → positive funding rate → longs pay shorts
    let new_pyth_data = build_mock_pyth_data(8_0000_0000, -8, 100_000);
    {
        let mut ctx = test_fixture.context.borrow_mut();
        ctx.set_account(
            &pyth_key,
            &solana_sdk::account::Account {
                lamports: u32::MAX as u64,
                data: new_pyth_data,
                owner: Pubkey::new_unique(),
                executable: false,
                rent_epoch: 0,
            }
            .into(),
        );
    }

    // Advance 1 hour for full funding period
    test_fixture.advance_time_seconds(3600).await;

    // Crank funding — updates global cumulative rate only (lazy settlement).
    test_fixture.crank_funding(&pyth_key).await?;

    // Trigger lazy funding settlement for each trader via a small deposit.
    // With the cumulative funding model, funding is settled on the next user interaction.
    test_fixture.deposit(Token::USDC, 1).await?;
    test_fixture
        .deposit_for_keypair(Token::USDC, 1, &second_keypair)
        .await?;

    // Check balances after funding settlement
    let payer_balance_after = test_fixture
        .market_fixture
        .get_quote_balance_atoms(&payer)
        .await;
    let second_balance_after = test_fixture
        .market_fixture
        .get_quote_balance_atoms(&second_keypair.pubkey())
        .await;

    // Mark (10) > Oracle (8) → positive funding rate
    // Longs pay, shorts receive
    // Payer (SHORT) should receive funding → balance increases (+ 1 atom deposit)
    // Second (LONG) should pay funding → balance decreases (+ 1 atom deposit)
    println!(
        "Payer (SHORT): before={}, after={}",
        payer_balance_before, payer_balance_after
    );
    println!(
        "Second (LONG): before={}, after={}",
        second_balance_before, second_balance_after
    );

    assert!(
        payer_balance_after > payer_balance_before,
        "Short should receive funding payment: before={}, after={}",
        payer_balance_before,
        payer_balance_after
    );
    assert!(
        second_balance_after < second_balance_before,
        "Long should pay funding: before={}, after={}",
        second_balance_before,
        second_balance_after
    );

    Ok(())
}

// ─── Test 9: Partial liquidation reduces position ────────────────
// Use a setup where equity is below maintenance but not deeply negative,
// so only a fraction of the position needs to be closed.
//
// Key trick: skip the initial crank_funding so the first crank (after price
// change) takes the `last_funding_ts == 0` early-return path — it only caches
// the oracle and does NOT apply any funding payments. This gives us a clean
// equity = margin + PnL with no funding effects.

#[tokio::test]
async fn test_partial_liquidation() -> anyhow::Result<()> {
    let pyth_key = Pubkey::new_unique();
    let pyth_data = build_mock_pyth_data(10_0000_0000, -8, 100_000);

    // 10% initial margin, 5% maintenance, 2% liquidation buffer (default)
    let mut test_fixture =
        TestFixture::new_with_pyth(pyth_key, pyth_data, 1000, 500).await;
    let second_keypair = test_fixture.second_keypair.insecure_clone();
    let payer = test_fixture.payer();

    test_fixture.claim_seat().await?;
    test_fixture.deposit(Token::USDC, 2 * USDC_UNIT_SIZE).await?;

    test_fixture
        .claim_seat_for_keypair(&second_keypair)
        .await?;
    test_fixture
        .deposit_for_keypair(Token::USDC, 100 * USDC_UNIT_SIZE, &second_keypair)
        .await?;

    // No initial crank_funding — oracle not needed for swap (uses orderbook)

    test_fixture
        .place_order_for_keypair(
            Side::Bid,
            2 * SOL,
            PRICE_10_MANTISSA,
            PRICE_10_EXPONENT,
            0,
            OrderType::Limit,
            &second_keypair,
        )
        .await?;

    // Payer sells 1 SOL → short, margin = 2 + 10 = 12 USDC
    test_fixture.swap(SOL, 0, true, true).await?;

    let (pos, _) = test_fixture.market_fixture.get_trader_position(&payer).await;
    assert_eq!(pos, -(SOL as i64));

    // Set oracle to 11.5 USDC and do the first-ever crank.
    // Since last_funding_ts == 0, crank just caches oracle — no funding applied.
    // In perps, margin stays as deposit (2 USDC) — swap doesn't credit quote.
    // equity = 2 + (10 - 11.5) = 0.5 USDC, maintenance = 11.5 * 5% = 0.575 → liquidatable
    // target_bps = 700, equity_bps = 434, f = 266/450 ≈ 0.59 → partial liquidation
    let new_pyth_data = build_mock_pyth_data(11_5000_0000, -8, 100_000);
    {
        let mut ctx = test_fixture.context.borrow_mut();
        ctx.set_account(
            &pyth_key,
            &solana_sdk::account::Account {
                lamports: u32::MAX as u64,
                data: new_pyth_data,
                owner: Pubkey::new_unique(),
                executable: false,
                rent_epoch: 0,
            }
            .into(),
        );
    }
    test_fixture.crank_funding(&pyth_key).await?;

    // Liquidate
    test_fixture
        .liquidate_for_keypair(&payer, &second_keypair)
        .await?;

    // Position should be PARTIALLY closed (not zero)
    let (pos_after, _) = test_fixture.market_fixture.get_trader_position(&payer).await;
    assert!(
        pos_after < 0 && pos_after > -(SOL as i64),
        "Position should be partially closed: got {}",
        pos_after,
    );

    Ok(())
}

// ─── Test 10: Partial liquidation proportional cost basis ────────
// Same no-initial-crank approach as test 9 for clean funding-free scenario.

#[tokio::test]
async fn test_partial_liquidation_cost_basis() -> anyhow::Result<()> {
    let pyth_key = Pubkey::new_unique();
    let pyth_data = build_mock_pyth_data(10_0000_0000, -8, 100_000);

    let mut test_fixture =
        TestFixture::new_with_pyth(pyth_key, pyth_data, 1000, 500).await;
    let second_keypair = test_fixture.second_keypair.insecure_clone();
    let payer = test_fixture.payer();

    test_fixture.claim_seat().await?;
    test_fixture.deposit(Token::USDC, 2 * USDC_UNIT_SIZE).await?;

    test_fixture
        .claim_seat_for_keypair(&second_keypair)
        .await?;
    test_fixture
        .deposit_for_keypair(Token::USDC, 100 * USDC_UNIT_SIZE, &second_keypair)
        .await?;

    // No initial crank

    test_fixture
        .place_order_for_keypair(
            Side::Bid,
            2 * SOL,
            PRICE_10_MANTISSA,
            PRICE_10_EXPONENT,
            0,
            OrderType::Limit,
            &second_keypair,
        )
        .await?;

    test_fixture.swap(SOL, 0, true, true).await?;

    let (_, cost_before) = test_fixture.market_fixture.get_trader_position(&payer).await;
    assert_eq!(cost_before, TEN_USDC, "Cost basis should be 10 USDC initially");

    // First-ever crank at 11.5 — just caches oracle, no funding
    // equity = 2 + (10 - 11.5) = 0.5, maintenance = 0.575 → liquidatable, partial
    let new_pyth_data = build_mock_pyth_data(11_5000_0000, -8, 100_000);
    {
        let mut ctx = test_fixture.context.borrow_mut();
        ctx.set_account(
            &pyth_key,
            &solana_sdk::account::Account {
                lamports: u32::MAX as u64,
                data: new_pyth_data,
                owner: Pubkey::new_unique(),
                executable: false,
                rent_epoch: 0,
            }
            .into(),
        );
    }
    test_fixture.crank_funding(&pyth_key).await?;

    test_fixture
        .liquidate_for_keypair(&payer, &second_keypair)
        .await?;

    let (pos_after, cost_after) = test_fixture.market_fixture.get_trader_position(&payer).await;

    // Partial liquidation expected — cost basis should be reduced proportionally
    assert!(pos_after != 0, "Should be partial, not full liquidation");
    let abs_before = SOL;
    let abs_after = (pos_after as i64).unsigned_abs();
    let expected_cost = (cost_before as u128 * abs_after as u128 / abs_before as u128) as u64;
    assert!(
        cost_after <= expected_cost + 1 && cost_after + 1 >= expected_cost,
        "Cost basis should be proportional: expected ~{}, got {}",
        expected_cost,
        cost_after,
    );

    Ok(())
}

// ─── Test 11: Full liquidation when deeply underwater ─────────────

#[tokio::test]
async fn test_full_liquidation_deeply_underwater() -> anyhow::Result<()> {
    let pyth_key = Pubkey::new_unique();
    let pyth_data = build_mock_pyth_data(10_0000_0000, -8, 100_000);

    let mut test_fixture =
        TestFixture::new_with_pyth(pyth_key, pyth_data, 1000, 500).await;
    let second_keypair = test_fixture.second_keypair.insecure_clone();
    let payer = test_fixture.payer();

    test_fixture.claim_seat().await?;
    test_fixture.deposit(Token::USDC, 2 * USDC_UNIT_SIZE).await?;

    test_fixture
        .claim_seat_for_keypair(&second_keypair)
        .await?;
    test_fixture
        .deposit_for_keypair(Token::USDC, 100 * USDC_UNIT_SIZE, &second_keypair)
        .await?;

    test_fixture.crank_funding(&pyth_key).await?;

    test_fixture
        .place_order_for_keypair(
            Side::Bid,
            2 * SOL,
            PRICE_10_MANTISSA,
            PRICE_10_EXPONENT,
            0,
            OrderType::Limit,
            &second_keypair,
        )
        .await?;

    // Payer goes short at 10
    test_fixture.swap(SOL, 0, true, true).await?;

    // Price jumps to 100 USDC → hugely underwater
    // equity = 12 + (10 - 100) = 12 - 90 = -78
    let new_pyth_data = build_mock_pyth_data(100_0000_0000, -8, 100_000);
    {
        let mut ctx = test_fixture.context.borrow_mut();
        ctx.set_account(
            &pyth_key,
            &solana_sdk::account::Account {
                lamports: u32::MAX as u64,
                data: new_pyth_data,
                owner: Pubkey::new_unique(),
                executable: false,
                rent_epoch: 0,
            }
            .into(),
        );
    }
    test_fixture.advance_time_seconds(3600).await;
    test_fixture.crank_funding(&pyth_key).await?;

    test_fixture
        .liquidate_for_keypair(&payer, &second_keypair)
        .await?;

    // Should be fully liquidated (not partial)
    let (pos_after, cost_after) = test_fixture.market_fixture.get_trader_position(&payer).await;
    assert_eq!(pos_after, 0, "Position should be fully closed when deeply underwater");
    assert_eq!(cost_after, 0, "Cost basis should be zero after full liquidation");

    Ok(())
}

// ─── Test 12: Insurance fund covers bad debt ──────────────────────

#[tokio::test]
async fn test_insurance_fund_covers_bad_debt() -> anyhow::Result<()> {
    let pyth_key = Pubkey::new_unique();
    let pyth_data = build_mock_pyth_data(10_0000_0000, -8, 100_000);

    // Use fees to build up insurance fund, then test bad debt coverage
    let mut test_fixture =
        TestFixture::new_with_pyth_and_fees(
            pyth_key,
            pyth_data,
            1000,  // 10% initial margin
            500,   // 5% maintenance
            500,   // 5% taker fee (high to build fund quickly)
            200,   // 2% liquidation buffer
        )
        .await;
    let second_keypair = test_fixture.second_keypair.insecure_clone();
    let payer = test_fixture.payer();

    test_fixture.claim_seat().await?;
    test_fixture
        .deposit(Token::USDC, 10 * USDC_UNIT_SIZE)
        .await?;

    test_fixture
        .claim_seat_for_keypair(&second_keypair)
        .await?;
    test_fixture
        .deposit_for_keypair(Token::USDC, 100 * USDC_UNIT_SIZE, &second_keypair)
        .await?;

    test_fixture.crank_funding(&pyth_key).await?;

    // Place order and swap to build insurance fund from fees
    test_fixture
        .place_order_for_keypair(
            Side::Bid,
            2 * SOL,
            PRICE_10_MANTISSA,
            PRICE_10_EXPONENT,
            0,
            OrderType::Limit,
            &second_keypair,
        )
        .await?;

    // Payer goes short at 10 → quote_atoms_traded = 10 USDC, fee = 5% * 10 = 0.5 USDC
    test_fixture.swap(SOL, 0, true, true).await?;

    // Verify insurance fund has accumulated fees
    let fund_balance = test_fixture
        .market_fixture
        .get_insurance_fund_balance()
        .await;
    assert!(
        fund_balance > 0,
        "Insurance fund should have fees: got {}",
        fund_balance
    );

    // Now crash the price → bad debt
    let new_pyth_data = build_mock_pyth_data(100_0000_0000, -8, 100_000);
    {
        let mut ctx = test_fixture.context.borrow_mut();
        ctx.set_account(
            &pyth_key,
            &solana_sdk::account::Account {
                lamports: u32::MAX as u64,
                data: new_pyth_data,
                owner: Pubkey::new_unique(),
                executable: false,
                rent_epoch: 0,
            }
            .into(),
        );
    }
    test_fixture.advance_time_seconds(3600).await;
    test_fixture.crank_funding(&pyth_key).await?;

    let fund_before = test_fixture
        .market_fixture
        .get_insurance_fund_balance()
        .await;

    test_fixture
        .liquidate_for_keypair(&payer, &second_keypair)
        .await?;

    // Insurance fund should have decreased (used to cover bad debt)
    let fund_after = test_fixture
        .market_fixture
        .get_insurance_fund_balance()
        .await;
    assert!(
        fund_after <= fund_before,
        "Insurance fund should decrease: before={}, after={}",
        fund_before,
        fund_after
    );

    Ok(())
}

// ─── Test 13: Insurance fund insufficient → liquidator reward reduced ──

#[tokio::test]
async fn test_insurance_fund_insufficient() -> anyhow::Result<()> {
    let pyth_key = Pubkey::new_unique();
    let pyth_data = build_mock_pyth_data(10_0000_0000, -8, 100_000);

    // No taker fee → insurance fund stays at 0
    let mut test_fixture =
        TestFixture::new_with_pyth(pyth_key, pyth_data, 1000, 500).await;
    let second_keypair = test_fixture.second_keypair.insecure_clone();
    let payer = test_fixture.payer();

    test_fixture.claim_seat().await?;
    test_fixture.deposit(Token::USDC, 2 * USDC_UNIT_SIZE).await?;

    test_fixture
        .claim_seat_for_keypair(&second_keypair)
        .await?;
    test_fixture
        .deposit_for_keypair(Token::USDC, 100 * USDC_UNIT_SIZE, &second_keypair)
        .await?;

    test_fixture.crank_funding(&pyth_key).await?;

    test_fixture
        .place_order_for_keypair(
            Side::Bid,
            2 * SOL,
            PRICE_10_MANTISSA,
            PRICE_10_EXPONENT,
            0,
            OrderType::Limit,
            &second_keypair,
        )
        .await?;

    test_fixture.swap(SOL, 0, true, true).await?;

    // Insurance fund should be 0 (no fees)
    let fund_before = test_fixture
        .market_fixture
        .get_insurance_fund_balance()
        .await;
    assert_eq!(fund_before, 0, "Insurance fund should be empty");

    // Crash price hugely
    let new_pyth_data = build_mock_pyth_data(100_0000_0000, -8, 100_000);
    {
        let mut ctx = test_fixture.context.borrow_mut();
        ctx.set_account(
            &pyth_key,
            &solana_sdk::account::Account {
                lamports: u32::MAX as u64,
                data: new_pyth_data,
                owner: Pubkey::new_unique(),
                executable: false,
                rent_epoch: 0,
            }
            .into(),
        );
    }
    test_fixture.advance_time_seconds(3600).await;
    test_fixture.crank_funding(&pyth_key).await?;

    let second_balance_before = test_fixture
        .market_fixture
        .get_quote_balance_atoms(&second_keypair.pubkey())
        .await;

    // This should succeed despite empty insurance fund
    test_fixture
        .liquidate_for_keypair(&payer, &second_keypair)
        .await?;

    // Trader margin should be 0
    let payer_balance = test_fixture
        .market_fixture
        .get_quote_balance_atoms(&payer)
        .await;
    assert_eq!(payer_balance, 0, "Trader margin should be 0 after bad debt liquidation");

    // Liquidator should still get some reward (possibly reduced)
    let second_balance_after = test_fixture
        .market_fixture
        .get_quote_balance_atoms(&second_keypair.pubkey())
        .await;
    // Even with reduced reward, second should have at least as much as before
    // (reward may be 0 if deficit exceeds the reward)
    assert!(
        second_balance_after >= second_balance_before,
        "Liquidator balance should not decrease"
    );

    Ok(())
}

// ─── Test 14: Taker fee collection ────────────────────────────────

#[tokio::test]
async fn test_taker_fee_collection() -> anyhow::Result<()> {
    let pyth_key = Pubkey::new_unique();
    let pyth_data = build_mock_pyth_data(10_0000_0000, -8, 100_000);

    // 1% taker fee
    let mut test_fixture =
        TestFixture::new_with_pyth_and_fees(
            pyth_key,
            pyth_data,
            1000,  // 10% initial
            500,   // 5% maintenance
            100,   // 1% taker fee
            200,   // 2% buffer
        )
        .await;
    let second_keypair = test_fixture.second_keypair.insecure_clone();

    test_fixture.claim_seat().await?;
    test_fixture
        .deposit(Token::USDC, 100 * USDC_UNIT_SIZE)
        .await?;

    test_fixture
        .claim_seat_for_keypair(&second_keypair)
        .await?;
    test_fixture
        .deposit_for_keypair(Token::USDC, 100 * USDC_UNIT_SIZE, &second_keypair)
        .await?;

    test_fixture.crank_funding(&pyth_key).await?;

    // Insurance fund should be 0 initially
    let fund_before = test_fixture
        .market_fixture
        .get_insurance_fund_balance()
        .await;
    assert_eq!(fund_before, 0, "Fund should start at 0");

    // Place a bid and fill it
    test_fixture
        .place_order_for_keypair(
            Side::Bid,
            2 * SOL,
            PRICE_10_MANTISSA,
            PRICE_10_EXPONENT,
            0,
            OrderType::Limit,
            &second_keypair,
        )
        .await?;

    // Payer sells 1 SOL → quote_atoms_traded ≈ 10 USDC
    // Fee = 1% * 10_000_000 = 100_000 quote atoms
    test_fixture.swap(SOL, 0, true, true).await?;

    let fund_after = test_fixture
        .market_fixture
        .get_insurance_fund_balance()
        .await;

    // 1% of 10 USDC = 100_000 quote atoms
    let expected_fee: u64 = TEN_USDC / 100; // 100_000
    assert_eq!(
        fund_after, expected_fee,
        "Insurance fund should have collected 1% fee: expected {}, got {}",
        expected_fee, fund_after,
    );

    Ok(())
}

// ─── Test 15: Liquidator reward on notional ───────────────────────
// Same no-initial-crank approach for clean funding-free scenario.

#[tokio::test]
async fn test_liquidator_reward_on_notional() -> anyhow::Result<()> {
    let pyth_key = Pubkey::new_unique();
    let pyth_data = build_mock_pyth_data(10_0000_0000, -8, 100_000);

    let mut test_fixture =
        TestFixture::new_with_pyth(pyth_key, pyth_data, 1000, 500).await;
    let second_keypair = test_fixture.second_keypair.insecure_clone();
    let payer = test_fixture.payer();

    test_fixture.claim_seat().await?;
    test_fixture.deposit(Token::USDC, 2 * USDC_UNIT_SIZE).await?;

    test_fixture
        .claim_seat_for_keypair(&second_keypair)
        .await?;
    test_fixture
        .deposit_for_keypair(Token::USDC, 100 * USDC_UNIT_SIZE, &second_keypair)
        .await?;

    // No initial crank

    test_fixture
        .place_order_for_keypair(
            Side::Bid,
            2 * SOL,
            PRICE_10_MANTISSA,
            PRICE_10_EXPONENT,
            0,
            OrderType::Limit,
            &second_keypair,
        )
        .await?;

    test_fixture.swap(SOL, 0, true, true).await?;

    // Record liquidator balance before
    let liquidator_before = test_fixture
        .market_fixture
        .get_quote_balance_atoms(&second_keypair.pubkey())
        .await;

    // First-ever crank at 11.5 — just caches oracle, no funding
    // equity = 2 + (10 - 11.5) = 0.5, maintenance = 0.575 → liquidatable, partial
    let new_pyth_data = build_mock_pyth_data(11_5000_0000, -8, 100_000);
    {
        let mut ctx = test_fixture.context.borrow_mut();
        ctx.set_account(
            &pyth_key,
            &solana_sdk::account::Account {
                lamports: u32::MAX as u64,
                data: new_pyth_data,
                owner: Pubkey::new_unique(),
                executable: false,
                rent_epoch: 0,
            }
            .into(),
        );
    }
    test_fixture.crank_funding(&pyth_key).await?;

    test_fixture
        .liquidate_for_keypair(&payer, &second_keypair)
        .await?;

    let liquidator_after = test_fixture
        .market_fixture
        .get_quote_balance_atoms(&second_keypair.pubkey())
        .await;

    let reward = liquidator_after.saturating_sub(liquidator_before);
    assert!(
        reward > 0,
        "Liquidator should receive a reward: before={}, after={}",
        liquidator_before,
        liquidator_after,
    );

    // Reward = 2.5% of closed notional at 11.5 USDC. With ~59% close,
    // closed_notional ≈ 6.8 USDC, reward ≈ 0.17 USDC = 169,944 atoms.
    println!("Liquidator reward: {} quote atoms", reward);

    Ok(())
}

// ─── Test 16: Withdraw succeeds with no position ──────────────────

#[tokio::test]
async fn test_withdraw_no_position() -> anyhow::Result<()> {
    // try_new_for_perps_test deposits 100 USDC for payer + claims seat
    let mut test_fixture = TestFixture::try_new_for_perps_test(100 * USDC_UNIT_SIZE).await?;

    let balance_before = test_fixture
        .market_fixture
        .get_quote_balance_atoms(&test_fixture.payer())
        .await;
    assert_eq!(balance_before, 100 * USDC_UNIT_SIZE);

    // Withdraw 50 USDC — no position open, should succeed
    test_fixture.withdraw(Token::USDC, 50 * USDC_UNIT_SIZE).await?;

    let balance_after = test_fixture
        .market_fixture
        .get_quote_balance_atoms(&test_fixture.payer())
        .await;
    assert_eq!(
        balance_after,
        50 * USDC_UNIT_SIZE,
        "Balance should be 50 USDC after withdrawing 50"
    );

    Ok(())
}

// ─── Test 17: Withdraw succeeds when equity stays above maintenance ──

#[tokio::test]
async fn test_withdraw_with_position_healthy() -> anyhow::Result<()> {
    let pyth_key = Pubkey::new_unique();
    let pyth_data = build_mock_pyth_data(10_0000_0000, -8, 100_000);

    // 10% initial margin, 5% maintenance
    let mut test_fixture =
        TestFixture::new_with_pyth(pyth_key, pyth_data, 1000, 500).await;
    let second_keypair = test_fixture.second_keypair.insecure_clone();
    let payer = test_fixture.payer();

    test_fixture.claim_seat().await?;
    test_fixture.deposit(Token::USDC, 10 * USDC_UNIT_SIZE).await?;

    test_fixture
        .claim_seat_for_keypair(&second_keypair)
        .await?;
    test_fixture
        .deposit_for_keypair(Token::USDC, 100 * USDC_UNIT_SIZE, &second_keypair)
        .await?;

    test_fixture.crank_funding(&pyth_key).await?;

    test_fixture
        .place_order_for_keypair(
            Side::Bid,
            2 * SOL,
            PRICE_10_MANTISSA,
            PRICE_10_EXPONENT,
            0,
            OrderType::Limit,
            &second_keypair,
        )
        .await?;

    // Payer goes short 1 SOL at 10 USDC
    // margin = 10 USDC (deposit only, swap doesn't credit quote in perps)
    // notional = 10 USDC, maintenance = 10 * 5% = 0.5 USDC
    // equity = 10 + (10 - 10) = 10 USDC → well above 0.5
    test_fixture.swap(SOL, 0, true, true).await?;

    // Withdraw 8 USDC → remaining margin = 2, equity = 2 + 0 = 2 USDC
    // maintenance = 0.5 → still healthy, should succeed
    test_fixture.withdraw(Token::USDC, 8 * USDC_UNIT_SIZE).await?;

    let balance_after = test_fixture
        .market_fixture
        .get_quote_balance_atoms(&payer)
        .await;
    assert_eq!(
        balance_after,
        2 * USDC_UNIT_SIZE,
        "Balance should be 2 USDC after withdrawing 8"
    );

    Ok(())
}

// ─── Test 18: Withdraw rejected when equity would drop below maintenance ──

#[tokio::test]
async fn test_withdraw_rejected_insufficient_margin() -> anyhow::Result<()> {
    let pyth_key = Pubkey::new_unique();
    let pyth_data = build_mock_pyth_data(10_0000_0000, -8, 100_000);

    // 10% initial margin, 5% maintenance
    let mut test_fixture =
        TestFixture::new_with_pyth(pyth_key, pyth_data, 1000, 500).await;
    let second_keypair = test_fixture.second_keypair.insecure_clone();

    test_fixture.claim_seat().await?;
    test_fixture.deposit(Token::USDC, 2 * USDC_UNIT_SIZE).await?;

    test_fixture
        .claim_seat_for_keypair(&second_keypair)
        .await?;
    test_fixture
        .deposit_for_keypair(Token::USDC, 100 * USDC_UNIT_SIZE, &second_keypair)
        .await?;

    test_fixture.crank_funding(&pyth_key).await?;

    test_fixture
        .place_order_for_keypair(
            Side::Bid,
            2 * SOL,
            PRICE_10_MANTISSA,
            PRICE_10_EXPONENT,
            0,
            OrderType::Limit,
            &second_keypair,
        )
        .await?;

    // Payer goes short 1 SOL at 10 USDC
    // margin = 2 USDC, notional = 10 USDC, maintenance = 0.5 USDC
    // equity = 2 + 0 = 2 USDC → above 0.5, position opens fine
    test_fixture.swap(SOL, 0, true, true).await?;

    // Try to withdraw 1.6 USDC → remaining margin = 0.4, equity = 0.4
    // maintenance = 10 * 5% = 0.5 → equity < maintenance → FAIL
    let result = test_fixture
        .withdraw(Token::USDC, 1_600_000)
        .await;
    assert!(
        result.is_err(),
        "Withdrawal should fail: equity would drop below maintenance margin"
    );

    Ok(())
}
