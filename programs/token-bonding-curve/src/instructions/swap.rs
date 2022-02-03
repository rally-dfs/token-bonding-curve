use anchor_lang::prelude::*;

use crate::processor;

#[derive(Accounts)]
pub struct Swap<'info> {
    ///   0. `[]` Token-swap
    pub token_swap: AccountInfo<'info>,
    ///   1. `[]` swap authority
    pub swap_authority: AccountInfo<'info>,
    ///   2. `[signer]` user transfer authority
    #[account(signer)]
    pub user_transfer_authority: AccountInfo<'info>,
    ///   3. `[writable]` token_(A|B) SOURCE Account, amount is transferable by user transfer authority,
    #[account(mut)]
    pub source: AccountInfo<'info>,
    ///   4. `[writable]` token_(A|B) Base Account to swap INTO.  Must be the SOURCE token.
    #[account(mut)]
    pub swap_source: AccountInfo<'info>,
    ///   5. `[writable]` token_(A|B) Base Account to swap FROM.  Must be the DESTINATION token.
    #[account(mut)]
    pub swap_destination: AccountInfo<'info>,
    ///   6. `[writable]` token_(A|B) DESTINATION Account assigned to USER as the owner.
    #[account(mut)]
    pub destination: AccountInfo<'info>,
    ///   7. `[writable]` Pool token mint, to generate trading fees
    #[account(mut)]
    pub pool_mint: AccountInfo<'info>,
    ///   8. `[writable]` Fee account, to receive trading fees
    #[account(mut)]
    pub pool_fee: AccountInfo<'info>,
    ///   9. '[]` Token program id
    pub token_program: AccountInfo<'info>,
    // TODO:     ///   10 `[optional, writable]` Host fee account to receive additional trading fees
}

///   Swap the tokens in the pool.

pub fn handler(ctx: Context<Swap>, amount_in: u64, minimum_amount_out: u64) -> ProgramResult {
    let accounts = vec![
        ctx.accounts.token_swap.clone(),
        ctx.accounts.swap_authority.clone(),
        ctx.accounts.user_transfer_authority.clone(),
        ctx.accounts.source.clone(),
        ctx.accounts.swap_source.clone(),
        ctx.accounts.swap_destination.clone(),
        ctx.accounts.destination.clone(),
        ctx.accounts.pool_mint.clone(),
        ctx.accounts.pool_fee.clone(),
        ctx.accounts.token_program.clone(),
    ];

    // TODO: figure out optional remaining accounts handling
    // if ctx.remaining_accounts.len() > 0 {
    //     accounts.push(ctx.remaining_accounts[0].clone());
    // }

    processor::Processor::process_swap(ctx.program_id, amount_in, minimum_amount_out, &accounts)
}
