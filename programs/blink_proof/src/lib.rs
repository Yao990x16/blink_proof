use anchor_lang::declare_program;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::{
    instruction::{AccountMeta, Instruction},
    program::invoke_signed,
};
use solana_keccak_hasher::hashv;

declare_program!(spl_account_compression);

declare_id!("Bi5tyuZ7xG8d718WcP8AhHJpxqADTCkPBTDoS3ncRpiQ");

const TREE_AUTHORITY_SEED: &[u8] = b"tree_authority";
const TREE_CONFIG_SEED: &[u8] = b"tree_config";
const ISSUER_CREDENTIAL_SEED: &[u8] = b"issuer_credential";
const MERKLE_TREE_MAX_DEPTH: u32 = 14;
const MERKLE_TREE_MAX_BUFFER_SIZE: u32 = 64;
const INIT_EMPTY_MERKLE_TREE_DISCRIMINATOR: [u8; 8] = [191, 11, 119, 7, 180, 107, 220, 110];
const LEAF_SCHEMA_VERSION: u8 = 1;

#[program]
pub mod blink_proof {
    use super::*;

    pub fn initialize_tree(ctx: Context<InitializeTree>) -> Result<()> {
        let tree_key = ctx.accounts.merkle_tree.key();
        let signer_seeds: &[&[u8]] = &[
            TREE_AUTHORITY_SEED,
            tree_key.as_ref(),
            &[ctx.bumps.tree_authority],
        ];

        let instruction = Instruction {
            program_id: ctx.accounts.compression_program.key(),
            accounts: vec![
                AccountMeta::new(tree_key, false),
                AccountMeta::new_readonly(ctx.accounts.tree_authority.key(), true),
                AccountMeta::new_readonly(ctx.accounts.log_wrapper.key(), false),
            ],
            data: init_empty_merkle_tree_data(MERKLE_TREE_MAX_DEPTH, MERKLE_TREE_MAX_BUFFER_SIZE),
        };

        invoke_signed(
            &instruction,
            &[
                ctx.accounts.merkle_tree.to_account_info(),
                ctx.accounts.tree_authority.to_account_info(),
                ctx.accounts.log_wrapper.to_account_info(),
                ctx.accounts.compression_program.to_account_info(),
            ],
            &[signer_seeds],
        )?;

        Ok(())
    }

    pub fn create_tree_config(ctx: Context<CreateTreeConfig>, is_public: bool) -> Result<()> {
        let config = &mut ctx.accounts.tree_config;
        config.admin = ctx.accounts.admin.key();
        config.merkle_tree = ctx.accounts.merkle_tree.key();
        config.is_public = is_public;
        config.issuer_count = 0;

        Ok(())
    }

    pub fn authorize_issuer(ctx: Context<AuthorizeIssuer>) -> Result<()> {
        let credential = &mut ctx.accounts.issuer_credential;
        credential.issuer = ctx.accounts.issuer.key();
        credential.merkle_tree = ctx.accounts.tree_config.merkle_tree;
        credential.granted_at = Clock::get()?.unix_timestamp;

        let config = &mut ctx.accounts.tree_config;
        config.issuer_count = config.issuer_count.saturating_add(1);

        Ok(())
    }

    pub fn revoke_issuer(ctx: Context<RevokeIssuer>) -> Result<()> {
        let config = &mut ctx.accounts.tree_config;
        config.issuer_count = config.issuer_count.saturating_sub(1);

        Ok(())
    }

    pub fn register_content(
        ctx: Context<RegisterContent>,
        salted_fingerprint: [u8; 32],
        raw_phash: [u8; 8],
    ) -> Result<()> {
        let tree_key = ctx.accounts.merkle_tree.key();
        let signer_seeds: &[&[u8]] = &[
            TREE_AUTHORITY_SEED,
            tree_key.as_ref(),
            &[ctx.bumps.tree_authority],
        ];

        // The external signer authorizes the content registration request.
        // The PDA then signs the CPI because it is the actual Merkle tree authority.
        let cpi_accounts = spl_account_compression::cpi::accounts::Append {
            merkle_tree: ctx.accounts.merkle_tree.to_account_info(),
            authority: ctx.accounts.tree_authority.to_account_info(),
            noop: ctx.accounts.log_wrapper.to_account_info(),
        };
        let cpi_program = ctx.accounts.compression_program.to_account_info();
        let signer = [signer_seeds];
        let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, &signer);

        ctx.accounts.registration_receipt.salted_fingerprint = salted_fingerprint;

        // Authorization check: if a tree_config exists and is not public,
        // verify the caller has a valid issuer credential for this tree.
        if let Some(config) = &ctx.accounts.tree_config {
            require!(
                config.merkle_tree == ctx.accounts.merkle_tree.key(),
                BlinkProofError::UnauthorizedIssuer
            );

            if !config.is_public {
                let credential = ctx
                    .accounts
                    .issuer_credential
                    .as_ref()
                    .ok_or(error!(BlinkProofError::UnauthorizedIssuer))?;
                require!(
                    credential.issuer == ctx.accounts.authority.key(),
                    BlinkProofError::UnauthorizedIssuer
                );
                require!(
                    credential.merkle_tree == ctx.accounts.merkle_tree.key(),
                    BlinkProofError::UnauthorizedIssuer
                );
            }
        }

        let leaf = versioned_leaf(&salted_fingerprint);
        spl_account_compression::cpi::append(cpi_ctx, leaf)?;
        let clock = Clock::get()?;

        emit!(ContentRegistered {
            creator: ctx.accounts.authority.key(),
            salted_fingerprint,
            raw_phash,
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }

    pub fn verify_content<'info>(
        ctx: Context<'_, '_, '_, 'info, VerifyContent<'info>>,
        root: [u8; 32],
        salted_fingerprint: [u8; 32],
        leaf_index: u32,
        proof: Vec<[u8; 32]>,
    ) -> Result<()> {
        require!(
            proof.len() == ctx.remaining_accounts.len(),
            BlinkProofError::InvalidProof
        );

        for (expected_node, account) in proof.iter().zip(ctx.remaining_accounts.iter()) {
            require!(
                account.key().to_bytes() == *expected_node,
                BlinkProofError::InvalidProof
            );
        }

        let leaf = versioned_leaf(&salted_fingerprint);
        let cpi_accounts = spl_account_compression::cpi::accounts::VerifyLeaf {
            merkle_tree: ctx.accounts.merkle_tree.to_account_info(),
        };
        let cpi_program = ctx.accounts.compression_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts)
            .with_remaining_accounts(ctx.remaining_accounts.to_vec());

        spl_account_compression::cpi::verify_leaf(cpi_ctx, root, leaf, leaf_index)?;

        emit!(ContentVerified {
            verifier: ctx.accounts.verifier.key(),
            salted_fingerprint,
            leaf_index,
            merkle_tree: ctx.accounts.merkle_tree.key(),
        });

        Ok(())
    }
}

#[derive(Accounts)]
pub struct InitializeTree<'info> {
    /// CHECK: The client preallocates this account with the SPL compression program as owner.
    #[account(mut, owner = spl_account_compression::ID)]
    pub merkle_tree: UncheckedAccount<'info>,

    /// CHECK: PDA used as the write authority for the compressed tree.
    #[account(
        seeds = [TREE_AUTHORITY_SEED, merkle_tree.key().as_ref()],
        bump
    )]
    pub tree_authority: UncheckedAccount<'info>,

    /// CHECK: Program address is validated by the constraint.
    #[account(address = spl_account_compression::ID)]
    pub compression_program: UncheckedAccount<'info>,

    /// CHECK: Program address is validated by the constraint.
    #[account(address = spl_noop::id())]
    pub log_wrapper: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct CreateTreeConfig<'info> {
    /// CHECK: The Merkle tree this config governs.
    #[account(owner = spl_account_compression::ID)]
    pub merkle_tree: UncheckedAccount<'info>,

    #[account(
        init,
        payer = admin,
        space = 8 + 32 + 32 + 1 + 4,
        seeds = [TREE_CONFIG_SEED, merkle_tree.key().as_ref()],
        bump
    )]
    pub tree_config: Account<'info, TreeConfig>,

    #[account(mut)]
    pub admin: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AuthorizeIssuer<'info> {
    #[account(
        mut,
        seeds = [TREE_CONFIG_SEED, tree_config.merkle_tree.as_ref()],
        bump,
        has_one = admin
    )]
    pub tree_config: Account<'info, TreeConfig>,

    #[account(
        init,
        payer = admin,
        space = 8 + 32 + 32 + 8,
        seeds = [
            ISSUER_CREDENTIAL_SEED,
            tree_config.merkle_tree.as_ref(),
            issuer.key().as_ref()
        ],
        bump
    )]
    pub issuer_credential: Account<'info, IssuerCredential>,

    /// CHECK: The public key being authorized. Does not need to sign.
    pub issuer: UncheckedAccount<'info>,

    #[account(mut)]
    pub admin: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct RevokeIssuer<'info> {
    #[account(
        mut,
        seeds = [TREE_CONFIG_SEED, tree_config.merkle_tree.as_ref()],
        bump,
        has_one = admin
    )]
    pub tree_config: Account<'info, TreeConfig>,

    #[account(
        mut,
        close = admin,
        seeds = [
            ISSUER_CREDENTIAL_SEED,
            tree_config.merkle_tree.as_ref(),
            issuer_credential.issuer.as_ref()
        ],
        bump
    )]
    pub issuer_credential: Account<'info, IssuerCredential>,

    #[account(mut)]
    pub admin: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(salted_fingerprint: [u8; 32])]
pub struct RegisterContent<'info> {
    /// CHECK: Compression-owned concurrent Merkle tree account.
    #[account(mut, owner = spl_account_compression::ID)]
    pub merkle_tree: UncheckedAccount<'info>,

    /// CHECK: PDA used as the write authority for the compressed tree.
    #[account(
        seeds = [TREE_AUTHORITY_SEED, merkle_tree.key().as_ref()],
        bump
    )]
    pub tree_authority: UncheckedAccount<'info>,

    /// The caller must sign to authorize this content registration request.
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(
        init,
        payer = authority,
        space = 8 + 32,
        seeds = [b"receipt", salted_fingerprint.as_ref()],
        bump
    )]
    pub registration_receipt: Account<'info, RegistrationReceipt>,

    pub system_program: Program<'info, System>,

    /// CHECK: Program address is validated by the constraint.
    #[account(address = spl_account_compression::ID)]
    pub compression_program: UncheckedAccount<'info>,

    /// CHECK: Program address is validated by the constraint.
    #[account(address = spl_noop::id())]
    pub log_wrapper: UncheckedAccount<'info>,

    /// Optional: tree config for permissioned trees. If absent, no authorization check.
    /// CHECK: Validated manually in instruction logic.
    pub tree_config: Option<Account<'info, TreeConfig>>,

    /// Optional: issuer credential for permissioned trees.
    /// CHECK: Validated manually in instruction logic.
    pub issuer_credential: Option<Account<'info, IssuerCredential>>,
}

#[derive(Accounts)]
pub struct VerifyContent<'info> {
    /// CHECK: Compression-owned Merkle tree account.
    #[account(owner = spl_account_compression::ID)]
    pub merkle_tree: UncheckedAccount<'info>,

    /// The user requesting verification.
    pub verifier: Signer<'info>,

    /// CHECK: Program address validated by constraint.
    #[account(address = spl_account_compression::ID)]
    pub compression_program: UncheckedAccount<'info>,
}

#[event]
pub struct ContentRegistered {
    pub creator: Pubkey,
    pub salted_fingerprint: [u8; 32],
    pub raw_phash: [u8; 8],
    pub timestamp: i64,
}

#[event]
pub struct ContentVerified {
    pub verifier: Pubkey,
    pub salted_fingerprint: [u8; 32],
    pub leaf_index: u32,
    pub merkle_tree: Pubkey,
}

fn init_empty_merkle_tree_data(max_depth: u32, max_buffer_size: u32) -> Vec<u8> {
    let mut data = Vec::with_capacity(16);
    data.extend_from_slice(&INIT_EMPTY_MERKLE_TREE_DISCRIMINATOR);
    data.extend_from_slice(&max_depth.to_le_bytes());
    data.extend_from_slice(&max_buffer_size.to_le_bytes());
    data
}

#[account]
pub struct RegistrationReceipt {
    pub salted_fingerprint: [u8; 32],
}

/// Tree configuration account. One per Merkle tree.
/// Determines whether the tree is public (anyone can attest) or permissioned.
#[account]
pub struct TreeConfig {
    /// Administrator who can authorize/revoke issuers.
    pub admin: Pubkey,
    /// The Merkle tree this config governs.
    pub merkle_tree: Pubkey,
    /// If true, anyone can call register_content. If false, only authorized issuers.
    pub is_public: bool,
    /// Number of currently authorized issuers.
    pub issuer_count: u32,
}

/// Issuer credential PDA. Proves a pubkey is authorized to attest on a specific tree.
#[account]
pub struct IssuerCredential {
    /// The authorized issuer's public key.
    pub issuer: Pubkey,
    /// The Merkle tree this credential applies to.
    pub merkle_tree: Pubkey,
    /// Unix timestamp when authorization was granted.
    pub granted_at: i64,
}

#[error_code]
pub enum BlinkProofError {
    #[msg("Only authorized issuers can register content on this tree")]
    UnauthorizedIssuer,
    #[msg("Merkle proof accounts do not match the supplied proof")]
    InvalidProof,
}

/// Hash the schema version together with the fingerprint so future leaf schemas
/// can coexist while preserving a fixed 32-byte compressed tree leaf.
fn versioned_leaf(salted_fingerprint: &[u8; 32]) -> [u8; 32] {
    let hash = hashv(&[&[LEAF_SCHEMA_VERSION], salted_fingerprint.as_ref()]);
    hash.to_bytes()
}
