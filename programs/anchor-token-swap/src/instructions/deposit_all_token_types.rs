use anchor_lang::prelude::*;

use crate::processor;

#[derive(Accounts)]
pub struct DepositAllTokenTypes<'info> {
    ///   0. `[]` Token-swap
    pub token_swap: AccountInfo<'info>,
    ///   1. `[]` swap authority
    pub swap_authority: AccountInfo<'info>,
    ///   2. `[signer]` user transfer authority
    #[account(signer)]
    pub user_transfer_authority: AccountInfo<'info>,
    ///   3. `[writable]` token_a user transfer authority can transfer amount,
    #[account(mut)]
    pub source_a: AccountInfo<'info>,
    ///   4. `[writable]` token_b user transfer authority can transfer amount,
    #[account(mut)]
    pub source_b: AccountInfo<'info>,
    ///   5. `[writable]` token_a Base Account to deposit into.
    #[account(mut)]
    pub token_a: AccountInfo<'info>,
    ///   6. `[writable]` token_b Base Account to deposit into.
    #[account(mut)]
    pub token_b: AccountInfo<'info>,
    ///   7. `[writable]` Pool MINT account, swap authority is the owner.
    #[account(mut)]
    pub pool_mint: AccountInfo<'info>,
    ///   8. `[writable]` Pool Account to deposit the generated tokens, user is the owner.
    #[account(mut)]
    pub destination: AccountInfo<'info>,
    ///   9. '[]` Token program id
    pub token_program: AccountInfo<'info>,
}

///   Deposit both types of tokens into the pool.  The output is a "pool"
///   token representing ownership in the pool. Inputs are converted to
///   the current ratio.
pub fn handler(
    ctx: Context<DepositAllTokenTypes>,
    pool_token_amount: u64,
    maximum_token_a_amount: u64,
    maximum_token_b_amount: u64,
) -> ProgramResult {
    let accounts = [
        ctx.accounts.token_swap.clone(),
        ctx.accounts.swap_authority.clone(),
        ctx.accounts.user_transfer_authority.clone(),
        ctx.accounts.source_a.clone(),
        ctx.accounts.source_b.clone(),
        ctx.accounts.token_a.clone(),
        ctx.accounts.token_b.clone(),
        ctx.accounts.pool_mint.clone(),
        ctx.accounts.destination.clone(),
        ctx.accounts.token_program.clone(),
    ];

    processor::Processor::process_deposit_all_token_types(
        ctx.program_id,
        pool_token_amount,
        maximum_token_a_amount,
        maximum_token_b_amount,
        &accounts,
    )
}
