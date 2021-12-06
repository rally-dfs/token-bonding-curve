use anchor_lang::prelude::*;

use crate::processor;

#[derive(Accounts)]
pub struct WithdrawSingleTokenTypeExactAmountOut<'info> {
    ///   0. `[]` Token-swap
    pub token_swap: AccountInfo<'info>,
    ///   1. `[]` swap authority
    pub swap_authority: AccountInfo<'info>,
    ///   2. `[signer]` user transfer authority
    #[account(signer)]
    pub user_transfer_authority: AccountInfo<'info>,
    ///   3. `[writable]` Pool mint account, swap authority is the owner
    #[account(mut)]
    pub pool_mint: AccountInfo<'info>,
    ///   4. `[writable]` SOURCE Pool account, amount is transferable by user transfer authority.
    #[account(mut)]
    pub pool_token_source: AccountInfo<'info>,
    ///   5. `[writable]` token_a Swap Account to potentially withdraw from.
    #[account(mut)]
    pub swap_token_a: AccountInfo<'info>,
    ///   6. `[writable]` token_b Swap Account to potentially withdraw from.
    #[account(mut)]
    pub swap_token_b: AccountInfo<'info>,
    ///   7. `[writable]` token_(A|B) User Account to credit
    #[account(mut)]
    pub destination: AccountInfo<'info>,
    ///   8. `[writable]` Fee account, to receive withdrawal fees
    #[account(mut)]
    pub pool_fee_account: AccountInfo<'info>,
    ///   9. '[]` Token program id
    pub token_program: AccountInfo<'info>,
}

///   Withdraw one token type from the pool at the current ratio given the
///   exact amount out expected.
pub fn handler(
    ctx: Context<WithdrawSingleTokenTypeExactAmountOut>,
    destination_token_amount: u64,
    maximum_pool_token_amount: u64,
) -> ProgramResult {
    // TODO: maybe not the best way to do this probably, kind of defeating the purpose of
    // anchor, but lets us just use process_foo directly
    let accounts = [
        ctx.accounts.token_swap.clone(),
        ctx.accounts.swap_authority.clone(),
        ctx.accounts.user_transfer_authority.clone(),
        ctx.accounts.pool_mint.clone(),
        ctx.accounts.pool_token_source.clone(),
        ctx.accounts.swap_token_a.clone(),
        ctx.accounts.swap_token_b.clone(),
        ctx.accounts.destination.clone(),
        ctx.accounts.pool_fee_account.clone(),
        ctx.accounts.token_program.clone(),
    ];

    processor::Processor::process_withdraw_single_token_type_exact_amount_out(
        ctx.program_id,
        destination_token_amount,
        maximum_pool_token_amount,
        &accounts,
    )
}

/*

let account_info_iter = &mut accounts.iter();
let swap_info = next_account_info(account_info_iter)?;
let authority_info = next_account_info(account_info_iter)?;
let user_transfer_authority_info = next_account_info(account_info_iter)?;
let pool_mint_info = next_account_info(account_info_iter)?;
let source_info = next_account_info(account_info_iter)?;
let swap_token_a_info = next_account_info(account_info_iter)?;
let swap_token_b_info = next_account_info(account_info_iter)?;
let destination_info = next_account_info(account_info_iter)?;
let pool_fee_account_info = next_account_info(account_info_iter)?;
let token_program_info = next_account_info(account_info_iter)?;
 */
