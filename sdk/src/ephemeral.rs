use solana_program::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    system_program,
};
use spl_associated_token_account::get_associated_token_address;

use crate::config::ManifestConfig;

/// Derive an ephemeral ATA PDA for `(owner, mint)`.
pub fn get_ephemeral_ata(cfg: &ManifestConfig, owner: &Pubkey, mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[owner.as_ref(), mint.as_ref()],
        &cfg.ephemeral_spl_token_id,
    )
}

/// Derive the global vault data account PDA for a mint.
pub fn get_global_vault(cfg: &ManifestConfig, mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[mint.as_ref()], &cfg.ephemeral_spl_token_id)
}

/// The SPL token account (ATA) that the global vault PDA owns — holds real tokens.
pub fn get_vault_token_account(cfg: &ManifestConfig, mint: &Pubkey) -> Pubkey {
    let (global_vault, _) = get_global_vault(cfg, mint);
    get_associated_token_address(&global_vault, mint)
}

/// InitializeGlobalVault (disc=1): create the per-mint global vault data account.
pub fn ix_init_global_vault(cfg: &ManifestConfig, payer: &Pubkey, mint: &Pubkey) -> Instruction {
    let e_spl = cfg.ephemeral_spl_token_id;
    let (vault, bump) = get_global_vault(cfg, mint);
    Instruction {
        program_id: e_spl,
        accounts: vec![
            AccountMeta::new(vault, false),
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
        data: vec![1u8, bump],
    }
}

/// InitializeEphemeralAta (disc=0): create an ephemeral ATA for `owner`.
pub fn ix_init_ephemeral_ata(
    cfg: &ManifestConfig,
    payer: &Pubkey,
    owner: &Pubkey,
    mint: &Pubkey,
) -> Instruction {
    let e_spl = cfg.ephemeral_spl_token_id;
    let (ata, bump) = get_ephemeral_ata(cfg, owner, mint);
    Instruction {
        program_id: e_spl,
        accounts: vec![
            AccountMeta::new(ata, false),
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(*owner, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
        data: vec![0u8, bump],
    }
}

/// DepositSplTokens (disc=2): move real SPL tokens into an ephemeral ATA.
pub fn ix_deposit_spl_tokens(
    cfg: &ManifestConfig,
    authority: &Pubkey,
    recipient: &Pubkey,
    mint: &Pubkey,
    source_token: &Pubkey,
    amount: u64,
) -> Instruction {
    let e_spl = cfg.ephemeral_spl_token_id;
    let (ata, _) = get_ephemeral_ata(cfg, recipient, mint);
    let (global_vault, _) = get_global_vault(cfg, mint);
    let vault_token = get_vault_token_account(cfg, mint);

    let mut data = vec![2u8];
    data.extend_from_slice(&amount.to_le_bytes());

    Instruction {
        program_id: e_spl,
        accounts: vec![
            AccountMeta::new(ata, false),
            AccountMeta::new_readonly(global_vault, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new(*source_token, false),
            AccountMeta::new(vault_token, false),
            AccountMeta::new(*authority, true),
            AccountMeta::new_readonly(spl_token::id(), false),
        ],
        data,
    }
}

/// DelegateEphemeralAta (disc=4): delegate the payer's ephemeral ATA to the ER.
pub fn ix_delegate_ephemeral_ata(
    cfg: &ManifestConfig,
    payer: &Pubkey,
    mint: &Pubkey,
) -> Instruction {
    let e_spl = cfg.ephemeral_spl_token_id;
    let dlp = cfg.delegation_program_id;
    let (ata, bump) = get_ephemeral_ata(cfg, payer, mint);
    let (buffer, _) = Pubkey::find_program_address(&[b"buffer", ata.as_ref()], &e_spl);
    let (delegation_record, _) =
        Pubkey::find_program_address(&[b"delegation", ata.as_ref()], &dlp);
    let (delegation_metadata, _) =
        Pubkey::find_program_address(&[b"delegation-metadata", ata.as_ref()], &dlp);

    Instruction {
        program_id: e_spl,
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(ata, false),
            AccountMeta::new_readonly(e_spl, false),
            AccountMeta::new(buffer, false),
            AccountMeta::new(delegation_record, false),
            AccountMeta::new(delegation_metadata, false),
            AccountMeta::new_readonly(dlp, false),
            AccountMeta::new_readonly(system_program::id(), false),
        ],
        data: vec![4u8, bump],
    }
}

/// Build a DelegateMarket instruction that delegates both the market account
/// and its ephemeral vault ATA to the MagicBlock ER.
pub fn delegate_market_ix(
    cfg: &ManifestConfig,
    payer: &Pubkey,
    market: &Pubkey,
    quote_mint: &Pubkey,
) -> Instruction {
    let dlp = cfg.delegation_program_id;
    let e_spl = cfg.ephemeral_spl_token_id;
    let owner = cfg.manifest_program_id;

    let (delegation_record, _) =
        Pubkey::find_program_address(&[b"delegation", market.as_ref()], &dlp);
    let (delegation_metadata, _) =
        Pubkey::find_program_address(&[b"delegation-metadata", market.as_ref()], &dlp);
    let (buffer, _) = Pubkey::find_program_address(&[b"buffer", market.as_ref()], &owner);

    let ephemeral_vault_ata = get_associated_token_address(market, quote_mint);
    let (vault_ata_buffer, _) =
        Pubkey::find_program_address(&[b"buffer", ephemeral_vault_ata.as_ref()], &e_spl);
    let (vault_ata_delegation_record, _) =
        Pubkey::find_program_address(&[b"delegation", ephemeral_vault_ata.as_ref()], &dlp);
    let (vault_ata_delegation_metadata, _) = Pubkey::find_program_address(
        &[b"delegation-metadata", ephemeral_vault_ata.as_ref()],
        &dlp,
    );

    Instruction {
        program_id: cfg.manifest_program_id,
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(*market, false),
            AccountMeta::new_readonly(owner, false),
            AccountMeta::new_readonly(dlp, false),
            AccountMeta::new(delegation_record, false),
            AccountMeta::new(delegation_metadata, false),
            AccountMeta::new_readonly(system_program::id(), false),
            AccountMeta::new(buffer, false),
            AccountMeta::new(ephemeral_vault_ata, false),
            AccountMeta::new_readonly(e_spl, false),
            AccountMeta::new(vault_ata_buffer, false),
            AccountMeta::new(vault_ata_delegation_record, false),
            AccountMeta::new(vault_ata_delegation_metadata, false),
        ],
        data: manifest::program::ManifestInstruction::DelegateMarket.to_vec(),
    }
}
