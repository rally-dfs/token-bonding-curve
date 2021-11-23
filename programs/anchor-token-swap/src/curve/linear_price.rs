//! Linear price swap curve, slope and initial price point set at init
use {
    crate::{
        curve::calculator::{
            map_zero_to_none, CurveCalculator, DynPack, RoundDirection, SwapWithoutFeesResult,
            TradeDirection, TradingTokenResult,
        },
        error::SwapError,
    },
    arrayref::{array_mut_ref, array_ref},
    solana_program::{
        program_error::ProgramError,
        program_pack::{IsInitialized, Pack, Sealed},
    },
    spl_math::{checked_ceil_div::CheckedCeilDiv, precise_number::PreciseNumber, uint::U256},
};

/// LinearPriceCurve struct implementing CurveCalculator
/// R is the "collateral" token (e.g. RLY), C is the "bonded" token (e.g. TAKI)
/// Price of a single C token (r, denominated in R) is defined by `r - initial_token_r_price = slope*(c - initial_token_c_price)`
/// where c is the amount of C that’s backed by R
/// TODO: rename all these to token A and token B, just using r and c temporarily while writing this
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinearPriceCurve {
    /// Slope of price increase (how much price of token B increases for every token A that's bonded to it) numerator
    pub slope_numerator: u64,
    /// Slope of price increase (how much price of token B increases for every token A that's bonded to it) denominator
    pub slope_denominator: u64,
    /// When there's 0 liquidity in the pool, what should the initial price point (c0,r0) defining the curve be?
    pub initial_token_r_price: u64, // AKA token A
    /// When there's 0 liquidity in the pool, what should the initial price point (c0,r0) defining the curve be?
    pub initial_token_c_price: u64, // AKA token B
}

impl CurveCalculator for LinearPriceCurve {
    /// TODO:
    /// Calculate how much destination token will be provided given an amount
    /// of source token.
    fn swap_without_fees(
        &self,
        source_amount: u128,
        swap_source_amount: u128,
        swap_destination_amount: u128,
        trade_direction: TradeDirection,
    ) -> Option<SwapWithoutFeesResult> {
        None
        // TODO: this is constant curve impl:
        // let token_c_price = self.token_c_price as u128;

        // let (source_amount_swapped, destination_amount_swapped) = match trade_direction {
        //     TradeDirection::BtoA => (source_amount, source_amount.checked_mul(token_c_price)?),
        //     TradeDirection::AtoB => {
        //         let destination_amount_swapped = source_amount.checked_div(token_c_price)?;
        //         let mut source_amount_swapped = source_amount;

        //         // if there is a remainder from buying token B, floor
        //         // token_r_amount to avoid taking too many tokens, but
        //         // don't recalculate the fees
        //         let remainder = source_amount_swapped.checked_rem(token_c_price)?;
        //         if remainder > 0 {
        //             source_amount_swapped = source_amount.checked_sub(remainder)?;
        //         }

        //         (source_amount_swapped, destination_amount_swapped)
        //     }
        // };
        // let source_amount_swapped = map_zero_to_none(source_amount_swapped)?;
        // let destination_amount_swapped = map_zero_to_none(destination_amount_swapped)?;
        // Some(SwapWithoutFeesResult {
        //     source_amount_swapped,
        //     destination_amount_swapped,
        // })
    }

    /// Get the amount of trading tokens for the given amount of pool tokens,
    /// provided the total trading tokens and supply of pool tokens.
    /// TODO: this isn't needed if we disable deposit/withdraw, otherwise
    /// we need it to determine how many pool tokens deposit_all_token_types mints out
    /// (given a max limit of A and B) or how many pool tokens
    /// withdraw_all_token_types burns (given a min limit of A and B)
    fn pool_tokens_to_trading_tokens(
        &self,
        pool_tokens: u128,
        pool_token_supply: u128,
        swap_token_r_amount: u128,
        swap_token_c_amount: u128,
        round_direction: RoundDirection,
    ) -> Option<TradingTokenResult> {
        None
        // TODO: this is constant curve impl:
        // let token_c_price = self.token_c_price as u128;
        // let total_value = self
        //     .normalized_value(swap_token_r_amount, swap_token_c_amount)?
        //     .to_imprecise()?;

        // let (token_r_amount, token_c_amount) = match round_direction {
        //     RoundDirection::Floor => {
        //         let token_r_amount = pool_tokens
        //             .checked_mul(total_value)?
        //             .checked_div(pool_token_supply)?;
        //         let token_c_amount = pool_tokens
        //             .checked_mul(total_value)?
        //             .checked_div(token_c_price)?
        //             .checked_div(pool_token_supply)?;
        //         (token_r_amount, token_c_amount)
        //     }
        //     RoundDirection::Ceiling => {
        //         let (token_r_amount, _) = pool_tokens
        //             .checked_mul(total_value)?
        //             .checked_ceil_div(pool_token_supply)?;
        //         let (pool_value_as_token_c, _) = pool_tokens
        //             .checked_mul(total_value)?
        //             .checked_ceil_div(token_c_price)?;
        //         let (token_c_amount, _) =
        //             pool_value_as_token_c.checked_ceil_div(pool_token_supply)?;
        //         (token_r_amount, token_c_amount)
        //     }
        // };
        // Some(TradingTokenResult {
        //     token_r_amount,
        //     token_c_amount,
        // })
    }

    /// Get the amount of pool tokens for the given amount of token A and B
    /// TODO: this isn't needed if we disable deposits, otherwise
    /// it's used in deposit_single_token_type_exact_amount_in to determine
    /// how much pool token to mint (given a trading token amount and a minimum_pool_token_rmount)
    fn deposit_single_token_type(
        &self,
        source_amount: u128,
        swap_token_r_amount: u128,
        swap_token_c_amount: u128,
        pool_supply: u128,
        trade_direction: TradeDirection,
    ) -> Option<u128> {
        None
        // TODO: this is constant curve impl:
        // trading_tokens_to_pool_tokens(
        //     self.token_c_price,
        //     source_amount,
        //     swap_token_r_amount,
        //     swap_token_c_amount,
        //     pool_supply,
        //     trade_direction,
        //     RoundDirection::Floor,
        // )
    }

    /// Get the amount of pool tokens for the withdrawn amount of token A or B.
    /// TODO: this mostly isn't needed if we disable withdrawals, UNLESS we have
    /// non-zero host fees/trade fees, in which case it's used in `swap` to determine
    /// how much pool token to mint (to account for fees) into the various fee accounts
    fn withdraw_single_token_type_exact_out(
        &self,
        source_amount: u128,
        swap_token_r_amount: u128,
        swap_token_c_amount: u128,
        pool_supply: u128,
        trade_direction: TradeDirection,
    ) -> Option<u128> {
        None
        // TODO: this is constant curve impl:
        // trading_tokens_to_pool_tokens(
        //     self.token_c_price,
        //     source_amount,
        //     swap_token_r_amount,
        //     swap_token_c_amount,
        //     pool_supply,
        //     trade_direction,
        //     RoundDirection::Ceiling,
        // )
    }

    /// Validate that the given curve has no invalid parameters
    /// Called on `initialize` - slope must be positive but initial point can be (0,0)
    fn validate(&self) -> Result<(), SwapError> {
        if self.slope_numerator == 0 || self.slope_denominator == 0 {
            Err(SwapError::InvalidCurve)
        } else {
            Ok(())
        }
    }

    /// Validate the given supply on initialization.
    /// We require at least some bonded token B for the curve to be useful (collateral token can be 0)
    /// TODO: if we enable deposits, then this check isn't needed, the pool can start with 0 of both
    fn validate_supply(&self, _token_r_amount: u64, token_c_amount: u64) -> Result<(), SwapError> {
        if token_c_amount == 0 {
            return Err(SwapError::EmptySupply);
        }
        Ok(())
    }

    /// TODO: we can explore enabling deposits if we resolve all the above functions
    /// that affect deposits
    /// (can still be independent of withdrawals - the latter requires amending CurveCalculator
    /// to add an allows_withdrawals function too)
    fn allows_deposits(&self) -> bool {
        false
    }

    /// The total normalized value of the constant price curve adds the total
    /// value of the token B side to the token A side.
    /// TODO: i think this is just used in tests
    fn normalized_value(
        &self,
        swap_token_r_amount: u128,
        swap_token_c_amount: u128,
    ) -> Option<PreciseNumber> {
        None
        // TODO: this is constant curve impl:
        // let swap_token_c_value = swap_token_c_amount.checked_mul(self.token_c_price as u128)?;
        // // special logic in case we're close to the limits, avoid overflowing u128
        // let value = if swap_token_c_value.saturating_sub(std::u64::MAX.into())
        //     > (std::u128::MAX.saturating_sub(std::u64::MAX.into()))
        // {
        //     swap_token_c_value
        //         .checked_div(2)?
        //         .checked_add(swap_token_r_amount.checked_div(2)?)?
        // } else {
        //     swap_token_r_amount
        //         .checked_add(swap_token_c_value)?
        //         .checked_div(2)?
        // };
        // PreciseNumber::new(value)
    }
}

/// IsInitialized is required to use `Pack::pack` and `Pack::unpack`
impl IsInitialized for LinearPriceCurve {
    fn is_initialized(&self) -> bool {
        true
    }
}
impl Sealed for LinearPriceCurve {}
impl Pack for LinearPriceCurve {
    const LEN: usize = 32;
    fn pack_into_slice(&self, output: &mut [u8]) {
        (self as &dyn DynPack).pack_into_slice(output);
    }

    fn unpack_from_slice(input: &[u8]) -> Result<LinearPriceCurve, ProgramError> {
        let slope_numerator = array_ref![input, 0, 8];
        let slope_denominator = array_ref![input, 8, 8];
        let initial_token_r_price = array_ref![input, 16, 8];
        let initial_token_c_price = array_ref![input, 24, 8];
        Ok(Self {
            slope_numerator: u64::from_le_bytes(*slope_numerator),
            slope_denominator: u64::from_le_bytes(*slope_denominator),
            initial_token_r_price: u64::from_le_bytes(*initial_token_r_price),
            initial_token_c_price: u64::from_le_bytes(*initial_token_c_price),
        })
    }
}

impl DynPack for LinearPriceCurve {
    fn pack_into_slice(&self, output: &mut [u8]) {
        let slope_numerator = array_mut_ref![output, 0, 8];
        *slope_numerator = self.slope_numerator.to_le_bytes();
        let slope_denominator = array_mut_ref![output, 8, 8];
        *slope_denominator = self.slope_denominator.to_le_bytes();
        let initial_token_r_price = array_mut_ref![output, 16, 8];
        *initial_token_r_price = self.initial_token_r_price.to_le_bytes();
        let initial_token_c_price = array_mut_ref![output, 24, 8];
        *initial_token_c_price = self.initial_token_c_price.to_le_bytes();
    }
}

// TODO: reenable these tests, add some specific ones for linear curve math
// #[cfg(test)]
// mod tests {
//     use super::*;
//     use crate::curve::calculator::{
//         test::{
//             check_curve_value_from_swap, check_deposit_token_conversion,
//             check_withdraw_token_conversion, total_and_intermediate,
//             CONVERSION_BASIS_POINTS_GUARANTEE,
//         },
//         INITIAL_SWAP_POOL_AMOUNT,
//     };
//     use proptest::prelude::*;

//     #[test]
//     fn swap_calculation_no_price() {
//         let swap_source_amount: u128 = 0;
//         let swap_destination_amount: u128 = 0;
//         let source_amount: u128 = 100;
//         let token_b_price = 1;
//         let curve = ConstantPriceCurve { token_b_price };

//         let expected_result = SwapWithoutFeesResult {
//             source_amount_swapped: source_amount,
//             destination_amount_swapped: source_amount,
//         };

//         let result = curve
//             .swap_without_fees(
//                 source_amount,
//                 swap_source_amount,
//                 swap_destination_amount,
//                 TradeDirection::AtoB,
//             )
//             .unwrap();
//         assert_eq!(result, expected_result);

//         let result = curve
//             .swap_without_fees(
//                 source_amount,
//                 swap_source_amount,
//                 swap_destination_amount,
//                 TradeDirection::BtoA,
//             )
//             .unwrap();
//         assert_eq!(result, expected_result);
//     }

//     #[test]
//     fn pack_flat_curve() {
//         let token_b_price = 1_251_258;
//         let curve = ConstantPriceCurve { token_b_price };

//         let mut packed = [0u8; ConstantPriceCurve::LEN];
//         Pack::pack_into_slice(&curve, &mut packed[..]);
//         let unpacked = ConstantPriceCurve::unpack(&packed).unwrap();
//         assert_eq!(curve, unpacked);

//         let mut packed = vec![];
//         packed.extend_from_slice(&token_b_price.to_le_bytes());
//         let unpacked = ConstantPriceCurve::unpack(&packed).unwrap();
//         assert_eq!(curve, unpacked);
//     }

//     #[test]
//     fn swap_calculation_large_price() {
//         let token_b_price = 1123513u128;
//         let curve = ConstantPriceCurve {
//             token_b_price: token_b_price as u64,
//         };
//         let token_b_amount = 500u128;
//         let token_a_amount = token_b_amount * token_b_price;
//         let bad_result = curve.swap_without_fees(
//             token_b_price - 1u128,
//             token_a_amount,
//             token_b_amount,
//             TradeDirection::AtoB,
//         );
//         assert!(bad_result.is_none());
//         let bad_result =
//             curve.swap_without_fees(1u128, token_a_amount, token_b_amount, TradeDirection::AtoB);
//         assert!(bad_result.is_none());
//         let result = curve
//             .swap_without_fees(
//                 token_b_price,
//                 token_a_amount,
//                 token_b_amount,
//                 TradeDirection::AtoB,
//             )
//             .unwrap();
//         assert_eq!(result.source_amount_swapped, token_b_price);
//         assert_eq!(result.destination_amount_swapped, 1u128);
//     }

//     #[test]
//     fn swap_calculation_max_min() {
//         let token_b_price = u64::MAX as u128;
//         let curve = ConstantPriceCurve {
//             token_b_price: token_b_price as u64,
//         };
//         let token_b_amount = 1u128;
//         let token_a_amount = token_b_price;
//         let bad_result = curve.swap_without_fees(
//             token_b_price - 1u128,
//             token_a_amount,
//             token_b_amount,
//             TradeDirection::AtoB,
//         );
//         assert!(bad_result.is_none());
//         let bad_result =
//             curve.swap_without_fees(1u128, token_a_amount, token_b_amount, TradeDirection::AtoB);
//         assert!(bad_result.is_none());
//         let bad_result =
//             curve.swap_without_fees(0u128, token_a_amount, token_b_amount, TradeDirection::AtoB);
//         assert!(bad_result.is_none());
//         let result = curve
//             .swap_without_fees(
//                 token_b_price,
//                 token_a_amount,
//                 token_b_amount,
//                 TradeDirection::AtoB,
//             )
//             .unwrap();
//         assert_eq!(result.source_amount_swapped, token_b_price);
//         assert_eq!(result.destination_amount_swapped, 1u128);
//     }

//     proptest! {
//         #[test]
//         fn deposit_token_conversion_a_to_b(
//             // in the pool token conversion calcs, we simulate trading half of
//             // source_token_amount, so this needs to be at least 2
//             source_token_amount in 2..u64::MAX,
//             swap_source_amount in 1..u64::MAX,
//             swap_destination_amount in 1..u64::MAX,
//             pool_supply in INITIAL_SWAP_POOL_AMOUNT..u64::MAX as u128,
//             token_b_price in 1..u64::MAX,
//         ) {
//             let traded_source_amount = source_token_amount / 2;
//             // Make sure that the trade yields at least 1 token B
//             prop_assume!(traded_source_amount / token_b_price >= 1);
//             // Make sure there's enough tokens to get back on the other side
//             prop_assume!(traded_source_amount / token_b_price <= swap_destination_amount);

//             let curve = ConstantPriceCurve {
//                 token_b_price,
//             };
//             check_deposit_token_conversion(
//                 &curve,
//                 source_token_amount as u128,
//                 swap_source_amount as u128,
//                 swap_destination_amount as u128,
//                 TradeDirection::AtoB,
//                 pool_supply,
//                 CONVERSION_BASIS_POINTS_GUARANTEE,
//             );
//         }
//     }

//     proptest! {
//         #[test]
//         fn deposit_token_conversion_b_to_a(
//             // in the pool token conversion calcs, we simulate trading half of
//             // source_token_amount, so this needs to be at least 2
//             source_token_amount in 2..u32::MAX, // kept small to avoid proptest rejections
//             swap_source_amount in 1..u64::MAX,
//             swap_destination_amount in 1..u64::MAX,
//             pool_supply in INITIAL_SWAP_POOL_AMOUNT..u64::MAX as u128,
//             token_b_price in 1..u32::MAX, // kept small to avoid proptest rejections
//         ) {
//             let curve = ConstantPriceCurve {
//                 token_b_price: token_b_price as u64,
//             };
//             let token_b_price = token_b_price as u128;
//             let source_token_amount = source_token_amount as u128;
//             let swap_source_amount = swap_source_amount as u128;
//             let swap_destination_amount = swap_destination_amount as u128;
//             // The constant price curve needs to have enough destination amount
//             // on the other side to complete the swap
//             prop_assume!(token_b_price * source_token_amount / 2 <= swap_destination_amount);

//             check_deposit_token_conversion(
//                 &curve,
//                 source_token_amount,
//                 swap_source_amount,
//                 swap_destination_amount,
//                 TradeDirection::BtoA,
//                 pool_supply,
//                 CONVERSION_BASIS_POINTS_GUARANTEE,
//             );
//         }
//     }

//     proptest! {
//         #[test]
//         fn withdraw_token_conversion(
//             (pool_token_supply, pool_token_amount) in total_and_intermediate(),
//             swap_token_a_amount in 1..u64::MAX,
//             swap_token_b_amount in 1..u32::MAX, // kept small to avoid proptest rejections
//             token_b_price in 1..u32::MAX, // kept small to avoid proptest rejections
//         ) {
//             let curve = ConstantPriceCurve {
//                 token_b_price: token_b_price as u64,
//             };
//             let token_b_price = token_b_price as u128;
//             let pool_token_amount = pool_token_amount as u128;
//             let pool_token_supply = pool_token_supply as u128;
//             let swap_token_a_amount = swap_token_a_amount as u128;
//             let swap_token_b_amount = swap_token_b_amount as u128;

//             let value = curve.normalized_value(swap_token_a_amount, swap_token_b_amount).unwrap();

//             // Make sure we trade at least one of each token
//             prop_assume!(pool_token_amount * value.to_imprecise().unwrap() >= 2 * token_b_price * pool_token_supply);

//             let withdraw_result = curve
//                 .pool_tokens_to_trading_tokens(
//                     pool_token_amount,
//                     pool_token_supply,
//                     swap_token_a_amount,
//                     swap_token_b_amount,
//                     RoundDirection::Floor,
//                 )
//                 .unwrap();
//             prop_assume!(withdraw_result.token_a_amount <= swap_token_a_amount);
//             prop_assume!(withdraw_result.token_b_amount <= swap_token_b_amount);

//             check_withdraw_token_conversion(
//                 &curve,
//                 pool_token_amount,
//                 pool_token_supply,
//                 swap_token_a_amount,
//                 swap_token_b_amount,
//                 TradeDirection::AtoB,
//                 CONVERSION_BASIS_POINTS_GUARANTEE
//             );
//             check_withdraw_token_conversion(
//                 &curve,
//                 pool_token_amount,
//                 pool_token_supply,
//                 swap_token_a_amount,
//                 swap_token_b_amount,
//                 TradeDirection::BtoA,
//                 CONVERSION_BASIS_POINTS_GUARANTEE
//             );
//         }
//     }

//     proptest! {
//         #[test]
//         fn curve_value_does_not_decrease_from_swap_a_to_b(
//             source_token_amount in 1..u64::MAX,
//             swap_source_amount in 1..u64::MAX,
//             swap_destination_amount in 1..u64::MAX,
//             token_b_price in 1..u64::MAX,
//         ) {
//             // Make sure that the trade yields at least 1 token B
//             prop_assume!(source_token_amount / token_b_price >= 1);
//             // Make sure there's enough tokens to get back on the other side
//             prop_assume!(source_token_amount / token_b_price <= swap_destination_amount);
//             let curve = ConstantPriceCurve { token_b_price };
//             check_curve_value_from_swap(
//                 &curve,
//                 source_token_amount as u128,
//                 swap_source_amount as u128,
//                 swap_destination_amount as u128,
//                 TradeDirection::AtoB
//             );
//         }
//     }

//     proptest! {
//         #[test]
//         fn curve_value_does_not_decrease_from_swap_b_to_a(
//             source_token_amount in 1..u32::MAX, // kept small to avoid proptest rejections
//             swap_source_amount in 1..u64::MAX,
//             swap_destination_amount in 1..u64::MAX,
//             token_b_price in 1..u32::MAX, // kept small to avoid proptest rejections
//         ) {
//             // The constant price curve needs to have enough destination amount
//             // on the other side to complete the swap
//             let curve = ConstantPriceCurve { token_b_price: token_b_price as u64 };
//             let token_b_price = token_b_price as u128;
//             let source_token_amount = source_token_amount as u128;
//             let swap_destination_amount = swap_destination_amount as u128;
//             let swap_source_amount = swap_source_amount as u128;
//             // The constant price curve needs to have enough destination amount
//             // on the other side to complete the swap
//             prop_assume!(token_b_price * source_token_amount <= swap_destination_amount);
//             check_curve_value_from_swap(
//                 &curve,
//                 source_token_amount,
//                 swap_source_amount,
//                 swap_destination_amount,
//                 TradeDirection::BtoA
//             );
//         }
//     }

//     proptest! {
//         #[test]
//         fn curve_value_does_not_decrease_from_deposit(
//             pool_token_amount in 2..u64::MAX, // minimum 2 to splitting on deposit
//             pool_token_supply in INITIAL_SWAP_POOL_AMOUNT..u64::MAX as u128,
//             swap_token_a_amount in 1..u64::MAX,
//             swap_token_b_amount in 1..u32::MAX, // kept small to avoid proptest rejections
//             token_b_price in 1..u32::MAX, // kept small to avoid proptest rejections
//         ) {
//             let curve = ConstantPriceCurve { token_b_price: token_b_price as u64 };
//             let pool_token_amount = pool_token_amount as u128;
//             let pool_token_supply = pool_token_supply as u128;
//             let swap_token_a_amount = swap_token_a_amount as u128;
//             let swap_token_b_amount = swap_token_b_amount as u128;
//             let token_b_price = token_b_price as u128;

//             let value = curve.normalized_value(swap_token_a_amount, swap_token_b_amount).unwrap();

//             // Make sure we trade at least one of each token
//             prop_assume!(pool_token_amount * value.to_imprecise().unwrap() >= 2 * token_b_price * pool_token_supply);
//             let deposit_result = curve
//                 .pool_tokens_to_trading_tokens(
//                     pool_token_amount,
//                     pool_token_supply,
//                     swap_token_a_amount,
//                     swap_token_b_amount,
//                     RoundDirection::Ceiling
//                 )
//                 .unwrap();
//             let new_swap_token_a_amount = swap_token_a_amount + deposit_result.token_a_amount;
//             let new_swap_token_b_amount = swap_token_b_amount + deposit_result.token_b_amount;
//             let new_pool_token_supply = pool_token_supply + pool_token_amount;

//             let new_value = curve.normalized_value(new_swap_token_a_amount, new_swap_token_b_amount).unwrap();

//             // the following inequality must hold:
//             // new_value / new_pool_token_supply >= value / pool_token_supply
//             // which reduces to:
//             // new_value * pool_token_supply >= value * new_pool_token_supply

//             let pool_token_supply = PreciseNumber::new(pool_token_supply).unwrap();
//             let new_pool_token_supply = PreciseNumber::new(new_pool_token_supply).unwrap();
//             //let value = U256::from(value);
//             //let new_value = U256::from(new_value);

//             assert!(new_value.checked_mul(&pool_token_supply).unwrap().greater_than_or_equal(&value.checked_mul(&new_pool_token_supply).unwrap()));
//         }
//     }

//     proptest! {
//         #[test]
//         fn curve_value_does_not_decrease_from_withdraw(
//             (pool_token_supply, pool_token_amount) in total_and_intermediate(),
//             swap_token_a_amount in 1..u64::MAX,
//             swap_token_b_amount in 1..u32::MAX, // kept small to avoid proptest rejections
//             token_b_price in 1..u32::MAX, // kept small to avoid proptest rejections
//         ) {
//             let curve = ConstantPriceCurve { token_b_price: token_b_price as u64 };
//             let pool_token_amount = pool_token_amount as u128;
//             let pool_token_supply = pool_token_supply as u128;
//             let swap_token_a_amount = swap_token_a_amount as u128;
//             let swap_token_b_amount = swap_token_b_amount as u128;
//             let token_b_price = token_b_price as u128;

//             let value = curve.normalized_value(swap_token_a_amount, swap_token_b_amount).unwrap();

//             // Make sure we trade at least one of each token
//             prop_assume!(pool_token_amount * value.to_imprecise().unwrap() >= 2 * token_b_price * pool_token_supply);
//             prop_assume!(pool_token_amount <= pool_token_supply);
//             let withdraw_result = curve
//                 .pool_tokens_to_trading_tokens(
//                     pool_token_amount,
//                     pool_token_supply,
//                     swap_token_a_amount,
//                     swap_token_b_amount,
//                     RoundDirection::Floor,
//                 )
//                 .unwrap();
//             prop_assume!(withdraw_result.token_a_amount <= swap_token_a_amount);
//             prop_assume!(withdraw_result.token_b_amount <= swap_token_b_amount);
//             let new_swap_token_a_amount = swap_token_a_amount - withdraw_result.token_a_amount;
//             let new_swap_token_b_amount = swap_token_b_amount - withdraw_result.token_b_amount;
//             let new_pool_token_supply = pool_token_supply - pool_token_amount;

//             let new_value = curve.normalized_value(new_swap_token_a_amount, new_swap_token_b_amount).unwrap();

//             // the following inequality must hold:
//             // new_value / new_pool_token_supply >= value / pool_token_supply
//             // which reduces to:
//             // new_value * pool_token_supply >= value * new_pool_token_supply

//             let pool_token_supply = PreciseNumber::new(pool_token_supply).unwrap();
//             let new_pool_token_supply = PreciseNumber::new(new_pool_token_supply).unwrap();
//             assert!(new_value.checked_mul(&pool_token_supply).unwrap().greater_than_or_equal(&value.checked_mul(&new_pool_token_supply).unwrap()));
//         }
//     }
// }
