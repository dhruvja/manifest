use manifest::{
    program::get_trader_index_with_hint,
    quantities::BaseAtoms,
    state::{SessionToken, SESSION_KEYS_PROGRAM_ID, SESSION_TOKEN_SIZE},
    validation::SESSION_KEYS_PROGRAM_ID as VALIDATION_SESSION_PROGRAM_ID,
};
use solana_program::pubkey::Pubkey;
use solana_sdk::{signature::Keypair, signer::Signer};

use crate::test_fixture::{perps::new_with_pyth, TestFixture};

#[test]
fn test_session_token_structure() {
    // Test that SessionToken has the correct size
    assert_eq!(std::mem::size_of::<SessionToken>(), SESSION_TOKEN_SIZE);
    assert_eq!(SESSION_TOKEN_SIZE, 168);

    // Test SessionToken creation
    let authority = Pubkey::new_unique();
    let target_program = Pubkey::new_unique();
    let session_signer = Pubkey::new_unique();
    let fee_payer = Pubkey::new_unique();
    let valid_until = 1000000i64;

    let session = SessionToken::new(
        authority,
        target_program,
        session_signer,
        valid_until,
        fee_payer,
    );

    assert_eq!(session.authority, authority);
    assert_eq!(session.target_program, target_program);
    assert_eq!(session.session_signer, session_signer);
    assert_eq!(session.valid_until, valid_until);
    assert_eq!(session.fee_payer, fee_payer);
}

#[test]
fn test_session_validation() {
    let authority = Pubkey::new_unique();
    let target_program = Pubkey::new_unique();
    let session_signer = Pubkey::new_unique();
    let fee_payer = Pubkey::new_unique();

    // Test valid session
    let current_time = 1000000i64;
    let future_expiry = current_time + 3600;
    let session = SessionToken::new(
        authority,
        target_program,
        session_signer,
        future_expiry,
        fee_payer,
    );

    assert!(session.is_valid(current_time));
    assert!(session.is_valid(future_expiry)); // Should be valid at exact expiry time
    assert!(!session.is_valid(future_expiry + 1)); // Should be invalid after expiry
}

#[test]
fn test_session_pda_derivation() {
    let authority = Pubkey::new_unique();
    let session_signer = Pubkey::new_unique();
    let program_id = SESSION_KEYS_PROGRAM_ID;

    let (pda1, bump1) = SessionToken::get_address(&authority, &session_signer, &program_id);
    let (pda2, bump2) = SessionToken::get_address(&authority, &session_signer, &program_id);

    // PDAs should be deterministic
    assert_eq!(pda1, pda2);
    assert_eq!(bump1, bump2);

    // Different session signers should produce different PDAs
    let different_signer = Pubkey::new_unique();
    let (pda3, _) = SessionToken::get_address(&authority, &different_signer, &program_id);
    assert_ne!(pda1, pda3);
}

#[test]
#[ignore] // This is an integration test that requires session-keys program deployed
fn test_batch_update_with_session() {
    // This test demonstrates how to use sessions with batch_update
    // It requires the session-keys program to be deployed and a session token created

    let mut fix = new_with_pyth();
    let trader_keypair = Keypair::new();
    let session_signer_keypair = Keypair::new();

    // In a real scenario, you would:
    // 1. Create a session token using the session-keys program
    // 2. Pass the session token account + session_signer as signer (instead of trader)
    // 3. batch_update should validate the session and allow the operation

    // For now, we just test that the normal path (without session) still works
    fix.claim_seat(&trader_keypair.pubkey());
    fix.deposit_for_keypair(100_000_000, &trader_keypair); // 100 USDC

    // Place an order using the trader keypair directly (no session)
    let orders = vec![manifest::program::processor::batch_update::PlaceOrderParams::new(
        1_000_000,   // 1 SOL
        100_000,     // price mantissa
        -6,          // price exponent (100 USDC)
        true,        // is_bid
        manifest::state::OrderType::Limit,
        0,           // no expiration
    )];

    fix.batch_update_for_keypair(&trader_keypair, vec![], orders);

    // Verify the order was placed
    let market_data = fix.market.try_borrow_data().unwrap();
    let dynamic_account = manifest::program::get_dynamic_account(&market_data);
    let trader_index = dynamic_account.get_trader_index(&trader_keypair.pubkey());
    assert_ne!(trader_index, hypertree::NIL);
}

#[test]
fn test_session_constants() {
    // Verify that the SESSION_KEYS_PROGRAM_ID constant is accessible
    let _program_id = SESSION_KEYS_PROGRAM_ID;
    let _program_id2 = VALIDATION_SESSION_PROGRAM_ID;

    // They should be the same
    assert_eq!(_program_id, _program_id2);

    // Session duration constant
    use manifest::state::MAX_SESSION_DURATION;
    assert_eq!(MAX_SESSION_DURATION, 7 * 24 * 60 * 60); // 1 week in seconds
}

// Note: Comprehensive integration tests with actual session tokens would require:
// 1. Deploying the session-keys program to a test validator
// 2. Creating session tokens via the session-keys program
// 3. Using those session tokens in batch_update/swap calls
// 4. Verifying that:
//    - Valid sessions allow operations
//    - Expired sessions are rejected
//    - Sessions for wrong programs are rejected
//    - Sessions with wrong signers are rejected
