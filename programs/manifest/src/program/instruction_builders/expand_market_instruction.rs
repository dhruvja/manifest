use crate::program::ManifestInstruction;
use solana_program::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};

/// ephemeral-rollups-spl program ID (lamport escrow owner).
pub const EPHEMERAL_ROLLUPS_SPL_ID: Pubkey =
    solana_program::pubkey!("DL2q6XaUpXsPsYrDpbieiXG6UisaUpzMSZCTkSvzn2Am");

/// Derive the lamport escrow PDA from ephemeral-rollups-spl.
/// Seeds: `[b"lamport_escrow", authority, validator, slot.to_le_bytes()]`
pub fn get_escrow_address(authority: &Pubkey, validator: &Pubkey, slot: u64) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            b"lamport_escrow",
            authority.as_ref(),
            validator.as_ref(),
            &slot.to_le_bytes(),
        ],
        &EPHEMERAL_ROLLUPS_SPL_ID,
    )
}

/// Build an expand-market instruction.
///
/// Accounts: `[payer (signer), market (writable), escrow (writable), er_spl_program]`
///
/// Data layout (after discriminant): `[num_blocks: u32, validator: Pubkey, escrow_slot: u64]`
pub fn expand_market_instruction(
    market: &Pubkey,
    payer: &Pubkey,
    escrow_pda: &Pubkey,
    er_spl_program: &Pubkey,
    validator: &Pubkey,
    escrow_slot: u64,
) -> Instruction {
    expand_market_n_instruction(
        market,
        payer,
        escrow_pda,
        er_spl_program,
        1,
        validator,
        escrow_slot,
    )
}

/// Build a batch expand-market instruction that ensures at least `num_free_blocks`
/// free blocks exist.
///
/// Accounts: `[payer (signer), market (writable), escrow (writable), er_spl_program]`
///
/// Data layout (after discriminant): `[num_blocks: u32, validator: Pubkey, escrow_slot: u64]`
pub fn expand_market_n_instruction(
    market: &Pubkey,
    payer: &Pubkey,
    escrow_pda: &Pubkey,
    er_spl_program: &Pubkey,
    num_free_blocks: u32,
    validator: &Pubkey,
    escrow_slot: u64,
) -> Instruction {
    let mut data = ManifestInstruction::Expand.to_vec();
    data.extend_from_slice(&num_free_blocks.to_le_bytes());
    data.extend_from_slice(&validator.to_bytes());
    data.extend_from_slice(&escrow_slot.to_le_bytes());

    Instruction {
        program_id: crate::id(),
        accounts: vec![
            AccountMeta::new_readonly(*payer, true),
            AccountMeta::new(*market, false),
            AccountMeta::new(*escrow_pda, false),
            AccountMeta::new_readonly(*er_spl_program, false),
        ],
        data,
    }
}
