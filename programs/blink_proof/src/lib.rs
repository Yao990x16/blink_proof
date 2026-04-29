use anchor_lang::prelude::*;
use anchor_lang::declare_program;
use anchor_lang::solana_program::{
    instruction::{AccountMeta, Instruction},
    program::invoke_signed,
};

declare_program!(spl_account_compression);

declare_id!("Bi5tyuZ7xG8d718WcP8AhHJpxqADTCkPBTDoS3ncRpiQ");

const TREE_AUTHORITY_SEED: &[u8] = b"tree_authority";
const MERKLE_TREE_MAX_DEPTH: u32 = 14;
const MERKLE_TREE_MAX_BUFFER_SIZE: u32 = 64;
const INIT_EMPTY_MERKLE_TREE_DISCRIMINATOR: [u8; 8] = [191, 11, 119, 7, 180, 107, 220, 110];

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
            data: init_empty_merkle_tree_data(
                MERKLE_TREE_MAX_DEPTH,
                MERKLE_TREE_MAX_BUFFER_SIZE,
            ),
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

    pub fn register_content(
        ctx: Context<RegisterContent>,
        args: RegisterContentArgs,
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

        spl_account_compression::cpi::append(cpi_ctx, args.content_hash)?;
        let clock = Clock::get()?;

        emit!(ContentRegistered {
            creator: ctx.accounts.authority.key(),
            content_hash: args.content_hash,
            timestamp: clock.unix_timestamp,
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
    pub authority: Signer<'info>,

    /// CHECK: Program address is validated by the constraint.
    #[account(address = spl_account_compression::ID)]
    pub compression_program: UncheckedAccount<'info>,

    /// CHECK: Program address is validated by the constraint.
    #[account(address = spl_noop::id())]
    pub log_wrapper: UncheckedAccount<'info>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct RegisterContentArgs {
    pub content_hash: [u8; 32],
}

#[event]
pub struct ContentRegistered {
    pub creator: Pubkey,
    pub content_hash: [u8; 32],
    pub timestamp: i64,
}

fn init_empty_merkle_tree_data(max_depth: u32, max_buffer_size: u32) -> Vec<u8> {
    let mut data = Vec::with_capacity(16);
    data.extend_from_slice(&INIT_EMPTY_MERKLE_TREE_DISCRIMINATOR);
    data.extend_from_slice(&max_depth.to_le_bytes());
    data.extend_from_slice(&max_buffer_size.to_le_bytes());
    data
}
