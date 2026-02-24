# Session Keys Integration for Manifest Perps DEX

This document explains how session keys have been integrated into the Manifest perpetual futures DEX program.

## Overview

Session keys allow users to create ephemeral keypairs that can sign transactions on their behalf for a limited time and specific program. This eliminates the need for constant wallet approvals, enabling a smoother trading experience.

## Architecture

### External Session Program

The session token creation and revocation is handled by the **MagicBlock session-keys v2 program** (ID: `KeyspM2ssCJbqUhQ4k7sveSiY4WjnYsrXkC8oDbwde5`).

### Manifest Program Integration

The Manifest program validates existing session tokens without managing their lifecycle:

1. **Session Token Structure** ([session_token.rs](programs/manifest/src/state/session_token.rs))
   - 144-byte on-chain account (8-byte Anchor discriminator + 136-byte struct):
     - `authority`: User's main wallet (32 bytes)
     - `target_program`: Manifest program ID (32 bytes)
     - `session_signer`: Ephemeral keypair (32 bytes)
     - `fee_payer`: Receives rent on revocation (32 bytes)
     - `valid_until`: Expiration timestamp, max 1 week (8 bytes)

2. **Validation Logic** ([session_validator.rs](programs/manifest/src/validation/session_validator.rs))
   - `validate_session_or_authority()` checks either:
     - Direct authority signing (normal case), OR
     - Valid session token + session signer

3. **Supported Instructions**
   - ✅ `BatchUpdate` - Place/cancel orders with session
   - ✅ `Swap` - Execute swaps with session
   - Future: Deposit, Withdraw, Liquidate (can be added same way)

## How It Works

### Account Structure

#### BatchUpdate with Session
```
Accounts (in order):
0. payer (signer) - Either authority OR session_signer
1. market (writable)
2. system_program
3. [OPTIONAL] session_token (144 bytes, owned by session-keys v2 program)
4. [OPTIONAL] base_mint (for global accounts)
5. [OPTIONAL] base_global (writable)
... (rest of global accounts)
```

#### Swap with Session
```
Accounts (in order):
0. payer
1. owner (signer) - Either authority OR session_signer
2. market (writable)
3. system_program
4. [OPTIONAL] session_token (144 bytes, owned by session-keys v2 program)
5. trader_quote (writable)
6. quote_vault (writable)
... (rest of accounts)
```

### Validation Flow

When a transaction is submitted:

1. **Load Context**: Check if account #3 (after system_program) is a session token
   - Identified by data length == 144 bytes
   - If not present, validates normal authority signing

2. **Validate Session** (if present):
   ```rust
   validate_session_or_authority(
       &trader_authority,      // From claimed seat
       signer_account,         // The keypair that signed
       session_token,          // Optional session token account
       &SESSION_KEYS_PROGRAM_ID,
       &manifest_program_id,
   )
   ```

3. **Checks Performed**:
   - ✅ Session token PDA is correct: `[b"session_token_v2", target_program,  session_signer, authority]`
   - ✅ Session token owned by session-keys program
   - ✅ Authority matches trader's claimed seat
   - ✅ Target program matches Manifest program ID
   - ✅ Session signer matches transaction signer
   - ✅ Session not expired (`current_time <= valid_until`)

4. **Authorization**: If all checks pass, operation proceeds as if authority signed directly

## Usage Examples

### Client-Side Integration (TypeScript)

```typescript
import { PublicKey, Transaction, Keypair } from '@solana/web3.js';
import { createSessionIx } from '@gumhq/spl-session';

// 1. Create ephemeral session keypair (store securely, but not as critical as main wallet)
const sessionSigner = Keypair.generate();

// 2. User signs ONCE to create session (via session-keys program)
const createSessionIx = await createSessionIx({
  sessionSigner: sessionSigner.publicKey,
  authority: userWallet.publicKey,
  targetProgram: MANIFEST_PROGRAM_ID,
  validUntil: Math.floor(Date.now() / 1000) + 3600, // 1 hour from now
});

await provider.sendAndConfirm(
  new Transaction().add(createSessionIx),
  [userWallet] // User signs this ONCE
);

// 3. Derive session token PDA (v2 seeds)
const [sessionTokenPDA] = PublicKey.findProgramAddressSync(
  [
    Buffer.from("session_token_v2"),
    MANIFEST_PROGRAM_ID.toBuffer(),
    userWallet.publicKey.toBuffer(),
    sessionSigner.publicKey.toBuffer(),
  ],
  SESSION_KEYS_PROGRAM_ID
);

// 4. Now trade WITHOUT user signatures!
const batchUpdateIx = batchUpdateInstruction({
  market,
  payer: sessionSigner.publicKey,     // Session signer, not user!
  sessionToken: sessionTokenPDA,       // Optional session token
  orders: [...],
  cancels: [...],
});

// Session signer signs - NO user interaction!
await provider.sendAndConfirm(
  new Transaction().add(batchUpdateIx),
  [sessionSigner]  // Ephemeral keypair signs
);
```

### Rust SDK Integration

```rust
use manifest::validation::validate_session_or_authority;
use manifest::state::{SESSION_KEYS_PROGRAM_ID, SessionToken};

// In your processor function:
let trader_authority = dynamic_account.get_trader_key_by_index(trader_index);

validate_session_or_authority(
    trader_authority,
    signer_account,
    session_token_opt,  // None if not using session
    &SESSION_KEYS_PROGRAM_ID,
    program_id,
)?;

// If validation passes, proceed with operation
```

## Security Considerations

### Session Token Security

- ✅ **Scoped to specific program**: Cannot be used on other programs
- ✅ **Time-limited**: Maximum 1 week duration
- ✅ **Revocable**: Can be revoked anytime by calling session-keys program
- ✅ **PDA-based**: Deterministic addresses prevent confusion

### Attack Vectors Prevented

1. **Cross-program abuse**: `target_program` must match Manifest program ID
2. **Expired sessions**: Checked against `Clock` sysvar
3. **Wrong signer**: `session_signer` must match transaction signer
4. **Authority mismatch**: Must match trader's claimed seat

### Recommended Practices

1. **Short durations for high-value**: Use shorter session durations (minutes) for large positions
2. **Revoke after use**: Revoke sessions when done trading
3. **Secure session keys**: Store ephemeral keypairs securely (though less critical than main wallet)
4. **Monitor sessions**: Track active sessions and expiration times

## Implementation Details

### Files Modified

1. **State**:
   - `state/session_token.rs` - SessionToken struct and helpers
   - `state/constants.rs` - SESSION_KEYS_PROGRAM_ID constant

2. **Validation**:
   - `validation/session_validator.rs` - validate_session_or_authority()
   - `validation/loaders.rs` - Load optional session token in BatchUpdateContext and SwapContext

3. **Processors**:
   - `processor/batch_update.rs` - Session validation before operations
   - `processor/swap.rs` - Session validation before operations

4. **Errors**:
   - `program/error.rs` - Session-specific error variants

5. **Tests**:
   - `tests/cases/sessions.rs` - Session token structure and validation tests

### Error Codes

- `SessionExpired (29)`: Session token has expired
- `InvalidSession (30)`: Session token is invalid or malformed
- `InvalidSessionProgram (31)`: Session not authorized for this program
- `InvalidSessionSigner (32)`: Session signer does not match transaction signer
- `SessionDurationTooLong (33)`: Session duration exceeds maximum (1 week)
- `InvalidSessionAuthority (34)`: Session authority does not match expected

## Testing

### Unit Tests

```bash
# Run session-specific tests
cargo test --lib sessions

# Run specific test
cargo test test_session_token_structure --lib
```

### Integration Tests

Full integration tests require:
1. Deploying session-keys program to test validator
2. Creating actual session tokens
3. Testing with real transactions

Example integration test (requires setup):
```bash
cargo test test_batch_update_with_session -- --ignored
```

## Future Enhancements

- [ ] Add session support to Deposit/Withdraw instructions
- [ ] Add session support to Liquidate instruction
- [ ] Create Rust client SDK helpers for session management
- [ ] Add session creation/revocation helpers in manifest-cli
- [ ] Implement session token refresh mechanism
- [ ] Add monitoring/analytics for session usage

## References

- [MagicBlock Session Keys Docs](https://docs.magicblock.gg/pages/tools/session-keys/integrating-sessions-in-your-program)
- [Session Keys Blog Post](https://www.magicblock.xyz/blog/session-keys)
- [Gum Program Library](https://github.com/magicblock-labs/session-keys)
- [session-keys Crate](https://crates.io/crates/session-keys)

## Session Keys Program ID

**Devnet/Mainnet**: `KeyspM2ssCJbqUhQ4k7sveSiY4WjnYsrXkC8oDbwde5`

Update this in [constants.rs](programs/manifest/src/state/constants.rs) if using a different deployment.
