use solana_sdk::{
    hash::{hash, Hash},
    instruction::{AccountMeta, Instruction},
    message::Message,
    pubkey::Pubkey,
    transaction::Transaction,
};
use solana_system_interface::program as system_program;

const TREE_AUTHORITY_SEED: &[u8] = b"tree_authority";
const REGISTER_CONTENT_DATA_LEN: usize = 48;

#[derive(Clone)]
pub struct BlinkProofConfig {
    pub program_id: Pubkey,
    pub compression_program_id: Pubkey,
    pub noop_program_id: Pubkey,
    pub merkle_tree: Pubkey,
}

pub fn build_register_content_transaction(
    config: &BlinkProofConfig,
    account: Pubkey,
    salted_fingerprint: [u8; 32],
    raw_phash: [u8; 8],
    recent_blockhash: Hash,
) -> Transaction {
    let instruction =
        build_register_content_instruction(config, account, salted_fingerprint, raw_phash);
    let message = Message::new_with_blockhash(&[instruction], Some(&account), &recent_blockhash);

    Transaction::new_unsigned(message)
}

pub fn build_register_content_instruction(
    config: &BlinkProofConfig,
    authority: Pubkey,
    salted_fingerprint: [u8; 32],
    raw_phash: [u8; 8],
) -> Instruction {
    let (tree_authority, _) = Pubkey::find_program_address(
        &[TREE_AUTHORITY_SEED, config.merkle_tree.as_ref()],
        &config.program_id,
    );
    let (registration_receipt, _) = Pubkey::find_program_address(
        &[b"receipt", salted_fingerprint.as_ref()],
        &config.program_id,
    );

    Instruction {
        program_id: config.program_id,
        accounts: vec![
            AccountMeta::new(config.merkle_tree, false),
            AccountMeta::new_readonly(tree_authority, false),
            AccountMeta::new(authority, true),
            AccountMeta::new(registration_receipt, false),
            AccountMeta::new_readonly(system_program::ID, false),
            AccountMeta::new_readonly(config.compression_program_id, false),
            AccountMeta::new_readonly(config.noop_program_id, false),
            // Optional tree_config account (pass program_id if None)
            AccountMeta::new_readonly(config.program_id, false),
            // Optional issuer_credential account (pass program_id if None)
            AccountMeta::new_readonly(config.program_id, false),
        ],
        data: build_register_content_data(salted_fingerprint, raw_phash),
    }
}

fn build_register_content_data(salted_fingerprint: [u8; 32], raw_phash: [u8; 8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(REGISTER_CONTENT_DATA_LEN);
    data.extend_from_slice(&anchor_instruction_discriminator("register_content"));
    data.extend_from_slice(&salted_fingerprint);
    data.extend_from_slice(&raw_phash);
    data
}

/// Compute the Anchor instruction discriminator for the given instruction name.
///
/// Anchor defines the discriminator as: `SHA-256("global:<ix_name>")[..8]`
/// `solana_sdk::hash::hash()` wraps SHA-256 internally, so the result is
/// identical to what Anchor generates at compile time. This is verified by
/// matching the on-chain program's IDL-derived discriminators.
fn anchor_instruction_discriminator(name: &str) -> [u8; 8] {
    let preimage = format!("global:{name}");
    let digest = hash(preimage.as_bytes()).to_bytes();
    let mut discriminator = [0u8; 8];
    discriminator.copy_from_slice(&digest[..8]);
    discriminator
}
