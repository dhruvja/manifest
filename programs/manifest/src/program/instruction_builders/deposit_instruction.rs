use crate::{
    program::{deposit::DepositParams, ManifestInstruction},
    validation::get_vault_address,
};
use borsh::BorshSerialize;
use hypertree::DataIndex;
use solana_program::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};

pub fn deposit_instruction(
    market: &Pubkey,
    payer: &Pubkey,
    mint: &Pubkey,
    amount_atoms: u64,
    trader_token_account: &Pubkey,
    token_program: Pubkey,
    trader_index_hint: Option<DataIndex>,
) -> Instruction {
    let (vault_address, _) = get_vault_address(market, mint);
    deposit_instruction_with_vault(
        market,
        payer,
        mint,
        amount_atoms,
        trader_token_account,
        &vault_address,
        token_program,
        trader_index_hint,
    )
}

/// Deposit instruction with an explicit vault address.
/// Use this for ephemeral mode where the vault is an EphemeralAta
/// at a different address than the SPL vault PDA.
pub fn deposit_instruction_with_vault(
    market: &Pubkey,
    payer: &Pubkey,
    mint: &Pubkey,
    amount_atoms: u64,
    trader_token_account: &Pubkey,
    vault: &Pubkey,
    token_program: Pubkey,
    trader_index_hint: Option<DataIndex>,
) -> Instruction {
    Instruction {
        program_id: crate::id(),
        accounts: vec![
            AccountMeta::new_readonly(*payer, true),
            AccountMeta::new(*market, false),
            AccountMeta::new(*trader_token_account, false),
            AccountMeta::new(*vault, false),
            AccountMeta::new_readonly(token_program, false),
            AccountMeta::new_readonly(*mint, false),
        ],
        data: [
            ManifestInstruction::Deposit.to_vec(),
            DepositParams::new(amount_atoms, trader_index_hint)
                .try_to_vec()
                .unwrap(),
        ]
        .concat(),
    }
}
