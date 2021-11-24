//! Linear price swap curve, slope and initial price point set at init
//! Currently this (especially `swap`) only works under the following assumptions:
//! Deposits (except the initial deposit) are disabled
//! The initial deposit should only have token B (the bonded token) and 0 token A (the collateral token)
//! Withdrawals are disabled (maybe we can add in a check to enable it in emergencies?), TODO: this isn't implemented yet though

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
    spl_math::precise_number::PreciseNumber,
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

/// Babylonian sqrt method
/// this takes ~50K compute vs PreciseNumber::sqrt which takes ~100K
fn sqrt_babylonian(x: u128) -> Option<u128> {
    let mut z = x.checked_add(1)?.checked_div(2)?;
    let mut y = x;
    while z < y {
        y = z;
        z = x.checked_div(z)?.checked_add(z)?.checked_div(2)?;
    }
    Some(y)
}

/// Returns the positive root of x given 0 = a*x^2 + bx + c
/// a is assumed to always be positive
fn solve_quadratic_positive_root(
    a: &PreciseNumber,
    b_abs_value: &PreciseNumber,
    b_is_negative: bool,
    c_abs_value: &PreciseNumber,
    c_is_negative: bool,
) -> Option<PreciseNumber> {
    // TODO: should write some tests for this

    // solve x = (-b + sqrt(b^2 - 4ac)) / 2a

    // a * 4 * c
    let four_a_c_abs_value = a
        .checked_mul(&(PreciseNumber::new(4)?))?
        .checked_mul(c_abs_value)?;

    // b^2 - four_a_c
    let b_squared = b_abs_value.checked_mul(b_abs_value)?;
    let b2_minus_4ac = match c_is_negative {
        // b^2 - (-|4ac|)
        true => b_squared.checked_add(&four_a_c_abs_value)?,
        // b^2 - (+|4ac|)
        false => b_squared.checked_sub(&four_a_c_abs_value)?, // we're going to sqrt this so no need for unsigned_sub
    };

    // note we have to use u128 sqrt since PreciseNumber::sqrt is really expensive (~100K compute vs ~50K compute)
    let b2_minus_4ac_u128 = b2_minus_4ac.to_imprecise()?;
    let sqrt_b2_minus_4ac_u128 = sqrt_babylonian(b2_minus_4ac_u128)?;
    let sqrt_b2_minus_4ac = PreciseNumber::new(sqrt_b2_minus_4ac_u128)?;

    // 2 * a
    let two_a = a.checked_mul(&(PreciseNumber::new(2)?))?;

    // numerator is sqrt(b^2-4ac) - b
    let numerator = match b_is_negative {
        true => {
            // sqrt_b2_minus_4ac - (-|b|)
            sqrt_b2_minus_4ac.checked_add(b_abs_value)?
        }
        false => {
            // sqrt_b2_minus_4ac - |b|
            // this needs to always be positive for our return value to be positive, so use checked_sub
            sqrt_b2_minus_4ac.checked_sub(b_abs_value)?
        }
    };

    // finally we return (sqrt(b^2-4ac) - b)/2a
    numerator.checked_div(&two_a)
}

/// Some helper functions used in `impl CurveCalculator` for readability
impl LinearPriceCurve {
    /// Returns the positive root for token_r_amount = 0.5m*c^2 + (r0 - m*c0)*c + i
    /// token_r_amount is assumed to always be >= 0 (i.e. no negative amounts of collateral token allowed)
    /// i is the integration constant such that 0 collateral token is locked at c0
    fn c_price_with_amt_r_locked(&self, token_r_amount: &PreciseNumber) -> Option<PreciseNumber> {
        // TODO: should write some tests for this

        let slope_numerator = PreciseNumber::new(self.slope_numerator.into())?;
        let slope_denominator = PreciseNumber::new(self.slope_denominator.into())?;
        let m = slope_numerator.checked_div(&slope_denominator)?;
        let r0 = PreciseNumber::new(self.initial_token_r_price.into())?;
        let c0 = PreciseNumber::new(self.initial_token_c_price.into())?;

        // a == 0.5m
        let a = m.checked_div(&(PreciseNumber::new(2)?))?;
        // TODO: rewrite everything using foo_is_positive instead of foo_is_negative, probably way easier to read
        // b == r0 - m*c0 (need to use unsigned_sub here to handle negatives)
        let (b_abs_value, b_is_negative) = r0.unsigned_sub(&(m.checked_mul(&c0)?));

        // calculate integration constant i0 when 0 collateral token is locked at c0,
        // i.e. 0 = a*c0^2 + b*c0 + i
        let (i0_abs_value, i0_is_negative);
        if b_is_negative {
            // since a is always positive, it's a little cleaner to solve for -i = a*c0^2 + b*c0
            // instead of working with all the negatives with PreciseNumber
            let negative_i0_info = a
                .checked_mul(&c0)?
                .checked_mul(&c0)?
                .unsigned_sub(&(b_abs_value.checked_mul(&c0)?));
            i0_abs_value = negative_i0_info.0; // abs value doesn't change from -i to i
            i0_is_negative = !negative_i0_info.1; // i_is_negative is opposite of whether negative_i is negative
        } else {
            // a and b are both positive so i is always negative
            i0_abs_value = a
                .checked_mul(&c0)?
                .checked_mul(&c0)?
                .checked_add(&(b_abs_value.checked_mul(&c0)?))?;
            i0_is_negative = true;
        }

        // finally, solve token_r_amount = a*c^2 + b*c + i0
        // i.e. 0 = a*c^2 + b*c + (i0-token_r_amount)
        let (i_abs_value, i_is_negative);
        if i0_is_negative {
            // both i0 and (-token_r_amount) are negative - can just add the two amounts and keep the sign negative
            i_abs_value = i0_abs_value.checked_add(token_r_amount)?;
            i_is_negative = i0_is_negative;
        } else {
            // otherwise, we have to do signed subtraction to solve (i0 - token_r_amount)
            let i_info = i0_abs_value.unsigned_sub(token_r_amount);
            i_abs_value = i_info.0;
            i_is_negative = i_info.1;
        }

        solve_quadratic_positive_root(&a, &b_abs_value, b_is_negative, &i_abs_value, i_is_negative)
    }

    fn swap_a_to_b(
        &self,
        source_amount: u128,
        swap_source_amount: u128,
        _swap_destination_amount: u128,
    ) -> Option<(u128, u128)> {
        // use swap_source_amount (collateral token) to determine where we are on the integration curve
        // note this only works if non-init deposits are disabled (and maybe if the initial deposit didn't have any token A in it?),
        // otherwise there could be some A token in the pool that isn't part of the bonding curve

        let r_start = PreciseNumber::new(swap_source_amount)?;
        let r_end = r_start.checked_add(&(PreciseNumber::new(source_amount)?))?;

        // TODO: two sqrt calls is pretty expensive (50K each), we could potentially optimize this by storing the initial deposit amount on chain and inferring c_start from that?
        // e.g c_start = initial_deposit_amount - swap_destination_amount (obviously only works if we disallow non-init deposits, and requires a lot of threading)
        let c_start = self.c_price_with_amt_r_locked(&r_start)?;
        let c_end = self.c_price_with_amt_r_locked(&r_end)?;
        let destination_amount = c_end.checked_sub(&c_start)?.to_imprecise()?;

        // TODO: need to handle rounding up/down, especially if not all the source_amount will be used (i.e. there's not enough swap_destination_amount)


        Some((source_amount, destination_amount))
    }

    fn swap_b_to_a(
        &self,
        _source_amount: u128,
        _swap_source_amount: u128,
        _swap_destination_amount: u128,
    ) -> Option<(u128, u128)> {
        Some((0, 0))
    }
}

impl CurveCalculator for LinearPriceCurve {
    /// Calculate how much destination token will be provided given an amount
    /// of source token.
    fn swap_without_fees(
        &self,
        source_amount: u128,
        swap_source_amount: u128,
        swap_destination_amount: u128,
        trade_direction: TradeDirection,
    ) -> Option<SwapWithoutFeesResult> {
        let (source_amount_swapped, destination_amount_swapped) = match trade_direction {
            TradeDirection::AtoB => {
                self.swap_a_to_b(source_amount, swap_source_amount, swap_destination_amount)?
            }
            TradeDirection::BtoA => {
                self.swap_b_to_a(source_amount, swap_source_amount, swap_destination_amount)?
            }
        };
        let source_amount_swapped = map_zero_to_none(source_amount_swapped)?;
        let destination_amount_swapped = map_zero_to_none(destination_amount_swapped)?;
        Some(SwapWithoutFeesResult {
            source_amount_swapped,
            destination_amount_swapped,
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_basic() {
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 2,
            initial_token_r_price: 50,
            initial_token_c_price: 300,
        };

        // put in 101 RLY, should get 2 CC out
        let (source_amount, destination_amount) = curve.swap_a_to_b(101, 0, 5000).unwrap();
        assert_eq!(source_amount, 101);
        assert_eq!(destination_amount, 2);

        // put in 103 RLY, should get 2 more CC out
        let (source_amount, destination_amount) = curve.swap_a_to_b(103, 101, 4998).unwrap();
        assert_eq!(source_amount, 103);
        assert_eq!(destination_amount, 2);

        // same as above but assuming they both have 8 decimals
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 2_0000_0000, // slope needs to be scaled down to take into account C having 8 decimals
            initial_token_r_price: 50, // since they both have 8 decimals, no need to scale this (it's still 50 base RLY for 1 base CC)
            initial_token_c_price: 300_0000_0000,
        };

        let (source_amount, destination_amount) =
            curve.swap_a_to_b(101_0000_0000, 0, 5000).unwrap();
        assert_eq!(source_amount, 101_0000_0000);
        assert_eq!(destination_amount, 2_0000_0000);

        // similar to 145K segment of forte curve, but assume r has 18 decimals (this just lets us cram more precision into
        // the calculation, as long as we interpret it correctly back out at the end)
        // since r has 12 more decimals of precision than c, scale both slope and initial_token_r_price by 1e12
        let curve = LinearPriceCurve {
            slope_numerator: 5689_549_999_968_874, // 5.689549999968874e-9 in forte, so should be 5.689549999968874e3 now
            slope_denominator: 1_000_000_000_000,
            initial_token_r_price: 35_915742_315103, // 35.9157423151027 in forte, so should be 3.59...e13 now
            initial_token_c_price: 145000_000000,
        };

        // putting in 7296.9394630144 RLY in, should get 200 CC out (i.e. 200_000000)
        let (source_amount, destination_amount) = curve
            .swap_a_to_b(7296_939463_014400_000000, 0, 5000_000000)
            .unwrap();
        assert_eq!(source_amount, 7296_939463_014400_000000);
        assert_eq!(destination_amount, 200_000000);

        // put in 7524.5214630093 more RLY, should get another 200 CC out
        let (source_amount, destination_amount) = curve
            .swap_a_to_b(
                7524_521463_009300_000000,
                7296_939463_014400_000000,
                4800_000000,
            )
            .unwrap();
        assert_eq!(source_amount, 7524_521463_009300_000000);
        assert_eq!(destination_amount, 200_000000);
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
