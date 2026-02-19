use crate::program::ManifestInstruction;
use solana_program::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    system_program,
};

pub fn expand_market_instruction(market: &Pubkey, payer: &Pubkey) -> Instruction {
    Instruction {
        program_id: crate::id(),
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(*market, false),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
        data: [ManifestInstruction::Expand.to_vec()].concat(),
    }
}

/// Expand with an explicit count: ensures at least `num_free_blocks` free blocks exist.
pub fn expand_market_n_instruction(
    market: &Pubkey,
    payer: &Pubkey,
    num_free_blocks: u32,
) -> Instruction {
    Instruction {
        program_id: crate::id(),
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(*market, false),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
        data: [
            ManifestInstruction::Expand.to_vec(),
            num_free_blocks.to_le_bytes().to_vec(),
        ]
        .concat(),
    }
}
