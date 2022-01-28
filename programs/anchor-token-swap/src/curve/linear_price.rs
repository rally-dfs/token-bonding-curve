//! Linear price swap curve, slope and initial price point set at init
//! Currently this (especially `swap`) only works under the following assumptions:
//! Deposits (except the initial deposit) are disabled
//! The initial deposit should only have token B (the bonded token) and 0 token A (the collateral token)
//! This curve only works with fees set to 0 (process_swap will panic otherwise)
//! Withdrawals are disabled (maybe we can add in a check to enable it in emergencies?), will panic if those
//! instructions are called

use {
    crate::{
        curve::calculator::{
            map_zero_to_none, CurveCalculator, DynPack, RoundDirection, SwapWithoutFeesResult,
            TradeDirection, TradingTokenResult,
        },
        dfs_precise_number::PreciseNumber,
        error::SwapError,
    },
    arrayref::{array_mut_ref, array_ref},
    solana_program::{
        program_error::ProgramError,
        program_pack::{IsInitialized, Pack, Sealed},
    },
};

/// LinearPriceCurve struct implementing CurveCalculator
/// A is the "collateral" token (e.g. RLY), B is the "bonded" token (e.g. TAKI).
/// The price of a single B token (a, denominated in amount of token A) is defined by
/// `a = slope*b + initial_token_a_price`
/// where b is the amount of token B that's been swapped out of this curve
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinearPriceCurve {
    /// Slope of price increase (how much price of token B increases for every token A that's bonded to it) numerator
    pub slope_numerator: u64,
    /// Slope of price increase (how much price of token B increases for every token A that's bonded to it) denominator
    pub slope_denominator: u64,
    /// When there's 0 liquidity in the pool, what should the initial price point a0 defining the curve be?
    /// i.e. what is the cost of 1 b token (denominated in A) when there's 0 liquidity
    pub initial_token_a_price_numerator: u64,
    /// When there's 0 liquidity in the pool, what should the initial price point a0 defining the curve be?
    /// i.e. what is the cost of 1 b token (denominated in A) when there's 0 liquidity
    pub initial_token_a_price_denominator: u64,
}

/// Babylonian sqrt method
/// this takes ~50K compute vs PreciseNumber::sqrt which takes ~100K
/// Note this will underestimate if not exact - that's taken into account in
/// solve_quadratic_positive_root
fn sqrt_babylonian(x: u128) -> Option<u128> {
    let mut z = x.checked_add(1)?.checked_div(2)?;
    let mut y = x;
    while z < y {
        y = z;
        z = x.checked_div(z)?.checked_add(z)?.checked_div(2)?;
    }
    Some(y)
}

/// Returns the positive root of x given lhs = k*x^2 + e*x, i.e.
/// 0 = k*x^2 + e*x - lhs
/// k, e, and lhs are assumed to always be non negative (so -lhs is always negative)
/// (We're using k/e/c instead of a/b/c to not clash with token a/b names)
/// Since e is always positive and c is always negative, the quadratic will always have one positive
/// and one negative root, just return the positive one
fn solve_quadratic_positive_root(
    k_numerator: &PreciseNumber,
    k_denominator: &PreciseNumber,
    e_value_numerator: &PreciseNumber,
    e_value_denominator: &PreciseNumber,
    lhs_value: &PreciseNumber,
    should_round_sqrt_up: bool,
) -> Option<PreciseNumber> {
    // solve positive root of 0 = k*x^2 + e*x + c, where c == -lhs_value
    // => x = (-e + sqrt(e^2 - 4kc)) / 2k
    // => x = (sqrt(e^2 + 4*k*lhs) - e) / 2k

    // k * 4 * lhs
    let four_k_lhs = k_numerator
        .checked_mul(&(PreciseNumber::new(4)?))?
        .checked_mul(lhs_value)?
        .checked_div(k_denominator)?;

    // e^2 + k * 4 * lhs
    let e2_plus_4_k_lhs = e_value_numerator
        .checked_mul(e_value_numerator)?
        .checked_div(e_value_denominator)?
        .checked_div(&e_value_denominator)?
        .checked_add(&four_k_lhs)?;

    // note we have to use u64 sqrt below (~10K compute) since PreciseNumber::sqrt (~100K compute)
    // and u128 sqrt (~50K compute) are both too expensive
    // TODO: need to move the rounding up/down stuff into sqrt_u128 too
    let sqrt_e2_plus_4_k_lhs = e2_plus_4_k_lhs.sqrt_u64(should_round_sqrt_up)?;

    // numerator is sqrt(e^2 + 4*k*lhs) - e
    let e_value = e_value_numerator.checked_div(e_value_denominator)?;
    // due to sqrt rounding, sometimes this None's if we rounded down the sqrt, so treat that as 0
    let numerator = match sqrt_e2_plus_4_k_lhs.checked_sub(&e_value) {
        Some(val) => val,
        None => PreciseNumber::new(0)?,
    };

    // finally we return (sqrt(e^2-4kc) - e)/2k,
    // AKA numerator * k_denominator / k_numerator / 2 (do all the division last)
    numerator
        .checked_mul(k_denominator)?
        .checked_div(&k_numerator)?
        .checked_div(&(PreciseNumber::new(2)?))
}

/// These functions use the integral of the linear price curve to determine liquidity of A at a
/// given B value (amt_a_locked_at_b_value_quadratic)
/// It also uses the quadratic formula to solve the same integral to determine the B value for a given
/// liquidity (b_value_with_amt_a_locked_quadratic)
///
/// swap_a_to_b and swap_b_to_a are the key functions at the bottom
/// The sqrt function drops down to u128 so we don't use all our compute but everything else uses PreciseNumber
impl LinearPriceCurve {
    /// Returns the amount of A token locked at a given b_value (by plugging b_value into the integral function)
    fn amt_a_locked_at_b_value_quadratic(&self, b_value: &PreciseNumber) -> Option<PreciseNumber> {
        // The liquidity integral is `token_a_bonded = 0.5m*b^2 + a0*b + 0` (integration constant is 0 since we know
        // there's 0 token A bonded at b = 0)

        // 0.5 * m * b^2
        let half_m_b_squared = PreciseNumber::new(self.slope_numerator.into())?
            .checked_mul(b_value)?
            .checked_mul(b_value)?
            .checked_div(&(PreciseNumber::new(self.slope_denominator.into())?))?
            .checked_div(&(PreciseNumber::new(2)?))?;

        // a0 * b (note a0 and b are always positive) - make sure to do division last
        let a0_times_b = PreciseNumber::new(self.initial_token_a_price_numerator.into())?
            .checked_mul(b_value)?
            .checked_div(&(PreciseNumber::new(self.initial_token_a_price_denominator.into())?))?;

        half_m_b_squared.checked_add(&a0_times_b)
    }

    /// Returns the positive root for token_a_amount = 0.5m*b^2 + a0*b + 0
    /// (integration constant is always 0 since we know there's 0 token A bonded at b = 0)
    fn b_value_with_amt_a_locked_quadratic(
        &self,
        token_a_amount: &PreciseNumber,
        should_round_sqrt_up: bool,
    ) -> Option<PreciseNumber> {
        // (We're using k/e for quadratic coefficients instead of a/b to not clash with token a/b names)

        // k = 0.5 * m
        // Note k is kept as a fraction since pre-dividing PreciseNumber loses a lot of
        // precision (only 12 decimal digits max) - we're going to be multiplying it against prices (k*b^2) so
        // no need to lose that precision (and as long as slope_numerator/price are all u64 there's plenty of
        // room in PreciseNumber to avoid overflow)
        let slope_numerator = PreciseNumber::new(self.slope_numerator.into())?;
        let slope_denominator = PreciseNumber::new(self.slope_denominator.into())?;
        let k_numerator = slope_numerator.checked_mul(&(PreciseNumber::new(1)?))?;
        let k_denominator = slope_denominator.checked_mul(&(PreciseNumber::new(2)?))?;

        // e = a0
        let e_value_numerator = PreciseNumber::new(self.initial_token_a_price_numerator.into())?;
        let e_value_denominator =
            PreciseNumber::new(self.initial_token_a_price_denominator.into())?;

        // solve 0 = k*x^2 + e*x - token_a_amount
        solve_quadratic_positive_root(
            &k_numerator,
            &k_denominator,
            &e_value_numerator,
            &e_value_denominator,
            &token_a_amount,
            should_round_sqrt_up,
        )
    }

    /// If `source_amount` will cause the swap to return all of its remaining `swap_destination_amount`,
    /// this returns the (maximum_token_a_amount, swap_destination_amount) that the swap can take
    /// Otherwise (if there's enough `swap_destination_amount` to handle all the `source_amount`), returns None
    fn maximum_a_remaining_for_swap_a_to_b(
        &self,
        a_start: &PreciseNumber,
        b_start: &PreciseNumber,
        source_amount: u128,
        swap_destination_amount: u128,
    ) -> Option<(u128, u128)> {
        // if at b_start + swap_destination_amount (the maximum B that be given out by the swap),
        // then the A value is <= source_amount, so only take that amount of A instead and give them all the
        // Bs remaining
        let maximum_b_value =
            b_start.checked_add(&(PreciseNumber::new(swap_destination_amount)?))?;
        let maximum_a_locked = self.amt_a_locked_at_b_value_quadratic(&maximum_b_value)?;
        let maximum_a_remaining = maximum_a_locked.checked_sub(&a_start)?.to_imprecise()?;

        if maximum_a_remaining <= source_amount {
            return Some((maximum_a_remaining, swap_destination_amount));
        } else {
            return None;
        }
    }

    /// Swap's in user's collateral token and returns out the bonded token,
    /// moving right on the price curve and increasing the price of the bonded token
    fn swap_a_to_b(
        &self,
        source_amount: u128,      // amount of user's token a (collateral token)
        swap_source_amount: u128, // swap's token a (collateral token)
        swap_destination_amount: u128, // swap's remaining token b (bonded token)
    ) -> Option<(u128, u128)> {
        // use swap_source_amount (collateral token) to determine where we are on the integration curve
        // note this only works if non-init deposits are disabled (and maybe if the initial deposit didn't have any token A in it?),
        // otherwise there could be some A token in the pool that isn't part of the bonding curve

        // quadratic formula version:
        let a_start = PreciseNumber::new(swap_source_amount)?;

        let b_start = self.b_value_with_amt_a_locked_quadratic(&a_start, true)?;

        match self.maximum_a_remaining_for_swap_a_to_b(
            &a_start,
            &b_start,
            source_amount,
            swap_destination_amount,
        ) {
            Some(val) => return Some(val),
            // no need to return None here if checked_add fails, can just skip this check and do real calculation below
            None => (),
        }

        // otherwise, there's enough B tokens for all the A they put in, find the b_end value for the amount of A
        // they're putting in and give them `b_end - b_start` tokens out
        let a_end = a_start.checked_add(&(PreciseNumber::new(source_amount)?))?;

        let b_end = self.b_value_with_amt_a_locked_quadratic(&a_end, false)?;

        let difference = b_end.checked_sub(&b_start)?;
        // PreciseNumber rounds .5+ up by default, make sure to floor instead so we don't allow
        // dust to round up for free
        let destination_amount = difference.floor()?.to_imprecise()?;

        Some((source_amount, destination_amount))
    }

    fn swap_b_to_a(
        &self,
        source_amount: u128,
        _swap_source_amount: u128,
        swap_destination_amount: u128,
    ) -> Option<(u128, u128)> {
        // use swap_destination_amount (collateral token) to determine where we are on the integration curve
        // note this only works if non-init deposits are disabled (and maybe if the initial deposit didn't have any token A in it?),
        // otherwise there could be some A token in the pool that isn't part of the bonding curve

        // make sure we round up here so that b_end and a_end are also over-estimated, which rounds down the final
        // token a output
        let b_start = self.b_value_with_amt_a_locked_quadratic(
            &(PreciseNumber::new(swap_destination_amount)?),
            true,
        )?;

        // b_end can be negative if the user put in too many B tokens (handled below)
        let (b_end, b_end_is_negative) =
            b_start.unsigned_sub(&(PreciseNumber::new(source_amount)?));

        // make sure to use b_end.ceiling() when doing below calculations a_end so we don't round in favor of the user
        // if we use b_end directly, it's possible to gain tokens for free by swapping back and forth due to
        // rounding (see swap_large_price_a_u32 test)
        // (especially since sqrt_babylonian under estimates, we often will end up with a b_end/a_end that's too low
        // due to rounding)
        let b_end = b_end.ceiling()?;

        // if b_end < 0 (i.e. there aren't enough A tokens in the swap for all the B tokens they put in),
        // then just give them all of the a tokens (swap_destination_amount) and only take the B tokens required to
        // get down from b_start to 0. this only works if we assume 0 A locked at b = 0
        if b_end_is_negative {
            return Some((b_start.to_imprecise()?, swap_destination_amount));
        }

        // otherwise if there's enough A tokens locked in swap_destination_amount, figure out the A value at
        // b_end and give them the difference (swap_destination_amount - a_end) tokens
        let a_end = self.amt_a_locked_at_b_value_quadratic(&b_end)?;

        // PreciseNumber rounds .5+ up by default, make sure to floor instead so we don't allow
        // dust to round up for free
        let destination_amount = PreciseNumber::new(swap_destination_amount)?
            .checked_sub(&a_end)?
            .floor()?
            .to_imprecise()?;



        Some((source_amount, destination_amount))
    }
}

/// Returns None iff slope is 0 or close enough to 0 with PreciseNumber
fn is_curve_param_valid(curve: &LinearPriceCurve) -> Option<()> {
    if curve.slope_numerator == 0
        || curve.slope_denominator == 0
        || curve.initial_token_a_price_denominator == 0
    {
        return None;
    };

    // since PreciseNumber only has 18 decimals, any slope < 1e-18 will be treated as 0
    let numerator = PreciseNumber::new(curve.slope_numerator.into())?;
    let denominator = PreciseNumber::new(curve.slope_denominator.into())?;
    let minimum =
        PreciseNumber::new(1)?.checked_div(&(PreciseNumber::new(1_000_000_000_000_000_000)?))?;

    match numerator
        .checked_div(&denominator)?
        .greater_than_or_equal(&minimum)
    {
        true => Some(()),
        false => None,
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
        _pool_tokens: u128,
        _pool_token_supply: u128,
        _swap_token_a_amount: u128,
        _swap_token_b_amount: u128,
        _round_direction: RoundDirection,
    ) -> Option<TradingTokenResult> {
        // this causes a panic if withdraw_all_token_types is called but that's ok for now, cheap way of
        // disabling withdrawals without having to change how SwapCurve works
        None

        // could we do something like this if we just want pool tokens to be 1-1 with B tokens and not
        // withdrawable/depositable for A tokens?
        // Some(TradingTokenResult {
        //     token_a_amount: 0,
        //     token_b_amount: pool_tokens,
        // })
    }

    /// Get the amount of pool tokens for the given amount of token A and B
    /// TODO: this isn't needed if we disable deposits, otherwise
    /// it's used in deposit_single_token_type_exact_amount_in to determine
    /// how much pool token to mint (given a trading token amount and a minimum_pool_token_rmount)
    fn deposit_single_token_type(
        &self,
        _source_amount: u128,
        _swap_token_a_amount: u128,
        _swap_token_b_amount: u128,
        _pool_supply: u128,
        _trade_direction: TradeDirection,
    ) -> Option<u128> {
        // this never gets called since allows_withdrawals is false (would panic otherwise so still safe)
        None
    }

    /// Get the amount of pool tokens for the withdrawn amount of token A or B.
    /// TODO: this mostly isn't needed if we disable withdrawals, UNLESS we have
    /// non-zero host fees/trade fees, in which case it's used in `swap` to determine
    /// how much pool token to mint (to account for fees) into the various fee accounts
    fn withdraw_single_token_type_exact_out(
        &self,
        _source_amount: u128,
        _swap_token_a_amount: u128,
        _swap_token_b_amount: u128,
        _pool_supply: u128,
        _trade_direction: TradeDirection,
    ) -> Option<u128> {
        // this causes a panic if SwapCurve.withdraw_single_token_type_exact_out instruction is called
        // but that's ok for now, cheap way of disabling withdrawals without having to change how SwapCurve works
        // (also if a non-zero fee curve is created this would also cause a panic, though that's disabled at the
        // lib.rs level)
        None
    }

    /// Validate that the given curve has no invalid parameters
    /// Called on `initialize` - slope must be positive but initial point can be (0,0)
    fn validate(&self) -> Result<(), SwapError> {
        match is_curve_param_valid(&self) {
            Some(_val) => Ok(()),
            None => Err(SwapError::InvalidCurve),
        }
    }

    /// Validate the given supply on initialization.
    /// We require at least some bonded token B for the curve to be useful (collateral token can be 0)
    /// TODO: if we enable deposits, then this check isn't needed, the pool can start with 0 of both
    fn validate_supply(&self, token_a_amount: u64, token_b_amount: u64) -> Result<(), SwapError> {
        if token_b_amount == 0 {
            return Err(SwapError::EmptySupply);
        }

        // i think there's probably a way to allow initial collateral token if we adjust the
        // initial token values to take that into account, but seems easier to disallow it. it's the same
        // as starting with 0 collateral token and then doing a swap anyway
        if token_a_amount != 0 {
            return Err(SwapError::InvalidSupply);
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

    /// The total normalized value of the linear price curve adds the total
    /// value of the token A side (as denominated in token B) to the token B side.
    fn normalized_value(
        &self,
        swap_token_a_amount: u128,
        swap_token_b_amount: u128,
    ) -> Option<spl_math::precise_number::PreciseNumber> {
        let b_value_of_a = self.b_value_with_amt_a_locked_quadratic(
            &(PreciseNumber::new(swap_token_a_amount)?),
            false,
        )?;
        let total_value = b_value_of_a.checked_add(&(PreciseNumber::new(swap_token_b_amount)?))?;

        // we only have a precision of 32 bits (9 digits) for sqrt so just truncate to that
        // (it's okay if the curve's value increases as long as the increase is under that precision)
        let value_bits = total_value.value.bits();
        let truncated_value = match value_bits > 32 {
            true => total_value.value >> (value_bits - 32),
            false => total_value.value,
        };

        Some(spl_math::precise_number::PreciseNumber {
            value: truncated_value,
        })
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
        let initial_token_a_price_numerator = array_ref![input, 16, 8];
        let initial_token_a_price_denominator = array_ref![input, 24, 8];
        Ok(Self {
            slope_numerator: u64::from_le_bytes(*slope_numerator),
            slope_denominator: u64::from_le_bytes(*slope_denominator),
            initial_token_a_price_numerator: u64::from_le_bytes(*initial_token_a_price_numerator),
            initial_token_a_price_denominator: u64::from_le_bytes(
                *initial_token_a_price_denominator,
            ),
        })
    }
}

impl DynPack for LinearPriceCurve {
    fn pack_into_slice(&self, output: &mut [u8]) {
        let slope_numerator = array_mut_ref![output, 0, 8];
        *slope_numerator = self.slope_numerator.to_le_bytes();
        let slope_denominator = array_mut_ref![output, 8, 8];
        *slope_denominator = self.slope_denominator.to_le_bytes();
        let initial_token_a_price = array_mut_ref![output, 16, 8];
        *initial_token_a_price = self.initial_token_a_price_numerator.to_le_bytes();
        let initial_token_a_price = array_mut_ref![output, 24, 8];
        *initial_token_a_price = self.initial_token_a_price_denominator.to_le_bytes();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::calculator::test::check_curve_value_from_swap;
    use proptest::prelude::*;

    #[test]
    fn swap_a_to_b_basic() {
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 2,
            initial_token_a_price_numerator: 150,
            initial_token_a_price_denominator: 3, // using non-1 just to test out
        };

        // put in 101 A, should get 2 B out
        let (source_amount, destination_amount) = curve.swap_a_to_b(101, 0, 5000).unwrap();
        assert_eq!(source_amount, 101);
        assert_eq!(destination_amount, 2);

        // put in 103 A, should get 2 more B out
        let (source_amount, destination_amount) = curve.swap_a_to_b(103, 101, 4998).unwrap();
        assert_eq!(source_amount, 103);
        assert_eq!(destination_amount, 2);

        // same as above but assuming they both have 8 decimals
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 2_0000_0000, // slope needs to be scaled down to take into account B having 8 decimals
            initial_token_a_price_numerator: 150, // since they both have 8 decimals, no need to scale this (it's still 50 base A for 1 base B)
            initial_token_a_price_denominator: 3, // using non-1 just to test out
        };

        let (source_amount, destination_amount) =
            curve.swap_a_to_b(101_0000_0000, 0, 5000_0000_0000).unwrap();
        assert_eq!(source_amount, 101_0000_0000);
        assert_eq!(destination_amount, 2_0000_0000);

        // putting in 5900K A @ 81600 A locked/20B remaining should give out the last 20 B
        let (source_amount, destination_amount) = curve
            .swap_a_to_b(5900_0000_0000, 81600_0000_0000, 20_0000_0000)
            .unwrap();
        assert_eq!(source_amount, 5900_0000_0000);
        assert_eq!(destination_amount, 20_0000_0000);

        // putting in 10K A @ 81600 A locked/20B remaining should give out the last 20 B and only take 5.9K A
        let (source_amount, destination_amount) = curve
            .swap_a_to_b(10000_0000_0000, 81600_0000_0000, 20_0000_0000)
            .unwrap();
        assert_eq!(source_amount, 5900_0000_0000);
        assert_eq!(destination_amount, 20_0000_0000);

        // similar to 145K segment of forte curve, but assume a has 18 decimals (this just lets us cram more precision into
        // the calculation, as long as we interpret it correctly back out at the end)
        // since a has 12 more decimals of precision than b, scale both slope and initial_token_a_price by 1e12
        let curve = LinearPriceCurve {
            slope_numerator: 5689_549_999_968_874, // 5.689549999968874e-9 in forte, so should be 5.689549999968874e3 now
            slope_denominator: 1_000_000_000_000,
            initial_token_a_price_numerator: 35_915742_315103, // 35.9157423151027 in forte, so should be 3.59...e13 now
            initial_token_a_price_denominator: 1,
        };

        // putting in 7296... A in, should move price to 145_199_999999.99
        // (i.e. get 199_999999 B out)
        let (source_amount, destination_amount) = curve
            .swap_a_to_b(7296_939463_019977_479999, 0, 5000_000000)
            .unwrap();
        assert_eq!(source_amount, 7296_939463_019977_479999);
        assert_eq!(destination_amount, 199_999997); // rounds down a bit due to sqrt precision

        // put in 7524... more A, should get another 199_999999 B out
        let (source_amount, destination_amount) = curve
            .swap_a_to_b(
                7524_521463_008709_920000,
                7296_939463_030000_000000,
                4800_000000,
            )
            .unwrap();
        assert_eq!(source_amount, 7524_521463_008709_920000);
        assert_eq!(destination_amount, 199_999997); // rounds down a bit due to sqrt precision
    }

    #[test]
    fn swap_b_to_a_basic() {
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 2,
            initial_token_a_price_numerator: 150,
            initial_token_a_price_denominator: 3, // using non-1 just to test out
        };

        // pretty much the opposite cases as above

        // put in 2 B at 101 A, should get 101 A out
        let (source_amount, destination_amount) = curve.swap_b_to_a(2, 4998, 101).unwrap();
        assert_eq!(source_amount, 2);
        assert_eq!(destination_amount, 101);

        // put in 2 B at 204 A, should get 103 A out
        let (source_amount, destination_amount) = curve.swap_b_to_a(2, 4996, 204).unwrap();
        assert_eq!(source_amount, 2);
        assert_eq!(destination_amount, 103);

        // same as above but assuming they both have 8 decimals
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 2_0000_0000, // slope needs to be scaled down to take into account B having 8 decimals
            initial_token_a_price_numerator: 150, // since they both have 8 decimals, no need to scale this (it's still 50 base A for 1 base B)
            initial_token_a_price_denominator: 3, // using non-1 just to test out
        };

        let (source_amount, destination_amount) = curve
            .swap_b_to_a(2_0000_0000, 4998_0000_0000, 101_0000_0000)
            .unwrap();
        assert_eq!(source_amount, 2_0000_0000);
        assert_eq!(destination_amount, 101_0000_0000);

        // similar to 145K segment of forte curve, but assume a has 18 decimals (this just lets us cram more precision into
        // the calculation, as long as we interpret it correctly back out at the end)
        // since a has 12 more decimals of precision than b, scale both slope and initial_token_a_price by 1e12
        let curve = LinearPriceCurve {
            slope_numerator: 5689_549_999_968_874, // 5.689549999968874e-9 in forte, so should be 5.689549999968874e3 now
            slope_denominator: 1_000_000_000_000,
            initial_token_a_price_numerator: 35_915742_315103, // 35.9157423151027 in forte, so should be 3.59...e13 now
            initial_token_a_price_denominator: 1,
        };

        // putting in 200 B at 7296.9394630144 A, should get it all out
        let (source_amount, destination_amount) = curve
            .swap_b_to_a(200_000000, 4800_000000, 7296_939463_019977_480000)
            .unwrap();
        assert_eq!(source_amount, 200_000000);
        // note this rounds down from  7296_939463019977480000 due to sqrt rounding
        assert_eq!(destination_amount, 7296_939427104235162052);

        // put in 200 B at 14821.4609260237 A, should get 7524.5214630093 A out
        let (source_amount, destination_amount) = curve
            .swap_b_to_a(200_000000, 4600_000000, 14821_460926_038709_920000)
            .unwrap();
        assert_eq!(source_amount, 200_000000);
        // note this rounds down from  7524_521463018732440000 due to sqrt rounding
        assert_eq!(destination_amount, 7524_521388911427798427);

        // put in 300 B at 7296.9394630144 A, should get it all out (and only take 200 B)
        let (source_amount, destination_amount) = curve
            .swap_b_to_a(300_000000, 4800_000000, 7296_939463_019977_480000)
            .unwrap();
        assert_eq!(source_amount, 200_000000);
        assert_eq!(destination_amount, 7296_939463_019977_480000);
    }

    #[test]
    fn swap_0_0_curve() {
        // a curve that starts at 0/0
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 2,
            initial_token_a_price_numerator: 0,
            initial_token_a_price_denominator: 1,
        };

        // put in 9 A, should get 6 B out
        let (source_amount, destination_amount) = curve.swap_a_to_b(9, 0, 5000).unwrap();
        assert_eq!(source_amount, 9);
        assert_eq!(destination_amount, 6);

        // put in 6 B at 9 A, should get all 9 A out
        let (source_amount, destination_amount) = curve.swap_b_to_a(6, 494, 9).unwrap();
        assert_eq!(source_amount, 6);
        assert_eq!(destination_amount, 9);

        // put in 11 B at 9 A, should get all 9 A out and only take 6 B
        let (source_amount, destination_amount) = curve.swap_b_to_a(11, 494, 9).unwrap();
        assert_eq!(source_amount, 6);
        assert_eq!(destination_amount, 9);
    }

    #[test]
    fn swap_without_fees() {
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 2,
            initial_token_a_price_numerator: 350,
            initial_token_a_price_denominator: 7, // using non-1 just to test out
        };

        let result = curve
            .swap_without_fees(101, 0, 5000, TradeDirection::AtoB)
            .unwrap();
        assert_eq!(
            result,
            SwapWithoutFeesResult {
                source_amount_swapped: 101,
                destination_amount_swapped: 2
            }
        );

        let result = curve
            .swap_without_fees(2, 4998, 101, TradeDirection::BtoA)
            .unwrap();
        assert_eq!(
            result,
            SwapWithoutFeesResult {
                source_amount_swapped: 2,
                destination_amount_swapped: 101
            }
        );
    }

    #[test]
    fn pack_flat_curve() {
        let curve = LinearPriceCurve {
            slope_numerator: u64::MAX,
            slope_denominator: u64::MAX - 1,
            initial_token_a_price_numerator: 0,
            initial_token_a_price_denominator: u32::MAX.into(),
        };

        let mut packed = [0u8; LinearPriceCurve::LEN];
        Pack::pack_into_slice(&curve, &mut packed[..]);
        let unpacked = LinearPriceCurve::unpack(&packed).unwrap();
        assert_eq!(curve, unpacked);

        let mut packed = vec![];
        packed.extend_from_slice(&curve.slope_numerator.to_le_bytes());
        packed.extend_from_slice(&curve.slope_denominator.to_le_bytes());
        packed.extend_from_slice(&curve.initial_token_a_price_numerator.to_le_bytes());
        packed.extend_from_slice(&curve.initial_token_a_price_denominator.to_le_bytes());
        let unpacked = LinearPriceCurve::unpack(&packed).unwrap();
        assert_eq!(curve, unpacked);
    }

    /// These swap_large_price_foo tests all test the overflow boundaries of u64/u128 test - mostly just to give
    /// some example curves with large numbers (and make sure they return None instead of panicking etc)
    /// They also test that rounding is always not in the user's favor to prevent arbitrage
    /// Summary: when initial_token_a_price == u64::MAX, these curves are all useless (it costs more than the entire
    /// supply of token B to get 1 token A out)
    #[test]
    fn swap_large_price_max_a() {
        // curve with everything near u64::MAX (though slope is actually ~1)
        let curve = LinearPriceCurve {
            slope_numerator: u64::MAX,
            slope_denominator: u64::MAX - 1,
            initial_token_a_price_numerator: u64::MAX,
            initial_token_a_price_denominator: 1,
        };

        // with initial_token_a_price == u64::MAX, there aren't enough ever enough A tokens to get any
        // B tokens out
        // 0 <- B value at A = 0
        // 1 <- B value at A = 2^64 (already spl token max)
        for exp in 0..96 {
            let result = curve.swap_without_fees(
                2_u128.pow(exp),
                0,
                1_00000_00000_00000_00000,
                TradeDirection::AtoB,
            );
            assert!(result.is_none());
        }

        // putting in 2^97 tokens works, though not much point since it's past spl token max
        let result = curve.swap_without_fees(
            2u128.pow(97),
            0,
            1_00000_00000_00000_00000,
            TradeDirection::AtoB,
        );
        assert_eq!(
            result.unwrap(),
            SwapWithoutFeesResult {
                source_amount_swapped: 2u128.pow(97),
                destination_amount_swapped: 4611686018
            }
        );

        // putting in u128 max tokens works too, so this won't ever overflow
        let result = curve.swap_without_fees(
            u128::MAX,
            0,
            1_00000_00000_00000_00000,
            TradeDirection::AtoB,
        );
        assert_eq!(
            result.unwrap(),
            SwapWithoutFeesResult {
                source_amount_swapped: u128::MAX,
                destination_amount_swapped: 13503953894904916780
            }
        );

        // b -> a (kind of pointless since we can't get here from a -> b but just checking for completeness)
        // 1 <- B value at A = 2^64 <- minimum amount of A to get any B tokens out, but already overflows
        // 0 <- B value at A = 0
        // (diff is 1)
        // put in 1 B tokens at A = 2^64, should get 2^64 A out
        // note just like the above, the sqrt calculation overflows even with just 1 B
        let result = curve.swap_without_fees(
            1,
            0, // this doesn't matter (it's the amount of token b left but we're going the other direction)
            2u128.pow(64),
            TradeDirection::BtoA,
        );
        assert!(result.is_none());
    }

    /// These swap_large_price_foo tests all test the overflow boundaries of u64/u128 test - mostly just to give
    /// some example curves with large numbers (and make sure they return None instead of panicking etc)
    /// They also test that rounding is always not in the user's favor to prevent arbitrage
    /// Summary: with a pretty high initial_token_a_price at u32::MAX, it takes almost all of the token A supply
    /// to get out very little token B. No significant rounding issues or overflow evne up to u128 max token a swapped
    #[test]
    fn swap_large_price_a_u32() {
        // example curve with A price relatively low and everything else high
        let curve = LinearPriceCurve {
            slope_numerator: u64::MAX,
            slope_denominator: u64::MAX - 1,
            initial_token_a_price_numerator: u32::MAX.into(),
            initial_token_a_price_denominator: 1,
        };

        // testing a -> b

        // put in 2^64 - 1 A tokens, should move B value from
        // 0 <- B value at A = 0
        // 3144134277.94 <- B value at A = 2^64 - 1 (should floor down)
        let result = curve.swap_without_fees(
            (u64::MAX).into(),
            0,
            1_00000_00000_00000_00000,
            TradeDirection::AtoB,
        );
        assert_eq!(
            result.unwrap(),
            SwapWithoutFeesResult {
                source_amount_swapped: u64::MAX.into(),
                // a little less than real value of 31441_34276 due to sqrt rounding
                destination_amount_swapped: 31441_34275
            }
        );

        // note calculator handles values even up to 128 max without overflowing
        // 0 <- B value at A = 0
        // 49753921409676.39 <- B value at A = 2^90
        let result = curve.swap_without_fees(
            u128::MAX,
            0,
            1_00000_00000_00000_00000,
            TradeDirection::AtoB,
        );
        assert_eq!(
            result.unwrap(),
            SwapWithoutFeesResult {
                source_amount_swapped: u128::MAX,
                // note because of sqrt precision, this is slightly rounded down from exact value of
                // 26087635646370597129
                destination_amount_swapped: 26087635639488208246
            }
        );

        // TODO: need to fix the below test values once DFSPN is finalized

        // testing b -> a on the same curve
        // 340282366920938463463374607431768211455 (u128 max) <- A value at B = 26087635639488208246
        // 85070591713359687941906431701768052580 <- A value at B = 13043817819744104123 (halfway to initial B)
        let result = curve
            .swap_without_fees(
                13043817819744104123, // amount B in = diff between B values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                340282366920938463463374607431768211455,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 13043817819744104123);
        assert_eq!(
            result.destination_amount_swapped,
            // note due to sqrt precision this is slightly less than the exact amount of
            // 255211775207578775521468175730000158875
            255211775087270790878881086478269459899 // amount A out = diff between A values
        );

        // now (with actual A numbers above), swap balance is 85070591833667672584493520953498751556
        // 85070591833667672584493520953498751556 <- A value at B = 13043817828967476162
        //  (using the rounded A value from above to make sure the rounding doesn't cause any compounding issues)
        // 21267647972422610890461683671995218336 <- A value at B = 6521908914483738081
        //  (another halfway down to initial)
        let result = curve
            .swap_without_fees(
                6521908914483738081, // amount B in = diff between B values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                85070591833667672584493520953498751556,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 6521908914483738081);
        assert_eq!(
            result.destination_amount_swapped,
            // same note as above - slightly off from exact amount of
            // 63802943861245061694031837281503533219
            63802943845173758276417067327404556546 // amount A out = diff between A values
        );

        // now (with actual A numbers above), swap balance is 21267647988493914308076453626094195010
        // 21267647988493914308076453626094195010 <- A value at B = 6521908916947940451.00
        // 0 <- A value at B = 0
        let result = curve
            .swap_without_fees(
                // note due to sqrt rounding this requires more than the actual amount
                // of 6521908916947940451
                6521908918180041637, // amount B in = diff between B values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                21267647988493914308076453626094195010,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 6521908918180041637);
        assert_eq!(
            result.destination_amount_swapped,
            21267647988493914308076453626094195010 // amount A out = diff between A values
        );

        // note we got out 26087635639488208246 b tokens at the end of a->b and
        // we put in 26087635652407883841 b tokens at the end of b->a (to get all the a back
        // out) - it's off by a few since we rounded such that there's no arbitrage opportunity

        // same as above but with a huge token b, make sure we only take the required amount
        let result = curve
            .swap_without_fees(
                u128::MAX, // way more token b than needed to get all the token a out
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                21267647988493914308076453626094195010,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(
            result.source_amount_swapped,
            6521908918180041637 // should still only take this much B
        );
        assert_eq!(
            result.destination_amount_swapped,
            21267647988493914308076453626094195010
        );
    }

    /// These swap_large_price_foo tests all test the overflow boundaries of u64/u128 test - mostly just to give
    /// some example curves with large numbers (and make sure they return None instead of panicking etc)
    /// They also test that rounding is always not in the user's favor to prevent arbitrage
    /// Summary: with a very low slope, overflow isn't an issue, though often times rounding and PreciseNumber's
    /// limit of 18 decimals of precision will cause rounding well below the exact solution
    #[test]
    fn swap_large_price_low_slope_u128() {
        // example curve with lowest possible slope and 0 starting A price (costs very little A to get a lot of B out)
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            // since PreciseNumber only has 18 decimals of precision, anything that
            // doesn't divide evenly or is < 1e-18 will be treated as 0 slope
            slope_denominator: 1_000_000_000_000_000_000,
            initial_token_a_price_numerator: 0,
            initial_token_a_price_denominator: 1,
        };

        // 0 <- B value at A = 0
        // 1414213562.37 <- B value at A = 1
        let result = curve.swap_without_fees(1, 0, u128::MAX - 1, TradeDirection::AtoB);
        assert_eq!(
            result.unwrap(),
            SwapWithoutFeesResult {
                source_amount_swapped: 1,
                // due to sqrt rounding, slightly lower than real value of 1414213562
                destination_amount_swapped: 1414213561
            }
        );

        // 0 <- B value at A = 0
        // 26087635650665564424699143612.51 <- B value at A = 2^128-1
        let result = curve.swap_without_fees(u128::MAX, 0, u128::MAX - 1, TradeDirection::AtoB);
        assert_eq!(
            result.unwrap(),
            SwapWithoutFeesResult {
                source_amount_swapped: u128::MAX,
                // due to sqrt precision, this is slightly off from exact value of
                // 26087635650665564424699143612
                destination_amount_swapped: 26087635642281361408000000000
            }
        );

        // testing b -> a on the same curve

        // put all 26087635642281361408000000000 B back in, should get all u128 max out
        let result = curve
            .swap_without_fees(
                26087635642281361408000000000, // amount B in = diff between B values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                u128::MAX,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 26087635642281361408000000000);
        assert_eq!(
            result.destination_amount_swapped,
            // due to sqrt precision, this is slightly rounded down from exact value of
            // 340282366920938463463374607431768211455 (u128 max max)
            340282366920938463426481119284349108223 // amount A out = diff between A values
        );

        // 128::MAX <- A value at B = 26087635642281361408000000000
        // 85070591675553607494415921514238967808 <- A value at B = 13043817821140680704000000000 (halfway to initial B)
        let result = curve
            .swap_without_fees(
                13043817821140680704000000000, // amount B in = diff between B values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                u128::MAX,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 13043817821140680704000000000);
        assert_eq!(
            result.destination_amount_swapped,
            // due to sqrt precision, this is slightly off from exact value of
            // 255211775245384855968958685917529243647
            255211775133339314018502795692393627647 // amount A out = diff between A values
        );

        // now (with actual A numbers above), swap balance is 85070591787599149444871811739374583808
        // 85070591787599149444871811739374583808 <- A value at B = 13043817829730615296000000000
        //  (using the rounded A value from above to make sure the rounding doesn't cause any compounding issues)
        // 21267647946899787361217952934843645952 <- A value at B = 6521908914865307648000000000
        //  (another halfway down to initial)
        let result = curve
            .swap_without_fees(
                6521908914865307648000000000, // amount B in = diff between B values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                85070591787599149444871811739374583808,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 6521908914865307648000000000);
        assert_eq!(
            result.destination_amount_swapped,
            63802943840699362083653858804530937856 // amount A out = diff between A values
        );

        // now (with actual A numbers above), swap balance is 21267647946899787361217952934843645952
        // 21267647946899787361217952934843645952 <- A value at B = 6521908914865307648000000000
        // 0 <- A value at B = 0
        let result = curve
            .swap_without_fees(
                6521908914865307648000000000, // amount B in = diff between B values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                21267647946899787361217952934843645952,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 6521908914865307648000000000);
        assert_eq!(
            result.destination_amount_swapped,
            21267647946899787361217952934843645952 // amount A out = diff between A values
        );

        // note we got out 26087635642281361408000000000 b tokens at the end of a->b and
        // we put in 26087635650871296000000000000 b tokens at the end of b->a (to get all the a back
        // out) - this is due to rounding down sqrt issues (safely, not in the user's favor)

        // same as above but with a huge token b, make sure we only take the required amount
        let result = curve
            .swap_without_fees(
                u128::MAX, // way more token b than needed to get all the token a out
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                21267647946899787361217952934843645952,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(
            result.source_amount_swapped,
            6521908914865307648000000000 // should still only take this much B
        );
        assert_eq!(
            result.destination_amount_swapped,
            21267647946899787361217952934843645952
        );
    }

    /// These swap_large_price_foo tests all test the overflow boundaries of u64/u128 test - mostly just to give
    /// some example curves with large numbers (and make sure they return None instead of panicking etc)
    /// They also test that rounding is always not in the user's favor to prevent arbitrage
    /// This is similar to swap_large_price_low_slope_u128 but we only go up to u64 (which is more realistic
    /// since SPL maxes out at u64)
    #[test]
    fn swap_large_price_low_slope_u64() {
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 1_000_000_000_000_000_000,
            initial_token_a_price_numerator: 0,
            initial_token_a_price_denominator: 1,
        };

        // same as above but we only use u64 values (realistically that's the maximum unless SPL
        // max supply changes)
        // 0 <- B value at A = 0
        // 6074000999952099384.73 <- B value at A = 2^64-1
        let result =
            curve.swap_without_fees(u64::MAX.into(), 0, u128::MAX - 1, TradeDirection::AtoB);
        assert_eq!(
            result.unwrap(),
            SwapWithoutFeesResult {
                source_amount_swapped: u64::MAX.into(),
                // due to sqrt precision, this is slightly off from exact value of
                // 6074000999952099384
                destination_amount_swapped: 6074000998000000000
            }
        );

        // testing b -> a on the same curve

        // put all 6074000998000000000 B back in, should get all u64 max out
        let result = curve
            .swap_without_fees(
                6074000998000000000, // amount B in = diff between B values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                u64::MAX.into(),
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 6074000998000000000);
        assert_eq!(
            result.destination_amount_swapped,
            // due to sqrt precision, this is slightly rounded down from exact value of
            // 18446744073709551615 (u64 max)
            18446744073709551613 // amount A out = diff between A values
        );

        // swap from initial A locked of u64 max all the way down to 0 - make sure
        // any rounding is not in user's favor to prevent arbitrage

        // u64 max <- A value at B = 6074000998000000000
        // 4611686015463124500.50 <- A value at B = 3037000499000000000 (~halfway to initial B)
        let result = curve
            .swap_without_fees(
                3037000499000000000, // amount B in = diff between B values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                u64::MAX.into(),
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 3037000499000000000);
        assert_eq!(
            result.destination_amount_swapped,
            // due to sqrt precision, this is slightly off from exact value of
            // 13835058058246427115
            13835058052172426114 // amount A out = diff between A values
        );

        // now (with actual A numbers above), swap balance is 4611686021537125501
        // 4611686021537125501 <- A value at B = 3037000501000000000.2
        //  (using the rounded A value from above to make sure the rounding doesn't cause any compounding issues)
        // 1152921505384281375.1 <- A value at B = 1518500250500000000
        //  (another halfway down to initial)
        let result = curve
            .swap_without_fees(
                1518500250500000000, // amount B in = diff between B values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                4611686021537125501,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 1518500250500000000);
        assert_eq!(
            result.destination_amount_swapped,
            // same note as above - slightly off from exact amount of
            // 3458764516152844126
            3458764514634343874 // amount A out = diff between A values
        );

        // now (with actual A numbers above), swap balance is 1152921506902781627
        // 1152921506902781627 <- A value at B = 1518500251500000000.6
        // 0 <- A value at B = 0 (b initial)
        let result = curve
            .swap_without_fees(
                // due to sqrt rounding, requires more token B than the exact value of 1518500251500000000
                1518500252000000000, // amount B in = diff between B values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                1152921506902781627,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 1518500252000000000);
        assert_eq!(
            result.destination_amount_swapped,
            1152921506902781627 // amount A out = diff between A values
        );

        // note we got out 6074000998000000000 b tokens at the end of a->b and
        // we put in 6074001001500000000 b tokens at the end of b->a (to get all the a back
        // out) - this is due to rounding down sqrt issues (safely, not in the user's favor)

        // same as above but with a huge token b, make sure we only take the required amount
        let result = curve
            .swap_without_fees(
                u128::MAX, // way more token b than needed to get all the token a out
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                1152921506902781627,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(
            result.source_amount_swapped,
            1518500252000000000 // should still only take this much B
        );
        assert_eq!(result.destination_amount_swapped, 1152921506902781627);
    }

    /// These swap_large_price_foo tests all test the overflow boundaries of u64/u128 test - mostly just to give
    /// some example curves with large numbers (and make sure they return None instead of panicking etc)
    /// They also test that rounding is always not in the user's favor to prevent arbitrage
    /// Summary: Having both initial_a_price numerator and denominator very large doesn't affect the math/doesn't
    /// introduce any new rounding issues
    #[test]
    fn swap_large_initial_a_num_and_den() {
        // curve with everything near u64::MAX
        let curve = LinearPriceCurve {
            slope_numerator: u64::MAX,
            slope_denominator: u64::MAX - 1,
            initial_token_a_price_numerator: u64::MAX - 1,
            initial_token_a_price_denominator: u64::MAX,
        };

        // testing a -> b

        // put in 2^64 - 1 A tokens, should move B value from
        // 0 <- B value at A = 0
        // 6074000998.95 <- B value at A = 2^64 - 1 (should floor down)
        let result = curve.swap_without_fees(
            (u64::MAX).into(),
            0,
            1_00000_00000_00000_00000,
            TradeDirection::AtoB,
        );
        assert_eq!(
            result.unwrap(),
            SwapWithoutFeesResult {
                source_amount_swapped: u64::MAX.into(),
                // due to sqrt precision, this is slightly off from exact value of
                // 60740_00998
                destination_amount_swapped: 60740_00997
            }
        );

        // testing b -> a on the same curve
        // 18446744073709551615 <- A value at B = 6074000998.95
        // 4611686018500124999.75 <- A value at B = 3037000499
        let result = curve
            .swap_without_fees(
                3037000499, // amount B in = diff between B values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                18446744073709551615,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 3037000499);
        assert_eq!(
            result.destination_amount_swapped,
            // note because of sqrt precision, this is slightly different than the exact amount of
            // 13835058055209426615
            13835058049135425613 // amount A out = diff between A values
        );

        // now (with actual A numbers above), swap balance is 4611686021537125501
        // 4611686021537125501 <- A value at B = 3037000500.00
        //  (using the rounded A value from above to make sure the rounding doesn't cause any compounding issues)
        // 1152921506143531500.06 <- A value at B = 1518500250
        //  (another halfway down to initial)
        let result = curve
            .swap_without_fees(
                1518500250, // amount B in = diff between B values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                4611686021537125501,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 1518500250);
        assert_eq!(
            result.destination_amount_swapped,
            // note because of sqrt precision, this is slightly different than the exact amount of
            // 3458764515393594001
            3458764513875093749 // amount A out = diff between A values
        );

        // now (with actual A numbers above), swap balance is 1152921507662031752
        // 1152921507662031752 <- A value at B = 1518500251.00
        // 0 <- A value at B = 0
        let result = curve
            .swap_without_fees(
                // note due to sqrt rounding this requires 1 more than the actual amount
                // (it ends up only taking 1518500251 as expected though)
                1518500252, // amount B in = diff between B values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                1152921507662031752,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 1518500251);
        assert_eq!(
            result.destination_amount_swapped,
            1152921507662031752 // amount A out = diff between A values
        );

        // note we got out 6074000997 b tokens at the end of a->b and
        // we put in 6074001000 b tokens at the end of b->a (to get all the a back
        // out) - it's off by a few since we rounded such that there's no arbitrage opportunity

        // same as above but with a huge token b, make sure we only take the required amount
        let result = curve
            .swap_without_fees(
                u128::MAX, // way more token b than needed to get all the token a out
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                1152921507662031752,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(
            result.source_amount_swapped,
            1518500251 // should still only take this much B
        );
        assert_eq!(result.destination_amount_swapped, 1152921507662031752);
    }

    /// These swap_large_price_foo tests all test the overflow boundaries of u64/u128 test - mostly just to give
    /// some example curves with large numbers (and make sure they return None instead of panicking etc)
    /// They also test that rounding is always not in the user's favor to prevent arbitrage
    /// Summary: Similar to swap_large_price_max_a and swap_large_price_a_u32, there's only a narrow band
    /// of token A swapped in where we can get any token B out (if at all) before reaching the u64 supply limit
    #[test]
    fn swap_maximum_slope() {
        // curve with slope = u64::MAX
        let curve = LinearPriceCurve {
            slope_numerator: u64::MAX,
            slope_denominator: 1,
            initial_token_a_price_numerator: u64::MAX - 1,
            initial_token_a_price_denominator: u64::MAX,
        };

        // before putting in 2^63 A tokens, there's not enough to get any B tokens out
        for exp in 0..63 {
            let result = curve.swap_without_fees(
                2_u128.pow(exp),
                0,
                1_00000_00000_00000_00000,
                TradeDirection::AtoB,
            );
            assert!(result.is_none());
        }

        // there's a narrow band where we can swap in a bunch of token A for
        // a small amount of token B (depending how high the slope is, this band will be wider)
        let result = curve
            .swap_without_fees(
                u64::MAX.into(),
                0,
                1_00000_00000_00000_00000,
                TradeDirection::AtoB,
            )
            .unwrap();

        assert_eq!(
            result,
            SwapWithoutFeesResult {
                source_amount_swapped: u64::MAX as u128,
                destination_amount_swapped: 1
            }
        );

        // putting in u128 max tokens works too, so this won't ever overflow (though we're past spl token supply now)
        let result = curve
            .swap_without_fees(
                u128::MAX,
                0,
                1_00000_00000_00000_00000,
                TradeDirection::AtoB,
            )
            .unwrap();
        assert_eq!(
            result,
            SwapWithoutFeesResult {
                source_amount_swapped: u128::MAX,
                // a little rounded down from real value of 6.0740009999e9
                destination_amount_swapped: 6074000998
            }
        );
    }

    /// These swap_large_price_foo tests all test the overflow boundaries of u64/u128 test - mostly just to give
    /// some example curves with large numbers (and make sure they return None instead of panicking etc)
    /// They also test that rounding is always not in the user's favor to prevent arbitrage
    /// Summary: At slightly lower slope than max, we can at least get a little token B out before the u64 limit,
    /// how much depends how close to u64::MAX the slope is. The rounding errors are very drastic at this high
    /// slope though
    #[test]
    fn swap_very_large_slope() {
        // curve with slope = 2^59 (this is about the max slope to get > 4 B tokens out before overflowing - makes
        // for a better b->a test below)
        let curve = LinearPriceCurve {
            slope_numerator: 2u64.pow(59),
            slope_denominator: 1,
            initial_token_a_price_numerator: u64::MAX - 1,
            initial_token_a_price_denominator: u64::MAX,
        };

        // testing a -> b

        // put in 2^64 - 1 A tokens, should move B value from
        // 0 <- B value at A = 0
        // 7.99 <- B value at A = 2^64 - 1 (should floor down)
        let result = curve.swap_without_fees(
            (u64::MAX).into(),
            0,
            1_00000_00000_00000_00000,
            TradeDirection::AtoB,
        );
        assert_eq!(
            result.unwrap(),
            SwapWithoutFeesResult {
                source_amount_swapped: u64::MAX.into(),
                destination_amount_swapped: 7
            }
        );

        // testing b -> a on the same curve
        // 18446744073709551615 <- A value at B = 7.99
        // 4611686018427387908.00 <- A value at B = 4
        let result = curve
            .swap_without_fees(
                3, // amount B in = diff between B values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                18446744073709551615,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 3);
        assert_eq!(
            result.destination_amount_swapped,
            // note because of the drastic slope, the rounding error from sqrt is a lot from the exact value of
            // 13835058055282163707
            11240984669916758010 // amount A out = diff between A values
        );

        // now (with actual A numbers above), swap balance is 7205759403792793605
        // 7205759403792793605 <- A value at B = 5.00
        //  (using the rounded A value from above to make sure the rounding doesn't cause any compounding issues)
        // 1152921504606846978.00 <- A value at B = 2
        let result = curve
            .swap_without_fees(
                2, // amount B in = diff between B values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                7205759403792793605,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 2);
        assert_eq!(
            result.destination_amount_swapped,
            // same note as above - rounded off from exact amount of
            // 6052837899185946627
            2594073385365405697 // amount A out = diff between A values
        );

        // now (with actual A numbers above), swap balance is 2594073385365405699
        // 2594073385365405699 <- A value at B = 3.00
        // 0 <- A value at B = 0
        let result = curve
            .swap_without_fees(
                3, // amount B in = diff between B values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                2594073385365405699,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 3);
        assert_eq!(
            result.destination_amount_swapped,
            2305843009213693954 // amount A out = diff between A values
        );

        // note we got out 7 b tokens at the end of a->b and
        // we put in 8 b tokens at the end of b->a (to get all the a back
        // out) - it's off by 1 since we rounded such that there's no arbitrage opportunity

        // same as above but with a huge token b, make sure we only take the required amount
        let result = curve
            .swap_without_fees(
                u128::MAX, // way more token b than needed to get all the token a out
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                2594073385365405699,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(
            result.source_amount_swapped,
            3 // should still only take this much B
        );
        assert_eq!(result.destination_amount_swapped, 2594073385365405699);
    }

    /// Tests curve.validate() will reject invalid curves
    #[test]
    fn swap_is_slope_valid() {
        // 0 should be Err
        let curve = LinearPriceCurve {
            slope_numerator: 0,
            slope_denominator: 1_000_000_000_001,
            initial_token_a_price_numerator: 1,
            initial_token_a_price_denominator: 1,
        };
        assert!(!curve.validate().is_ok());

        // undef should be Err
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 0,
            initial_token_a_price_numerator: 1,
            initial_token_a_price_denominator: 1,
        };
        assert!(!curve.validate().is_ok());

        // less than 1e-18 should be Err
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 1_000_000_000_000_000_001,
            initial_token_a_price_numerator: 1,
            initial_token_a_price_denominator: 1,
        };
        assert!(!curve.validate().is_ok());

        // 1e-18 should be Ok
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 1_000_000_000_000_000_000,
            initial_token_a_price_numerator: 1,
            initial_token_a_price_denominator: 1,
        };
        assert!(curve.validate().is_ok());

        // taki curve - should be Ok
        let curve = LinearPriceCurve {
            slope_numerator: 37,
            slope_denominator: 1_400_000_000_000_000_000,
            initial_token_a_price_numerator: 1,
            initial_token_a_price_denominator: 1,
        };
        assert!(curve.validate().is_ok());

        // undef token a price should be ERr
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 1_000_000_000_000,
            initial_token_a_price_numerator: 1,
            initial_token_a_price_denominator: 0,
        };
        assert!(!curve.validate().is_ok());
    }

    /// Tests swapping the minimum amount of tokens at a time (e.g. 1) in a loop from 0 to max and
    /// then back to 0, making sure there's no rounding arbitrage opportunities. Useful for sanity checking
    /// specific swap steps for a specific curve (e.g. one about to be created on mainnet)
    #[test]
    fn minimum_token_exchange_rounding() {
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 1_000_000_000_000,
            initial_token_a_price_numerator: 0,
            initial_token_a_price_denominator: 1,
        };
        let starting_supply_b: u128 = 10_000_000;
        // swap at least `step` tokens at a time, can tweak this if it takes a lot of token a to get out 1 token b
        // (would be even better to use something analogous to the next_b_value/current_b_value that we use below)
        let step = 1;

        let mut swap_supply_a = 0;
        let mut swap_supply_b: u128 = starting_supply_b.into();

        while swap_supply_b > 0 {
            let mut amount_a = step;
            loop {
                let result = curve.swap_without_fees(
                    amount_a,
                    swap_supply_a,
                    swap_supply_b,
                    TradeDirection::AtoB,
                );

                if result.is_some() {
                    let SwapWithoutFeesResult {
                        source_amount_swapped,
                        destination_amount_swapped,
                    } = result.unwrap();
                    swap_supply_a += source_amount_swapped;
                    swap_supply_b -= destination_amount_swapped;

                    // uncomment to see every token step:
                    // msg!(
                    //     "Swapped {:?} token a (bal {:?}) for {:?} token b (bal {:?})",
                    //     source_amount_swapped,
                    //     swap_supply_a,
                    //     destination_amount_swapped,
                    //     swap_supply_b,
                    // );
                    break;
                } else {
                    // if result was none, there wasn't enough a token to get out any b, so try a bit more
                    amount_a += step;
                }
            }
        }

        // at this point, swap has 0 b and has taken in `swap_supply_a` amount of token a
        assert!(swap_supply_b == 0);
        assert!(swap_supply_a > 0);

        // now swap all the way back from b to a
        while swap_supply_a > 0 {
            // usually (for small slope curves), it takes a lot of b to get back 1 a,
            // so just precalculate a reasonable starting point instead of starting from 1
            let current_b_value = curve
                .b_value_with_amt_a_locked_quadratic(
                    &(PreciseNumber::new(swap_supply_a).unwrap()),
                    false,
                )
                .unwrap()
                .to_imprecise()
                .unwrap();

            let next_b_value = curve
                .b_value_with_amt_a_locked_quadratic(
                    &(PreciseNumber::new(swap_supply_a - 1).unwrap()),
                    true,
                )
                .unwrap()
                .to_imprecise()
                .unwrap();

            let mut amount_b = current_b_value - next_b_value - 10;
            loop {
                let result = curve.swap_without_fees(
                    amount_b,
                    swap_supply_b,
                    swap_supply_a,
                    TradeDirection::BtoA,
                );

                if result.is_some() {
                    let SwapWithoutFeesResult {
                        source_amount_swapped,
                        destination_amount_swapped,
                    } = result.unwrap();
                    swap_supply_b += source_amount_swapped;
                    swap_supply_a -= destination_amount_swapped;

                    // uncomment to see every token step:
                    // msg!(
                    //     "Swapped {:?} token b (bal {:?}) for {:?} token a (bal {:?})",
                    //     source_amount_swapped,
                    //     swap_supply_b,
                    //     destination_amount_swapped,
                    //     swap_supply_a,
                    // );
                    break;
                } else {
                    // if result was none, there wasn't enough a token to get out any b, so try a bit more
                    amount_b += step;
                }
            }
        }

        // make sure some user can't get out all the a while making a profit on b, i.e.
        // the swap should now have more b in it than we started with
        assert!(swap_supply_a == 0);
        assert!(swap_supply_b >= starting_supply_b);
    }

    // TODO: there's a bunch of withdraw/deposit tests from constant_curve that we could write a version of if we
    // enable those, e.g. curve_value_does_not_decrease_from_withdraw/deposit, deposit_token_conversion_b_to_a/b_to_a,
    // withdraw_token_conversion

    proptest! {
        #[test]
        fn curve_value_does_not_decrease_from_swap_a_to_b(
            // how much a user is swapping in
            source_token_amount in 1..u64::MAX,
            // how much a is already in swap (determines spot price), for a low slope curve we might overflow
            // if we go all the way to u64::MAX
            swap_source_amount in 1..u32::MAX,
        ) {
            // Kind of complicated to check overflow/underflow if we try to also make these parameterized in proptest
            // (and we'd basically be using the functions we're trying to test to do those prop_assume checks, so
            // kind of pointless), probably just makes sense to run this test on a specific curve to sanity check it
            // (and tweak the source_token_amount/swap_source_amount ranges to not overflow)
            let curve = LinearPriceCurve {
                slope_numerator: 1,
                slope_denominator: 1_000_000_000_000,
                initial_token_a_price_numerator: 0,
                initial_token_a_price_denominator: 1,
            };

            let (_source_amount_swapped, destination_amount_swapped) = curve
                .swap_a_to_b(
                    source_token_amount as u128,
                    swap_source_amount as u128,
                    u64::MAX as u128,
                )
                .unwrap();

            // ignore the trades where not enough source_token_amount was put in to get any b out
            if destination_amount_swapped > 0 {
                check_curve_value_from_swap(
                    &curve,
                    source_token_amount as u128,
                    swap_source_amount as u128,
                    // swap_destination_amount (the amount of token b in the swap) doesn't affect any of the math so
                    // no need to parameterize it, it would just introduces extra prop_assume checks at the top)
                    u64::MAX as u128,
                    TradeDirection::AtoB
                );
            }
        }
    }

    proptest! {
        #[test]
        fn curve_value_does_not_decrease_from_swap_b_to_a(
            // how much b user is swapping in
            source_token_amount in 1..u64::MAX,
            // how much a is already in swap (determines spot price), for a low slope curve we might overflow
            // if we go all the way to u64::MAX
            swap_destination_amount in 1..u32::MAX,
        ) {
            // Same as above - too complicated to parametrize all these in proptest
            let curve = LinearPriceCurve {
                slope_numerator: 1,
                slope_denominator: 1_000_000_000_000,
                initial_token_a_price_numerator: 0,
                initial_token_a_price_denominator: 1,
            };

            let (_source_amount_swapped, destination_amount_swapped) = curve
                .swap_b_to_a(
                    source_token_amount as u128,
                    u64::MAX as u128,
                    swap_destination_amount as u128,
                )
                .unwrap();

            // ignore the trades where not enough source_token_amount was put in to get any a out
            if destination_amount_swapped > 0 {
                check_curve_value_from_swap(
                    &curve,
                    source_token_amount as u128,
                    // swap_source_amount (the amount of token b in the swap) doesn't affect any of the math so
                    // no need to parameterize it, it would just introduces extra prop_assume checks at the top)
                    u64::MAX as u128,
                    swap_destination_amount as u128,
                    TradeDirection::BtoA
                );
            }
        }
    }

    /// Sanity check tests for solve_quadratic_positive_root helper function
    #[test]
    fn solve_quadratic_positive_root_cases() {
        // e == 0
        // x^2 - 25, x = -5 and 5
        let result = solve_quadratic_positive_root(
            &(PreciseNumber::new(1).unwrap()),
            &(PreciseNumber::new(1).unwrap()),
            &(PreciseNumber::new(0).unwrap()),
            &(PreciseNumber::new(1).unwrap()),
            &(PreciseNumber::new(25).unwrap()),
            true,
        )
        .unwrap()
        .to_imprecise()
        .unwrap();
        assert_eq!(result, 5); // should return positive root

        // c == 0
        // x^2 + 5x, x = -5 and 0
        let result = solve_quadratic_positive_root(
            &(PreciseNumber::new(5).unwrap()),
            &(PreciseNumber::new(5).unwrap()), // not reducing to test division
            &(PreciseNumber::new(10).unwrap()),
            &(PreciseNumber::new(2).unwrap()), // not reducing to test division
            &(PreciseNumber::new(0).unwrap()),
            true,
        )
        .unwrap()
        .to_imprecise()
        .unwrap();
        assert_eq!(result, 0); // should return positive root

        // all nonzero
        // x^2 + 4x - 5, x = -5 and 1
        let result = solve_quadratic_positive_root(
            &(PreciseNumber::new(3).unwrap()),
            &(PreciseNumber::new(3).unwrap()), // not reducing to test division
            &(PreciseNumber::new(28).unwrap()),
            &(PreciseNumber::new(7).unwrap()), // not reducing to test division
            &(PreciseNumber::new(5).unwrap()),
            true,
        )
        .unwrap()
        .to_imprecise()
        .unwrap();
        assert_eq!(result, 1); // should return positive root
    }
}
