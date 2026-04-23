use anchor_lang::prelude::*;

declare_id!("Bi5tyuZ7xG8d718WcP8AhHJpxqADTCkPBTDoS3ncRpiQ");

#[program]
pub mod blink_proof {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        msg!("Greetings from: {:?}", ctx.program_id);
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize {}
