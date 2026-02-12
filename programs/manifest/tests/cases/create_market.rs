use manifest::program::create_market_instructions;
use manifest::validation::get_market_address;
use solana_program_test::tokio;
use solana_sdk::{instruction::Instruction, signer::Signer};

use crate::TestFixture;

#[tokio::test]
async fn create_market() -> anyhow::Result<()> {
    let _test_fixture: TestFixture = TestFixture::new().await;

    Ok(())
}

#[tokio::test]
async fn create_market_fail_same_base_and_quote() -> anyhow::Result<()> {
    let test_fixture: TestFixture = TestFixture::new().await;

    assert!(test_fixture
        .create_new_market(
            &test_fixture.sol_mint_fixture.key,
            &test_fixture.sol_mint_fixture.key
        )
        .await
        .is_err());
    Ok(())
}

#[tokio::test]
async fn create_market_fail_already_initialized() -> anyhow::Result<()> {
    let test_fixture: TestFixture = TestFixture::new().await;

    // The default TestFixture already created a market with sol/usdc.
    // Trying to create the same pair again should fail because the PDA already exists.
    assert!(test_fixture
        .create_new_market(
            &test_fixture.sol_mint_fixture.key,
            &test_fixture.usdc_mint_fixture.key
        )
        .await
        .is_err());

    Ok(())
}

#[tokio::test]
async fn create_market_pda_address() -> anyhow::Result<()> {
    let test_fixture: TestFixture = TestFixture::new().await;

    let (expected_market_key, _) = get_market_address(
        &test_fixture.sol_mint_fixture.key,
        &test_fixture.usdc_mint_fixture.key,
    );
    assert_eq!(test_fixture.market_fixture.key, expected_market_key);

    Ok(())
}
