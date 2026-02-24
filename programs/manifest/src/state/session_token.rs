use std::mem::size_of;

use bytemuck::{Pod, Zeroable};
use shank::ShankType;
use solana_program::pubkey::Pubkey;
use static_assertions::const_assert_eq;

/// Maximum session duration: 1 week in seconds (604800)
pub const MAX_SESSION_DURATION: i64 = 7 * 24 * 60 * 60;

/// Session-keys v2 uses an 8-byte Anchor discriminator prefix
pub const SESSION_TOKEN_DISCRIMINATOR_SIZE: usize = 8;

/// On-chain account size: 8-byte discriminator + 136-byte struct = 144 bytes.
/// Used in loaders to detect session token accounts by data_len().
pub const SESSION_TOKEN_SIZE: usize = SESSION_TOKEN_DISCRIMINATOR_SIZE + size_of::<SessionToken>();

/// SessionToken v2 account - allows ephemeral keypairs to sign on behalf of a user
///
/// Sessions are scoped to:
/// - A specific target program (prevents use on other programs)
/// - An expiration timestamp (max 1 week)
/// - A specific ephemeral signer keypair
///
/// On-chain account data: 8-byte discriminator + 136-byte struct = 144 bytes
/// PDA seeds: [b"session_token_v2", authority, session_signer]
#[repr(C)]
#[derive(Default, Debug, Copy, Clone, Zeroable, Pod, ShankType)]
pub struct SessionToken {
    /// The user's main wallet that created this session
    pub authority: Pubkey,

    /// The program this session is authorized for (Manifest program ID)
    pub target_program: Pubkey,

    /// The ephemeral keypair that will sign transactions
    pub session_signer: Pubkey,

    /// The fee payer who created the session (receives rent on revocation)
    pub fee_payer: Pubkey,

    /// Unix timestamp when this session expires
    pub valid_until: i64,
}

// 32 + // authority
// 32 + // target_program
// 32 + // session_signer
// 32 + // fee_payer
//  8   // valid_until
// = 136 (+ 8 byte discriminator on-chain = 144 = SESSION_TOKEN_SIZE)
const_assert_eq!(size_of::<SessionToken>(), 136);
const_assert_eq!(SESSION_TOKEN_SIZE, 144);
const_assert_eq!(size_of::<SessionToken>() % 8, 0);

impl SessionToken {
    /// Create a new session token
    pub fn new(
        authority: Pubkey,
        target_program: Pubkey,
        session_signer: Pubkey,
        valid_until: i64,
        fee_payer: Pubkey,
    ) -> Self {
        SessionToken {
            authority,
            target_program,
            session_signer,
            valid_until,
            fee_payer,
        }
    }

    /// Check if the session is still valid (not expired)
    pub fn is_valid(&self, current_timestamp: i64) -> bool {
        current_timestamp <= self.valid_until
    }

    /// Get the PDA seeds for this session token (v2)
    pub fn get_seeds(target_program: &Pubkey, authority: &Pubkey, session_signer: &Pubkey) -> Vec<Vec<u8>> {
        vec![
            b"session_token_v2".to_vec(),
            target_program.to_bytes().to_vec(),
            authority.to_bytes().to_vec(),
            session_signer.to_bytes().to_vec(),
        ]
    }

    /// Derive the PDA address for a session token (v2)
    pub fn get_address(
        target_program: &Pubkey,
        authority: &Pubkey,
        session_signer: &Pubkey,
        session_keys_program_id: &Pubkey,
    ) -> (Pubkey, u8) {
        Pubkey::find_program_address(
            &[
                b"session_token_v2",
                target_program.as_ref(),
                authority.as_ref(),
                session_signer.as_ref(),
            ],
            session_keys_program_id,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_token_size() {
        // Struct is 136 bytes, on-chain with discriminator is 144
        assert_eq!(size_of::<SessionToken>(), 136);
        assert_eq!(SESSION_TOKEN_SIZE, 144);
    }

    #[test]
    fn test_session_validation() {
        let authority = Pubkey::new_unique();
        let target_program = Pubkey::new_unique();
        let session_signer = Pubkey::new_unique();
        let fee_payer = Pubkey::new_unique();

        let current_time = 1000000;
        let expiration = current_time + 3600; // 1 hour from now

        let session = SessionToken::new(
            authority,
            target_program,
            session_signer,
            expiration,
            fee_payer,
        );

        // Should be valid before expiration
        assert!(session.is_valid(current_time));
        assert!(session.is_valid(expiration));

        // Should be invalid after expiration
        assert!(!session.is_valid(expiration + 1));
    }

    #[test]
    fn test_pda_derivation() {
        let target_program = Pubkey::new_unique();
        let authority = Pubkey::new_unique();
        let session_signer = Pubkey::new_unique();
        let session_keys_program_id = Pubkey::new_unique();

        let (pda, _bump) = SessionToken::get_address(&target_program, &authority, &session_signer, &session_keys_program_id);

        // PDA should be deterministic
        let (pda2, _bump2) = SessionToken::get_address(&target_program, &authority, &session_signer, &session_keys_program_id);
        assert_eq!(pda, pda2);
    }
}
