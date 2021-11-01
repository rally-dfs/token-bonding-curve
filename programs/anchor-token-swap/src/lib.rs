//! An Uniswap-like program for the Solana blockchain.

use anchor_lang::prelude::*;

mod instructions;

pub mod constraints;
pub mod curve;
pub mod error;
pub mod instruction_nonanchor;
pub mod processor;
pub mod state;

use instructions::*;

// solana_program::declare_id!("SwaPpA9LAaLfeLi3a68M4DjnLqgtticKg6CnyNwgAC8");
declare_id!("SwaPpA9LAaLfeLi3a68M4DjnLqgtticKg6CnyNwgAC8");

/// documentation
#[program]
mod anchor_token_swap {
    use super::*;

    /// Creates a 'deposit_single_token_type_exact_amount_in' instruction.
    pub fn deposit_single_token_type_exact_amount_in(
        ctx: Context<DepositSingleTokenTypeExactAmountIn>,
        source_token_amount: u64,
        minimum_pool_token_amount: u64,
    ) -> ProgramResult {
        instructions::deposit_single_token_type_exact_amount_in::handler(
            ctx,
            source_token_amount,
            minimum_pool_token_amount,
        )
    }
}
