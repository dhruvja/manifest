use crate::program::{liquidate::LiquidateParams, ManifestInstruction};
use borsh::BorshSerialize;
use solana_program::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    system_program,
};

pub fn liquidate_instruction(
    market: &Pubkey,
    liquidator: &Pubkey,
    trader_to_liquidate: &Pubkey,
) -> Instruction {
    Instruction {
        program_id: crate::id(),
        accounts: vec![
            AccountMeta::new(*liquidator, true),
            AccountMeta::new(*market, false),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
        data: [
            ManifestInstruction::Liquidate.to_vec(),
            LiquidateParams::new(*trader_to_liquidate)
                .try_to_vec()
                .unwrap(),
        ]
        .concat(),
    }
}
