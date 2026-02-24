use solana_program::{
    account_info::AccountInfo, clock::Clock, program_error::ProgramError, pubkey::Pubkey,
    sysvar::Sysvar,
};

use crate::{
    program::error::ManifestError,
    require,
    state::{SessionToken, SESSION_TOKEN_DISCRIMINATOR_SIZE},
};

/// Validates that the signer is authorized either directly as the authority,
/// or via a valid session token.
///
/// This replaces Anchor's `session_auth_or` macro for native Solana.
///
/// # Arguments
/// * `signer` - The account that signed the transaction
/// * `session_token` - Optional session token account
/// * `session_keys_program_id` - The session-keys program ID
/// * `manifest_program_id` - The Manifest program ID (target program)
///
/// # Returns
/// * `Ok(Pubkey)` - The trader authority (from session or direct signer)
/// * `Err(ProgramError)` if validation fails
pub fn validate_session_or_authority<'a>(
    signer: &AccountInfo<'a>,
    session_token: Option<&AccountInfo<'a>>,
    session_keys_program_id: &Pubkey,
    manifest_program_id: &Pubkey,
) -> Result<Pubkey, ProgramError> {
    // If no session token provided, signer must be the authority
    if session_token.is_none() {
        require!(
            signer.is_signer,
            ProgramError::MissingRequiredSignature,
            "Authority must sign transaction"
        )?;
        return Ok(*signer.key);
    }

    // Session token provided - validate it
    let session_token_info = session_token.unwrap();

    // Verify signer actually signed (fail fast before expensive operations)
    require!(
        signer.is_signer,
        ProgramError::MissingRequiredSignature,
        "Session signer must sign transaction"
    )?;

    // Verify session token is owned by session-keys program
    // Only the session-keys program can write to accounts it owns, so this
    // ensures the data is trusted and the account is a valid session token PDA
    require!(
        session_token_info.owner == session_keys_program_id,
        ProgramError::from(ManifestError::InvalidSession),
        "Session token not owned by session-keys program"
    )?;

    // Deserialize and validate session token (v2: 8-byte discriminator + struct)
    let session_data = session_token_info.try_borrow_data()?;
    let struct_size = std::mem::size_of::<SessionToken>();

    require!(
        session_data.len() >= SESSION_TOKEN_DISCRIMINATOR_SIZE + struct_size,
        ProgramError::from(ManifestError::InvalidSession),
        "Session token account data too small"
    )?;

    // Skip the 8-byte Anchor discriminator
    let session = bytemuck::try_from_bytes::<SessionToken>(
        &session_data[SESSION_TOKEN_DISCRIMINATOR_SIZE..SESSION_TOKEN_DISCRIMINATOR_SIZE + struct_size],
    )
    .map_err(|_| ProgramError::from(ManifestError::InvalidSession))?;

    // Verify the session token PDA is correct
    // This proves the session-keys program created this exact session token.
    // The PDA derivation uses the authority (from deserialized data) and the signer.
    // Even if someone crafted malicious data, the PDA wouldn't match unless the
    // session-keys program explicitly created a session for this authority+signer pair.
    let expected_seeds: &[&[u8]] = &[
        b"session_token_v2",
        manifest_program_id.as_ref(),
        signer.key.as_ref(),
        session.authority.as_ref(),
    ];
    let (expected_pda, _bump) = Pubkey::find_program_address(expected_seeds, session_keys_program_id);

    require!(
        session_token_info.key == &expected_pda,
        ProgramError::from(ManifestError::InvalidSession),
        "Session token PDA does not match expected address"
    )?;

    // Check target program matches Manifest program
    require!(
        session.target_program == *manifest_program_id,
        ProgramError::from(ManifestError::InvalidSessionProgram),
        "Session not authorized for this program"
    )?;

    // Check session_signer matches the actual signer
    require!(
        session.session_signer == *signer.key,
        ProgramError::from(ManifestError::InvalidSessionSigner),
        "Session signer does not match transaction signer"
    )?;

    // Check expiration
    let clock = Clock::get()?;
    require!(
        session.is_valid(clock.unix_timestamp),
        ProgramError::from(ManifestError::SessionExpired),
        "Session has expired"
    )?;

    // Return the trader authority from the session
    Ok(session.authority)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_program::clock::Clock;

    #[test]
    fn test_direct_authority_validation() {
        // This would require mocking AccountInfo which is complex in native Solana
        // In practice, this is tested via integration tests
    }
}
