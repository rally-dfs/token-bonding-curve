use anchor_lang::prelude::*;

use crate::constraints::SWAP_CONSTRAINTS;
use crate::curve::{base::SwapCurve, fees::Fees};
use crate::processor;

// TODO: we're just using AccountInfo below for token_swap but in theory we should make it a ProgramAccount and rewrite SwapV1 to derive from anchor
// (this will make it backward incompatible though with regular spl-token-swap since anchor will init data into those accounts)

///   Initializes a new swap
#[derive(Accounts)]
pub struct Initialize<'info> {
    ///   0. `[writable, signer]` New Token-swap to create.
    #[account(mut, signer)]
    pub token_swap: AccountInfo<'info>,
    ///   1. `[]` swap authority derived from `create_program_address(&[Token-swap account])`
    pub swap_authority: AccountInfo<'info>,
    ///   2. `[]` token_a Account. Must be non zero, owned by swap authority.
    pub token_a: AccountInfo<'info>,
    ///   3. `[]` token_b Account. Must be non zero, owned by swap authority.
    pub token_b: AccountInfo<'info>,
    ///   4. `[writable]` Pool Token Mint. Must be empty, owned by swap authority. Freeze authority must be null.
    #[account(mut)]
    pub pool: AccountInfo<'info>,
    ///   5. `[]` Pool Token Account to deposit trading and withdraw fees.
    ///   Must be empty, not owned by swap authority
    pub fee: AccountInfo<'info>,
    ///   6. `[writable]` Pool Token Account to deposit the initial pool token
    ///   supply.  Must be empty, not owned by swap authority.
    #[account(mut)]
    pub destination: AccountInfo<'info>,
    ///   7. '[]` Token program id
    pub token_program: AccountInfo<'info>,
}

///   Initializes a new swap
///   Note that SwapCurve has a dynamic trait so can't be borsh serialized easily, so lib.rs just handles
///   creating the SwapCurve based on the primitives passed into the different instructions
pub fn handler(ctx: Context<Initialize>, fees: Fees, swap_curve: SwapCurve) -> ProgramResult {
    let accounts = [
        ctx.accounts.token_swap.clone(),
        ctx.accounts.swap_authority.clone(),
        ctx.accounts.token_a.clone(),
        ctx.accounts.token_b.clone(),
        ctx.accounts.pool.clone(),
        ctx.accounts.fee.clone(),
        ctx.accounts.destination.clone(),
        ctx.accounts.token_program.clone(),
    ];
    processor::Processor::process_initialize(
        ctx.program_id,
        fees,
        swap_curve,
        &accounts,
        &SWAP_CONSTRAINTS,
    )
}
