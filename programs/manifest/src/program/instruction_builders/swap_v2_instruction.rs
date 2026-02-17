use crate::{
    program::{swap::SwapParams, ManifestInstruction},
    validation::get_vault_address,
};
use borsh::BorshSerialize;
use solana_program::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
};

/// Build a SwapV2 instruction for the perps market.
/// SwapV2 separates payer (gas) from owner (token accounts).
///
/// Accounts: [payer(signer), owner(signer), market(writable), system_program,
///            trader_quote(writable), quote_vault(writable), token_program_quote]
#[allow(clippy::too_many_arguments)]
pub fn swap_v2_instruction(
    market: &Pubkey,
    payer: &Pubkey,
    owner: &Pubkey,
    _base_mint: &Pubkey,
    quote_mint: &Pubkey,
    _trader_base_account: &Pubkey,
    trader_quote_account: &Pubkey,
    in_atoms: u64,
    out_atoms: u64,
    is_base_in: bool,
    is_exact_in: bool,
    _token_program_base: Pubkey,
    token_program_quote: Pubkey,
    _include_global: bool,
) -> Instruction {
    let (vault_quote_account, _) = get_vault_address(market, quote_mint);

    let account_metas: Vec<AccountMeta> = vec![
        AccountMeta::new_readonly(*payer, true),
        AccountMeta::new_readonly(*owner, true),
        AccountMeta::new(*market, false),
        AccountMeta::new_readonly(solana_program::system_program::id(), false),
        AccountMeta::new(*trader_quote_account, false),
        AccountMeta::new(vault_quote_account, false),
        AccountMeta::new_readonly(token_program_quote, false),
    ];

    Instruction {
        program_id: crate::id(),
        accounts: account_metas,
        data: [
            ManifestInstruction::Swap.to_vec(),
            SwapParams::new(in_atoms, out_atoms, is_base_in, is_exact_in)
                .try_to_vec()
                .unwrap(),
        ]
        .concat(),
    }
}
