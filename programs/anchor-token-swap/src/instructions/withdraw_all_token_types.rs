use anchor_lang::prelude::*;

use crate::processor;

#[derive(Accounts)]
pub struct WithdrawAllTokenTypes<'info> {
    ///   0. `[]` Token-swap
    pub token_swap: AccountInfo<'info>,
    ///   1. `[]` swap authority
    pub swap_authority: AccountInfo<'info>,
    ///   2. `[]` user transfer authority
    pub user_transfer_authority: AccountInfo<'info>,
    ///   3. `[writable]` token_(A|B) SOURCE Account, amount is transferable by user transfer authority,
    #[account(mut)]
    pub source_token: AccountInfo<'info>,
    ///   3. `[writable]` Pool mint account, swap authority is the owner
    #[account(mut)]
    pub pool_mint: AccountInfo<'info>,
    ///   4. `[writable]` SOURCE Pool account, amount is transferable by user transfer authority.
    #[account(mut)]
    pub source: AccountInfo<'info>,
    ///   5. `[writable]` token_a Swap Account to withdraw FROM.
    #[account(mut)]
    pub swap_token_a: AccountInfo<'info>,
    ///   6. `[writable]` token_b Swap Account to withdraw FROM.
    #[account(mut)]
    pub swap_token_b: AccountInfo<'info>,
    ///   7. `[writable]` token_a user Account to credit.
    #[account(mut)]
    pub destination_token_a: AccountInfo<'info>,
    ///   8. `[writable]` token_b user Account to credit.
    #[account(mut)]
    pub destination_token_b: AccountInfo<'info>,
    ///   9. `[writable]` Fee account, to receive withdrawal fees
    #[account(mut)]
    pub fee_account: AccountInfo<'info>,
    ///   10 '[]` Token program id
    pub token_program: AccountInfo<'info>,
}

///   Withdraw both types of tokens from the pool at the current ratio, given
///   pool tokens.  The pool tokens are burned in exchange for an equivalent
///   amount of token A and B.
pub fn handler(
    ctx: Context<WithdrawAllTokenTypes>,
    pool_token_amount: u64,
    minimum_token_a_amount: u64,
    minimum_token_b_amount: u64,
) -> ProgramResult {
    let accounts = [
        ctx.accounts.token_swap.clone(),
        ctx.accounts.swap_authority.clone(),
        ctx.accounts.user_transfer_authority.clone(),
        ctx.accounts.pool_mint.clone(),
        ctx.accounts.source.clone(),
        ctx.accounts.swap_token_a.clone(),
        ctx.accounts.swap_token_b.clone(),
        ctx.accounts.destination_token_a.clone(),
        ctx.accounts.destination_token_b.clone(),
        ctx.accounts.fee_account.clone(),
        ctx.accounts.token_program.clone(),
    ];

    processor::Processor::process_withdraw_all_token_types(
        ctx.program_id,
        pool_token_amount,
        minimum_token_a_amount,
        minimum_token_b_amount,
        &accounts,
    )
}
