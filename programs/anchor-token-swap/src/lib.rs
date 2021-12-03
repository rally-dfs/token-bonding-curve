//! An Uniswap-like program for the Solana blockchain.

use anchor_lang::prelude::*;

mod instructions;

pub mod constraints;
pub mod curve;
pub mod error;
pub mod processor;
pub mod state;

use curve::fees::Fees;
use instructions::*;

// solana_program::declare_id!("SwaPpA9LAaLfeLi3a68M4DjnLqgtticKg6CnyNwgAC8");
declare_id!("SwaPpA9LAaLfeLi3a68M4DjnLqgtticKg6CnyNwgAC8");

/// documentation
#[program]
mod anchor_token_swap {
    use super::*;

    ///   Creates an 'initialize' instruction with ConstantPrice curve
    ///   Note that SwapCurve has a dynamic trait so can't be borsh serialized easily, so we just handles
    ///   creating the SwapCurve based on the primitives passed into the different instructions
    pub fn initialize_constant_price(
        ctx: Context<Initialize>,
        // TODO: should be able to just accept Fees in here instead of all these but wasn't working, not sure why
        trade_fee_numerator: u64,
        trade_fee_denominator: u64,
        owner_trade_fee_numerator: u64,
        owner_trade_fee_denominator: u64,
        owner_withdraw_fee_numerator: u64,
        owner_withdraw_fee_denominator: u64,
        host_fee_numerator: u64,
        host_fee_denominator: u64,
        token_b_price: u64,
    ) -> ProgramResult {
        instructions::initialize::handler(
            ctx,
            Fees {
                trade_fee_numerator,
                trade_fee_denominator,
                owner_trade_fee_numerator,
                owner_trade_fee_denominator,
                owner_withdraw_fee_numerator,
                owner_withdraw_fee_denominator,
                host_fee_numerator,
                host_fee_denominator,
            },
            curve::base::SwapCurve {
                curve_type: curve::base::CurveType::ConstantPrice,
                calculator: Box::new(curve::constant_price::ConstantPriceCurve { token_b_price }),
            },
        )
    }

    ///   Creates an 'initialize' instruction with LinearPrice curve
    ///   Note that SwapCurve has a dynamic trait so can't be borsh serialized easily, so we just handles
    ///   creating the SwapCurve based on the primitives passed into the different instructions
    pub fn initialize_linear_price(
        ctx: Context<Initialize>,
        slope_numerator: u64,
        slope_denominator: u64,
        initial_token_a_price: u64,
        initial_token_b_price: u64,
    ) -> ProgramResult {
        // just hardcode fees to 0 for linear curve, we don't support those right now (would require implementing
        // some withdraw logic to calculate the fees during swap)
        instructions::initialize::handler(
            ctx,
            Fees {
                trade_fee_numerator: 0,
                trade_fee_denominator: 1,
                owner_trade_fee_numerator: 0,
                owner_trade_fee_denominator: 1,
                owner_withdraw_fee_numerator: 0,
                owner_withdraw_fee_denominator: 1,
                host_fee_numerator: 0,
                host_fee_denominator: 1,
            },
            curve::base::SwapCurve {
                curve_type: curve::base::CurveType::LinearPrice,
                calculator: Box::new(curve::linear_price::LinearPriceCurve {
                    slope_numerator,
                    slope_denominator,
                    initial_token_r_price: initial_token_a_price,
                    initial_token_c_price: initial_token_b_price,
                }),
            },
        )
    }

    /// Creates a 'swap' instruction.
    pub fn swap(ctx: Context<Swap>, amount_in: u64, minimum_amount_out: u64) -> ProgramResult {
        instructions::swap::handler(ctx, amount_in, minimum_amount_out)
    }

    /// Creates a 'deposit_all_token_types' instruction.
    pub fn deposit_all_token_types(
        ctx: Context<DepositAllTokenTypes>,
        pool_token_amount: u64,
        maximum_token_a_amount: u64,
        maximum_token_b_amount: u64,
    ) -> ProgramResult {
        instructions::deposit_all_token_types::handler(
            ctx,
            pool_token_amount,
            maximum_token_a_amount,
            maximum_token_b_amount,
        )
    }

    /// Creates a 'withdraw_all_token_types' instruction.
    pub fn withdraw_all_token_types(
        ctx: Context<WithdrawAllTokenTypes>,
        pool_token_amount: u64,
        minimum_token_a_amount: u64,
        minimum_token_b_amount: u64,
    ) -> ProgramResult {
        instructions::withdraw_all_token_types::handler(
            ctx,
            pool_token_amount,
            minimum_token_a_amount,
            minimum_token_b_amount,
        )
    }

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

    /// Creates a 'deposit_single_token_type_exact_amount_in' instruction.
    pub fn withdraw_single_token_type_exact_amount_out(
        ctx: Context<WithdrawSingleTokenTypeExactAmountOut>,
        destination_token_amount: u64,
        maximum_pool_token_amount: u64,
    ) -> ProgramResult {
        instructions::withdraw_single_token_type_exact_amount_out::handler(
            ctx,
            destination_token_amount,
            maximum_pool_token_amount,
        )
    }
}
