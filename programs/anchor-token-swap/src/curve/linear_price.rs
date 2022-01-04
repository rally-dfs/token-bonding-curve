//! Linear price swap curve, slope and initial price point set at init
//! Currently this (especially `swap`) only works under the following assumptions:
//! Deposits (except the initial deposit) are disabled
//! The initial deposit should only have token B (the bonded token) and 0 token A (the collateral token)
//! This curve only works with fees set to 0 (process_swap will panic otherwise)
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

/// Returns the positive root of x given 0 = a*x^2 + bx + c
/// a is assumed to always be positive
fn solve_quadratic_positive_root(
    a_numerator: &PreciseNumber,
    a_denominator: &PreciseNumber,
    b_abs_value: &PreciseNumber,
    b_is_negative: bool,
    c_abs_value: &PreciseNumber,
    c_is_negative: bool,
    should_round_sqrt_up: bool,
) -> Option<PreciseNumber> {
    // TODO: should write some tests for this

    // solve x = (-b + sqrt(b^2 - 4ac)) / 2a

    // a * 4 * c
    let four_a_c_abs_value = a_numerator
        .checked_mul(&(PreciseNumber::new(4)?))?
        .checked_mul(c_abs_value)?
        .checked_div(a_denominator)?;

    // b^2 - four_a_c
    let b_squared = b_abs_value.checked_mul(b_abs_value)?;
    let b2_minus_4ac = match c_is_negative {
        // b^2 - (-|4ac|)
        true => b_squared.checked_add(&four_a_c_abs_value)?,
        // b^2 - (+|4ac|)
        false => b_squared.checked_sub(&four_a_c_abs_value)?, // we're going to sqrt this so no need for unsigned_sub
    };

    let sqrt_b2_minus_4ac_raw;
    if b2_minus_4ac.less_than(&(PreciseNumber::new(1)?)) {
        // PreciseNumber's out of the box sqrt is too inaccurate for numbers < 1, add as many
        // decimals as we can fit into u128 (12 max from PreciseNumber + 26) and then do it manually
        // TODO: this would be cleaner if we also encapsulated it into DFSPreciseNumber.sqrt
        let b2_minus_4ac_value =
            b2_minus_4ac.checked_mul(&(PreciseNumber::new(100_0000_0000_0000_0000_0000_0000)?))?;
        let b2_minus_4ac_value_u128 = b2_minus_4ac_value.to_imprecise()?;
        let sqrt_b2_minus_4ac_value_u128 = sqrt_babylonian(b2_minus_4ac_value_u128)?;

        // convert back to PreciseNumber and divide result by 13 decimals
        sqrt_b2_minus_4ac_raw = PreciseNumber::new(sqrt_b2_minus_4ac_value_u128)?
            .checked_div(&(PreciseNumber::new(10_0000_0000_0000)?))?;
    } else {
        // note we have to use u128 sqrt since PreciseNumber::sqrt is really expensive (~100K compute vs ~50K compute)
        let b2_minus_4ac_u128 =
            match b2_minus_4ac.less_than_or_equal(&(PreciseNumber::new(u128::MAX)?)) {
                true => b2_minus_4ac.to_imprecise()?,
                // to_imprecise panics instead of returning None so need to handle explicitly here
                // TODO: should do this everywhere else we call to_imprecise also, probably put into a DFSPreciseNumber wrapper
                false => return None,
            };
        let sqrt_b2_minus_4ac_u128 = sqrt_babylonian(b2_minus_4ac_u128)?;
        sqrt_b2_minus_4ac_raw = PreciseNumber::new(sqrt_b2_minus_4ac_u128)?;
    }

    // make sure to add 1 if we're supposed to round up (and it wasn't a perfect square)
    let sqrt_b2_minus_4ac = match should_round_sqrt_up
        && sqrt_b2_minus_4ac_raw
            .checked_mul(&sqrt_b2_minus_4ac_raw)?
            .less_than(&b2_minus_4ac)
    {
        true => sqrt_b2_minus_4ac_raw.checked_add(&(PreciseNumber::new(1)?))?,
        false => sqrt_b2_minus_4ac_raw,
    };

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

    // finally we return (sqrt(b^2-4ac) - b)/2a,
    // AKA numerator * a_denominator / a_numerator / 2 (do all the division last)
    numerator
        .checked_mul(a_denominator)?
        .checked_div(&a_numerator)?
        .checked_div(&(PreciseNumber::new(2)?))
}

/// These functions use the integral of the linear price curve to determine liquidity of R at a
/// given C value (amt_r_locked_at_c_value_quadratic)
/// It also uses the quadratic formula to solve the same integral to determine the C value for a given
/// liquidity (c_value_with_amt_r_locked_quadratic)
///
/// swap_a_to_b_quadratic and swap_b_to_a_quadratic are the key functions at the bottom
/// the sqrt function drops down to u128 so we don't use all our compute but everything else uses PreciseNumber
impl LinearPriceCurve {
    /// Returns the coefficients a, b, b_is_negative, i, i_is_negative in the liquidity integral
    /// token_r_bonded = 0.5m*c^2 + (r0 - m*c0)*c + i0
    /// a == 0.5m, b == (r0 - m*c0), i0 == integration constant when 0 collateral token is locked at c0
    /// Note a is returned as a fraction since returning it as a pre-divided PreciseNumber loses a lot of
    /// precision (only 12 decimal digits max) - we're going to be multiplying it against prices (a*c^2) so
    /// no need to lose that precision (and as long as slope_numerator/price are all u64 there's plenty of
    /// room in PreciseNumber to avoid overflow)
    fn liquidity_curve_quadratic_constants(
        &self,
    ) -> Option<(
        PreciseNumber, // a numerator
        PreciseNumber, // a denominator
        PreciseNumber, // b abs value
        bool,          // b is negative
        PreciseNumber, // i0 abs value
        bool,          // i0 is negative
    )> {

        let slope_numerator = PreciseNumber::new(self.slope_numerator.into())?;
        let slope_denominator = PreciseNumber::new(self.slope_denominator.into())?;
        let r0 = PreciseNumber::new(self.initial_token_r_price.into())?;
        let c0 = PreciseNumber::new(self.initial_token_c_price.into())?;

        // a == 0.5m, precalculate a*c0^2 (do the division last so we don't lose precision)
        let a_numerator = slope_numerator.checked_mul(&(PreciseNumber::new(1)?))?;
        let a_denominator = slope_denominator.checked_mul(&(PreciseNumber::new(2)?))?;
        let a_c0_squared_numerator = a_numerator.checked_mul(&c0)?.checked_mul(&c0)?;
        let a_c0_squared = a_c0_squared_numerator
            .checked_div(&slope_denominator)?
            .checked_div(&(PreciseNumber::new(2)?))?; // this is a little more precise if we divide here instead of using a_denominator

        // TODO: rewrite everything using foo_is_positive instead of foo_is_negative, probably way easier to read

        // b == r0 - m*c0 (need to use unsigned_sub here to handle negatives)
        let mc0 = slope_numerator
            .checked_mul(&c0)?
            .checked_div(&slope_denominator)?;
        let (b_abs_value, b_is_negative) = r0.unsigned_sub(&mc0);

        // calculate integration constant i0 when 0 collateral token is locked at c0,
        // i.e. 0 = a*c0^2 + b*c0 + i
        let (i0_abs_value, i0_is_negative);
        if b_is_negative {
            // since a is always positive, it's a little cleaner to solve for -i = a*c0^2 + b*c0
            // instead of working with all the negatives with PreciseNumber
            let negative_i0_info = a_c0_squared.unsigned_sub(&(b_abs_value.checked_mul(&c0)?));
            i0_abs_value = negative_i0_info.0; // abs value doesn't change from -i to i
            i0_is_negative = !negative_i0_info.1; // i_is_negative is opposite of whether negative_i is negative
        } else {
            // a and b are both positive so i is always negative
            i0_abs_value = a_c0_squared.checked_add(&(b_abs_value.checked_mul(&c0)?))?;
            i0_is_negative = true;
        }

        Some((
            a_numerator,
            a_denominator,
            b_abs_value,
            b_is_negative,
            i0_abs_value,
            i0_is_negative,
        ))
    }

    /// Returns the amount of R token locked at a given c_value (by plugging c_value into the integral function)
    /// Since the return must be positive, this only works for C > initial_c_value (there are potentially rounding
    /// errors when c_value == initial_c_value where a very small negative PreciseNumber should round up to 0, so
    /// best not to call this with c_value == initial_c_value either)
    fn amt_r_locked_at_c_value_quadratic(&self, c_value: &PreciseNumber) -> Option<PreciseNumber> {
        let (a_numerator, a_denominator, b_abs_value, b_is_negative, i0_abs_value, i0_is_negative) =
            self.liquidity_curve_quadratic_constants()?;

        let a_price_squared = a_numerator
            .checked_mul(c_value)?
            .checked_mul(c_value)?
            .checked_div(&a_denominator)?;
        let b_price_abs_value = b_abs_value.checked_mul(c_value)?;

        // there's some rounding errors at the edges that can cause 0 to look like slightly negative numbers when calling
        // PreciseNumber subtraction, so need to correctly treat that as 0
        let (amount_locked, amount_is_negative) = if b_is_negative && i0_is_negative {
            let total_to_subtract = b_price_abs_value.checked_add(&i0_abs_value)?;

            a_price_squared.unsigned_sub(&total_to_subtract)
        } else if b_is_negative {
            a_price_squared
                .checked_add(&i0_abs_value)?
                .unsigned_sub(&b_price_abs_value)
        } else if i0_is_negative {
            a_price_squared
                .checked_add(&b_price_abs_value)?
                .unsigned_sub(&i0_abs_value)
        } else {
            (
                a_price_squared
                    .checked_add(&b_price_abs_value)?
                    .checked_add(&i0_abs_value)?,
                false,
            )
        };

        // only allow negatives if it's close to 0
        // TODO: DFSPreciseNumber should fix this ugliness, we're just wrongly
        // returning the positive version of a small negative number instead :(
        if amount_is_negative && amount_locked.to_imprecise()? != 0 {
            return None;
        }

        Some(amount_locked)
    }

    /// Returns the positive root for token_r_amount = 0.5m*c^2 + (r0 - m*c0)*c + i0
    /// token_r_amount is assumed to always be >= 0 (i.e. no negative amounts of collateral token allowed)
    /// i is the integration constant such that 0 collateral token is locked at c0
    fn c_value_with_amt_r_locked_quadratic(
        &self,
        token_r_amount: &PreciseNumber,
        should_round_sqrt_up: bool,
    ) -> Option<PreciseNumber> {
        let (a_numerator, a_denominator, b_abs_value, b_is_negative, i0_abs_value, i0_is_negative) =
            self.liquidity_curve_quadratic_constants()?;

        // finally, solve token_r_amount = a*c^2 + b*c + i0
        // i.e. 0 = a*c^2 + b*c + (i0-token_r_amount)
        let (i_abs_value, i_is_negative);
        if i0_is_negative {
            // both i0 and (-token_r_amount) are negative - can just add the two amounts and keep the sign negative
            i_abs_value = i0_abs_value.checked_add(token_r_amount)?;
            i_is_negative = true;
        } else {
            // otherwise, we have to do signed subtraction to solve (i0 - token_r_amount)
            let i_info = i0_abs_value.unsigned_sub(token_r_amount);
            i_abs_value = i_info.0;
            i_is_negative = i_info.1;
        }

        solve_quadratic_positive_root(
            &a_numerator,
            &a_denominator,
            &b_abs_value,
            b_is_negative,
            &i_abs_value,
            i_is_negative,
            should_round_sqrt_up,
        )
    }

    /// If `source_amount` will cause the swap to return all of its remaining `swap_destination_amount`,
    /// this returns the (maximum_token_a_amount, swap_destination_amount) that the swap can take
    /// Otherwise (if there's enough `swap_destination_amount` to handle all the `source_amount`), returns None
    fn maximum_a_remaining_for_swap_a_to_b(
        &self,
        c_start: &PreciseNumber,
        r_start: &PreciseNumber,
        source_amount: u128,
        swap_destination_amount: u128,
    ) -> Option<(u128, u128)> {
        // if at c_start + swap_destination_amount (the maximum CC that be given out by the swap), the R value is <= source_amount,
        // then only take that amount of R instead and give them all the CCs remaining
        let maximum_c_value =
            c_start.checked_add(&(PreciseNumber::new(swap_destination_amount)?))?;
        let maximum_r_locked = self.amt_r_locked_at_c_value_quadratic(&maximum_c_value)?;
        let maximum_r_remaining = maximum_r_locked.checked_sub(&r_start)?;

        // to_imprecise panics instead of returning None so need to handle explicitly here
        // TODO: this could be simplified with DFSPreciseNumber
        let maximum_r_remaining_u128 =
            match maximum_r_remaining.less_than_or_equal(&(PreciseNumber::new(u128::MAX)?)) {
                true => maximum_r_remaining.to_imprecise(),
                false => None,
            }?;

        if maximum_r_remaining_u128 <= source_amount {
            return Some((maximum_r_remaining_u128, swap_destination_amount));
        } else {
            return None;
        }
    }

    /// Swap's in user's collateral token and returns out the bonded token,
    /// moving right on the price curve and increasing the price of the bonded token
    fn swap_a_to_b_quadratic(
        &self,
        source_amount: u128,      // amount of user's token a (collateral token)
        swap_source_amount: u128, // swap's token a (collateral token)
        swap_destination_amount: u128, // swap's remaining token b (bonded token)
    ) -> Option<(u128, u128)> {
        // use swap_source_amount (collateral token) to determine where we are on the integration curve
        // note this only works if non-init deposits are disabled (and maybe if the initial deposit didn't have any token A in it?),
        // otherwise there could be some A token in the pool that isn't part of the bonding curve

        // quadratic formula version:
        let r_start = PreciseNumber::new(swap_source_amount)?;

        // TODO: two sqrt calls is pretty expensive (50K each), we could potentially optimize this by storing the initial deposit amount on chain and inferring c_start from that?
        // e.g c_start = initial_deposit_amount - swap_destination_amount (obviously only works if we disallow non-init deposits, and requires a lot of threading)
        let c_start = self.c_value_with_amt_r_locked_quadratic(&r_start, true)?;
        match self.maximum_a_remaining_for_swap_a_to_b(
            &c_start,
            &r_start,
            source_amount,
            swap_destination_amount,
        ) {
            Some(val) => return Some(val),
            // no need to return None here if checked_add fails, can just skip this check and do real calculation below
            None => (),
        }

        // otherwise, there's enough C tokens for all the R they put in, find the c_end value for the amount of R they're putting in and give them `c_end - c_start` tokens out
        let r_end = r_start.checked_add(&(PreciseNumber::new(source_amount)?))?;
        let c_end = self.c_value_with_amt_r_locked_quadratic(&r_end, false)?;

        let difference = c_end.checked_sub(&c_start)?;
        // PreciseNumber rounds .5+ up by default, make sure to floor instead so we don't allow
        // dust to round up for free
        let destination_amount = difference.floor()?.to_imprecise()?;

        Some((source_amount, destination_amount))
    }

    fn swap_b_to_a_quadratic(
        &self,
        source_amount: u128,
        _swap_source_amount: u128,
        swap_destination_amount: u128,
    ) -> Option<(u128, u128)> {
        // use swap_destination_amount (collateral token) to determine where we are on the integration curve
        // note this only works if non-init deposits are disabled (and maybe if the initial deposit didn't have any token A in it?),
        // otherwise there could be some A token in the pool that isn't part of the bonding curve

        let c_start = self.c_value_with_amt_r_locked_quadratic(
            &(PreciseNumber::new(swap_destination_amount)?),
            true,
        )?;

        // c_end can be negative if the user put in too many C tokens (handled below)
        let (c_end, c_end_is_negative) =
            c_start.unsigned_sub(&(PreciseNumber::new(source_amount)?));

        // make sure to use c_end.ceiling() when doing below calculations r_end so we don't round in favor of the user
        // if we use c_end directly, it's possible to gain tokens for free by swapping back and forth due to
        // rounding (see swap_large_price_almost_max_r test)
        // (especially since sqrt_babylonian under estimates, we often will end up with a c_end/r_end that's too low
        // due to rounding)
        let c_end = c_end.ceiling()?;

        // if c_end <= initial_c_value (i.e. there aren't enough R tokens in the swap for all the C tokens they put in), then just give
        // them all of the r tokens (swap_destination_amount) and only take the C tokens required to get down to initial_c_value
        // this only works if we assume 0 R locked at initial_c_value
        let initial_c_value = PreciseNumber::new(self.initial_token_c_price.into())?;
        if c_end_is_negative || c_end.less_than_or_equal(&initial_c_value) {
            let maximum_c_remaining = c_start.checked_sub(&initial_c_value)?.to_imprecise()?;
            return Some((maximum_c_remaining, swap_destination_amount));
        }

        // otherwise if there's enough R tokens locked in swap_destination_amount, figure out the R value at c_end and give them the difference (swap_destination_amount - r_end) tokens
        let r_end = self.amt_r_locked_at_c_value_quadratic(&c_end)?;

        // PreciseNumber rounds .5+ up by default, make sure to floor instead so we don't allow
        // dust to round up for free
        let destination_amount = PreciseNumber::new(swap_destination_amount)?
            .checked_sub(&r_end)?
            .floor()?
            .to_imprecise()?;

        Some((source_amount, destination_amount))
    }
}

/// These functions use the square + triangle area geometry formula to determine R liquidity between 2 given
/// C values (amt_r_locked_between_c_values_precise)
/// It then uses that to perform binary search to determine C value for a given amount of R
/// liquidity (c_value_with_amt_r_locked_bsearch_u128)
///
/// swap_a_to_b_bsearch and swap_b_to_a_bsearch are the key functions at the bottom
/// the area calculation for C -> R uses PreciseNumber but the binary search for R -> C uses u128 (otherwise
/// we run out of compute)
///
/// Haven't figured out a way to get both precise enough and low compute enough yet - this currently either
/// fails the anchor tests with not enough compute (if we use PreciseNumber for area calculation) or it
/// fails the rust tests (if we use u128 for area calculation)
impl LinearPriceCurve {
    /// Calculates the area (amount of token R locked) under the curve between c_start and c_end
    /// In cases where we overflow PreciseNumber, we just return u128::MAX (the total amount of R
    /// can't exceed this anyway, and it's a little more useful to return Some value instead of None
    /// for the binary search)
    fn amt_r_locked_between_c_values_precise(
        &self,
        c_start: &PreciseNumber,
        c_end: &PreciseNumber,
    ) -> Option<PreciseNumber> {
        // TODO: write some tests for this

        let slope_numerator = PreciseNumber::new(self.slope_numerator.into())?;
        let slope_denominator = PreciseNumber::new(self.slope_denominator.into())?;
        let m = slope_numerator.checked_div(&slope_denominator)?;
        let r0 = PreciseNumber::new(self.initial_token_r_price.into())?;
        let c0 = PreciseNumber::new(self.initial_token_c_price.into())?;

        let r_start = m
            .checked_mul(&(c_start.checked_sub(&c0)?))?
            .checked_add(&r0)?;
        let r_end = m
            .checked_mul(&(c_end.checked_sub(&c0)?))?
            .checked_add(&r0)?;

        let r_delta = r_end.checked_sub(&r_start)?;
        let c_delta = c_end.checked_sub(&c_start)?;

        let square_area = c_delta.checked_mul(&r_start)?;

        let triangle_area = c_delta
            .checked_div(&(PreciseNumber::new(2))?)?
            .checked_mul(&r_delta)?;

        square_area.checked_add(&triangle_area)
    }

    // TODO: this doesn't have enough precision, we're overflowing u128 too often (e.g. even on the r_end slope calculation)
    // /// Calculates the area (amount of token R locked) under the curve between c_start and c_end
    fn amt_r_locked_between_c_values_u128(&self, c_start: u128, c_end: u128) -> Option<u128> {
        // TODO: write some tests for this

        let r_start = (self.slope_numerator as u128)
            .checked_mul(c_start.checked_sub(self.initial_token_c_price.into())?)?
            .checked_div(self.slope_denominator.into())?
            .checked_add(self.initial_token_r_price.into())?;
        let r_end_num = (self.slope_numerator as u128)
            .checked_mul(c_end.checked_sub(self.initial_token_c_price.into())?)?;
        let r_end = r_end_num
            .checked_div(self.slope_denominator.into())?
            .checked_add(self.initial_token_r_price.into())?;

        let r_delta = r_end.checked_sub(r_start)?;
        let c_delta = c_end.checked_sub(c_start)?;

        let square_area = c_delta.checked_mul(r_start)?;
        let triangle_area = c_delta.checked_div(2)?.checked_mul(r_delta)?;

        square_area.checked_add(triangle_area)
    }

    fn c_value_with_amt_r_locked_bsearch_u128(
        &self,
        r_amt_target: u128,
        c_lower_bound: u128,
        c_upper_bound: u128,
    ) -> Option<u128> {
        let c0 = PreciseNumber::new(self.initial_token_c_price.into())?;

        let mut min = c_lower_bound;
        let mut max = c_upper_bound;

        while min <= max {
            let cur_c = min.checked_add(max)?.checked_div(2)?;

            // TODO: not sure if we can make this work to be both precise and compute-cheap enough
            // this runs out of compute for anchor-token-swap.ts
            // let cur_r_locked = self.amt_r_locked_between_c_values_precise(
            //     &(PreciseNumber::new(self.initial_token_c_price.into())?),
            //     &(PreciseNumber::new(cur_c)?),
            // )?;

            // this fails our rust tests, not precise enough
            let cur_r_locked =
                self.amt_r_locked_between_c_values_u128(self.initial_token_c_price.into(), cur_c)?;

            if r_amt_target == cur_r_locked {
                return Some(cur_c);
            } else if r_amt_target > cur_r_locked {
                min = cur_c.checked_add(1)?;
            } else if r_amt_target < cur_r_locked {
                max = cur_c.checked_sub(1)?;
            }
        }

        // TODO: just placeholder, handle if target is outside of lower/upper bounds
        Some(min)
    }

    fn swap_a_to_b_bsearch(
        &self,
        source_amount: u128,
        swap_source_amount: u128,
        swap_destination_amount: u128,
    ) -> Option<(u128, u128)> {
        // use swap_source_amount (collateral token) to determine where we are on the integration curve
        // note this only works if non-init deposits are disabled (and maybe if the initial deposit didn't have any token A in it?),
        // otherwise there could be some A token in the pool that isn't part of the bonding curve

        // swap_source_amt = (c_start - c0) * r0 + triangle area
        // so swap_source_amt >= (c_start - c0) * r0
        // so c_start <= swap_source_amt/r0 + c0
        let c_start_upper_bound =
            match swap_source_amount.checked_div(self.initial_token_r_price.into()) {
                Some(val) => val.checked_add(self.initial_token_c_price.into())?,
                // TODO: is this okay? probably will run out of compute - what's a better fallback?
                None => u128::MAX.checked_sub(self.initial_token_c_price.into())?,
            };

        let c_start = self.c_value_with_amt_r_locked_bsearch_u128(
            swap_source_amount,
            self.initial_token_c_price.into(),
            c_start_upper_bound,
        )?;
        let c_end = self.c_value_with_amt_r_locked_bsearch_u128(
            swap_source_amount.checked_add(source_amount)?,
            c_start,
            c_start.checked_add(swap_destination_amount)?, // swap_destination_amount + c_start is the most the pool can give out
        )?;

        let destination_amount = c_end.checked_sub(c_start)?;

        // TODO: need to handle rounding up/down, especially if not all the source_amount will be used (i.e. there's not enough swap_destination_amount)

        Some((source_amount, destination_amount))
    }

    fn swap_b_to_a_bsearch(
        &self,
        source_amount: u128,
        _swap_source_amount: u128,
        swap_destination_amount: u128,
    ) -> Option<(u128, u128)> {
        // use swap_destination_amount (collateral token) to determine where we are on the integration curve
        // note this only works if non-init deposits are disabled (and maybe if the initial deposit didn't have any token A in it?),
        // otherwise there could be some A token in the pool that isn't part of the bonding curve

        let c_start_upper_bound =
            match swap_destination_amount.checked_div(self.initial_token_r_price.into()) {
                Some(val) => val.checked_add(self.initial_token_c_price.into())?,
                // TODO: is this okay? probably will run out of compute - what's a better fallback?
                None => u128::MAX.checked_sub(self.initial_token_c_price.into())?,
            };

        let c_start = self.c_value_with_amt_r_locked_bsearch_u128(
            swap_destination_amount,
            self.initial_token_c_price.into(),
            c_start_upper_bound,
        )?;
        let c_end = c_start.checked_sub(source_amount)?;

        // we should have enough compute to at least use PreciseNumber geometry here
        let destination_amount = self
            .amt_r_locked_between_c_values_precise(
                &(PreciseNumber::new(c_end)?),
                &(PreciseNumber::new(c_start)?),
            )?
            .to_imprecise()?;

        // TODO: need to handle rounding up/down, especially if not all the source_amount will be used (i.e. there's not enough swap_destination_amount)

        Some((source_amount, destination_amount))
    }
}

/// Returns None iff slope is 0 or close enough to 0 with PreciseNumber
fn is_slope_valid(curve: &LinearPriceCurve) -> Option<()> {
    if curve.slope_numerator == 0 || curve.slope_denominator == 0 {
        return None;
    };

    // since PreciseNumber only has 12 decimals, any slope < 1e-12 will be treated as 0
    let numerator = PreciseNumber::new(curve.slope_numerator.into())?;
    let denominator = PreciseNumber::new(curve.slope_denominator.into())?;
    let minimum = PreciseNumber::new(1)?.checked_div(&(PreciseNumber::new(1_000_000_000_000)?))?;

    match numerator
        .checked_div(&denominator)?
        .greater_than_or_equal(&minimum)
    {
        true => Some(()),
        false => None,
    }
}

/// This can be removed once we settle on either the quadratic or bsearch method, just using to triage
/// between the two methods for now
impl LinearPriceCurve {
    fn swap_a_to_b(
        &self,
        source_amount: u128,
        swap_source_amount: u128,
        swap_destination_amount: u128,
    ) -> Option<(u128, u128)> {
        self.swap_a_to_b_quadratic(source_amount, swap_source_amount, swap_destination_amount)
    }

    fn swap_b_to_a(
        &self,
        source_amount: u128,
        swap_source_amount: u128,
        swap_destination_amount: u128,
    ) -> Option<(u128, u128)> {
        self.swap_b_to_a_quadratic(source_amount, swap_source_amount, swap_destination_amount)
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
        // this causes a panic if withdraw_all_token_types is called but that's ok for now, cheap way of
        // disabling withdrawals without having to change how SwapCurve works
        None

        // could we do something like this if we just want pool tokens to be 1-1 with B tokens and not withdrawable/depositable for A tokens?
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
        source_amount: u128,
        swap_token_r_amount: u128,
        swap_token_c_amount: u128,
        pool_supply: u128,
        trade_direction: TradeDirection,
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
        source_amount: u128,
        swap_token_r_amount: u128,
        swap_token_c_amount: u128,
        pool_supply: u128,
        trade_direction: TradeDirection,
    ) -> Option<u128> {
        // this causes a panic if SwapCurve.withdraw_single_token_type_exact_out instruction is called
        // but that's ok for now, cheap way of disabling withdrawals without having to change how SwapCurve works
        // (also if a non-zero fee curve is created this would also cause a panic, though that's disabled at the lib.rs level)
        None
    }

    /// Validate that the given curve has no invalid parameters
    /// Called on `initialize` - slope must be positive but initial point can be (0,0)
    fn validate(&self) -> Result<(), SwapError> {
        match is_slope_valid(&self) {
            Some(_val) => Ok(()),
            None => Err(SwapError::InvalidCurve),
        }
    }

    /// Validate the given supply on initialization.
    /// We require at least some bonded token B for the curve to be useful (collateral token can be 0)
    /// TODO: if we enable deposits, then this check isn't needed, the pool can start with 0 of both
    fn validate_supply(&self, token_r_amount: u64, token_c_amount: u64) -> Result<(), SwapError> {
        if token_c_amount == 0 {
            return Err(SwapError::EmptySupply);
        }

        // i think there's probably a way to allow initial collateral token if we adjust the
        // initial token values to take that into account, but seems easier to disallow it. it's the same
        // as starting with 0 collateral token and then doing a swap anyway
        if token_r_amount != 0 {
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
    use crate::curve::calculator::{
        test::{
            check_curve_value_from_swap, check_deposit_token_conversion,
            check_withdraw_token_conversion, total_and_intermediate,
            CONVERSION_BASIS_POINTS_GUARANTEE,
        },
        INITIAL_SWAP_POOL_AMOUNT,
    };
    use proptest::prelude::*;

    #[test]
    fn swap_a_to_b_basic() {
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
            curve.swap_a_to_b(101_0000_0000, 0, 5000_0000_0000).unwrap();
        assert_eq!(source_amount, 101_0000_0000);
        assert_eq!(destination_amount, 2_0000_0000);

        // putting in 5900K RLY @ 81600 RLY locked/20CC remaining should give out the last 20 CC
        let (source_amount, destination_amount) = curve
            .swap_a_to_b(5900_0000_0000, 81600_0000_0000, 20_0000_0000)
            .unwrap();
        assert_eq!(source_amount, 5900_0000_0000);
        assert_eq!(destination_amount, 20_0000_0000);

        // putting in 10K RLY @ 81600 RLY locked/20CC remaining should give out the last 20 CC and only take 5.9K RLY
        let (source_amount, destination_amount) = curve
            .swap_a_to_b(10000_0000_0000, 81600_0000_0000, 20_0000_0000)
            .unwrap();
        assert_eq!(source_amount, 5900_0000_0000);
        assert_eq!(destination_amount, 20_0000_0000);

        // similar to 145K segment of forte curve, but assume r has 18 decimals (this just lets us cram more precision into
        // the calculation, as long as we interpret it correctly back out at the end)
        // since r has 12 more decimals of precision than c, scale both slope and initial_token_r_price by 1e12
        let curve = LinearPriceCurve {
            slope_numerator: 5689_549_999_968_874, // 5.689549999968874e-9 in forte, so should be 5.689549999968874e3 now
            slope_denominator: 1_000_000_000_000,
            initial_token_r_price: 35_915742_315103, // 35.9157423151027 in forte, so should be 3.59...e13 now
            initial_token_c_price: 145000_000000,
        };

        // putting in 7296... RLY in, should move price to 145_199_999999.99
        // (i.e. get 199_999999 CC out)
        let (source_amount, destination_amount) = curve
            .swap_a_to_b(7296_939463_019977_479999, 0, 5000_000000)
            .unwrap();
        assert_eq!(source_amount, 7296_939463_019977_479999);
        assert_eq!(destination_amount, 199_999999); // should floor instead of rounding up to 200

        // putting in a little more than above should move price to 145_200_000000.0000
        // (note we lose a little precision from u128 sqrt, so this crosses 145_200_000000 a little
        // past the exact value of 7296_939463_019977_480000)
        let (source_amount, destination_amount) = curve
            .swap_a_to_b(7296_939463_030000_000000, 0, 5000_000000)
            .unwrap();
        assert_eq!(source_amount, 7296_939463_030000_000000);
        assert_eq!(destination_amount, 200_000000); // crossed past 200 so should give out 200 now

        // put in 7524... more RLY, should get another 199_999999 CC out
        let (source_amount, destination_amount) = curve
            .swap_a_to_b(
                7524_521463_008709_920000,
                7296_939463_030000_000000,
                4800_000000,
            )
            .unwrap();
        assert_eq!(source_amount, 7524_521463_008709_920000);
        assert_eq!(destination_amount, 199_999999); // should floor instead of rounding up to 200

        // putting in a little more than above should move price to 145_400_000000.0000
        let (source_amount, destination_amount) = curve
            .swap_a_to_b(
                7524_521463_020000_000000,
                7296_939463_030000_000000,
                4800_000000,
            )
            .unwrap();
        assert_eq!(source_amount, 7524_521463_020000_000000);
        assert_eq!(destination_amount, 199_999999); // should floor instead of rounding up to 200

    }

    #[test]
    fn swap_b_to_a_basic() {
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 2,
            initial_token_r_price: 50,
            initial_token_c_price: 300,
        };

        // pretty much the opposite cases as above

        // put in 2 CC at 101 RLY, should get 101 RLY out
        let (source_amount, destination_amount) = curve.swap_b_to_a(2, 4998, 101).unwrap();
        assert_eq!(source_amount, 2);
        assert_eq!(destination_amount, 101);

        // put in 2 CC at 204 RLY, should get 103 RLY out
        let (source_amount, destination_amount) = curve.swap_b_to_a(2, 4996, 204).unwrap();
        assert_eq!(source_amount, 2);
        assert_eq!(destination_amount, 103);

        // same as above but assuming they both have 8 decimals
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 2_0000_0000, // slope needs to be scaled down to take into account C having 8 decimals
            initial_token_r_price: 50, // since they both have 8 decimals, no need to scale this (it's still 50 base RLY for 1 base CC)
            initial_token_c_price: 300_0000_0000,
        };

        let (source_amount, destination_amount) = curve
            .swap_b_to_a(2_0000_0000, 4998_0000_0000, 101_0000_0000)
            .unwrap();
        assert_eq!(source_amount, 2_0000_0000);
        assert_eq!(destination_amount, 101_0000_0000);

        // similar to 145K segment of forte curve, but assume r has 18 decimals (this just lets us cram more precision into
        // the calculation, as long as we interpret it correctly back out at the end)
        // since r has 12 more decimals of precision than c, scale both slope and initial_token_r_price by 1e12
        let curve = LinearPriceCurve {
            slope_numerator: 5689_549_999_968_874, // 5.689549999968874e-9 in forte, so should be 5.689549999968874e3 now
            slope_denominator: 1_000_000_000_000,
            initial_token_r_price: 35_915742_315103, // 35.9157423151027 in forte, so should be 3.59...e13 now
            initial_token_c_price: 145000_000000,
        };

        // putting in 200 CC at 7296.9394630144 RLY, should get it all out
        let (source_amount, destination_amount) = curve
            .swap_b_to_a(200_000000, 4800_000000, 7296_939463_019977_480000)
            .unwrap();
        assert_eq!(source_amount, 200_000000);
        // note this rounds down due to sqrt rounding, we could get closer to real value of
        // 7296_939463019977480000 if we pad some precision to sqrt
        assert_eq!(destination_amount, 7296_939427104235162052);

        // put in 200 CC at 14821.4609260237 RLY, should get 7524.5214630093 RLY out
        let (source_amount, destination_amount) = curve
            .swap_b_to_a(200_000000, 4600_000000, 14821_460926_038709_920000)
            .unwrap();
        assert_eq!(source_amount, 200_000000);
        // note this rounds down due to sqrt rounding, we could get closer to real value of
        // 7524_521463018732440000 if we pad some precision to sqrt
        assert_eq!(destination_amount, 7524_521425965080122058);

        // put in 300 CC at 7296.9394630144 RLY, should get it all out (and only take 200 CC)
        let (source_amount, destination_amount) = curve
            .swap_b_to_a(300_000000, 4800_000000, 7296_939463_019977_480000)
            .unwrap();
        assert_eq!(source_amount, 200_000000);
        assert_eq!(destination_amount, 7296_939463_019977_480000);

        // a curve that starts at 0/0
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 2,
            initial_token_r_price: 0,
            initial_token_c_price: 0,
        };

        // put in 6 CC at 9 RLY, should get all 9 RLY out
        let (source_amount, destination_amount) = curve.swap_b_to_a(6, 494, 9).unwrap();
        assert_eq!(source_amount, 6);
        assert_eq!(destination_amount, 9);

        // put in 11 CC at 9 RLY, should get all 9 RLY out and only take 6 CC
        let (source_amount, destination_amount) = curve.swap_b_to_a(11, 494, 9).unwrap();
        assert_eq!(source_amount, 6);
        assert_eq!(destination_amount, 9);
    }

    #[test]
    fn swap_without_fees() {
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 2,
            initial_token_r_price: 50,
            initial_token_c_price: 300,
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
            initial_token_r_price: 0,
            initial_token_c_price: 1,
        };

        let mut packed = [0u8; LinearPriceCurve::LEN];
        Pack::pack_into_slice(&curve, &mut packed[..]);
        let unpacked = LinearPriceCurve::unpack(&packed).unwrap();
        assert_eq!(curve, unpacked);

        let mut packed = vec![];
        packed.extend_from_slice(&curve.slope_numerator.to_le_bytes());
        packed.extend_from_slice(&curve.slope_denominator.to_le_bytes());
        packed.extend_from_slice(&curve.initial_token_r_price.to_le_bytes());
        packed.extend_from_slice(&curve.initial_token_c_price.to_le_bytes());
        let unpacked = LinearPriceCurve::unpack(&packed).unwrap();
        assert_eq!(curve, unpacked);
    }

    /// These swap_large_price_foo tests all test the overflow boundaries of u64/u128 test - mostly just to give
    /// some example curves with large numbers (and make sure they return None instead of something crazy)
    #[test]
    fn swap_large_price_max_r() {
        // curve with everything near u64::MAX
        let curve = LinearPriceCurve {
            slope_numerator: u64::MAX,
            slope_denominator: u64::MAX - 1,
            initial_token_r_price: u64::MAX,
            initial_token_c_price: u64::MAX,
        };

        // with initial_token_r_price == u64::MAX, swap_a_to_b_quadratic always either overflows on the sqrt calculation
        // or there aren't enough R tokens to get any C tokens out
        // note this is independent of initial_token_c_price (see below) since it's only dependent on R locked
        // (this is true for both quadratic and bsearch methods)
        // 18446744073709551615 <- C value at R = 0
        // 18446744073709551616 <- C value at R = 2^64 <- minimum amount of R to get any C tokens out, but already overflows
        for exp in 0..128 {
            let result = curve.swap_without_fees(
                2_u128.pow(exp),
                0,
                1_00000_00000_00000_00000,
                TradeDirection::AtoB,
            );
            assert!(result.is_none());
        }

        // b -> a (kind of pointless since we can't get here from a -> b but just checking for completeness)
        // 18446744073709551616 <- C value at R = 2^64 <- minimum amount of R to get any C tokens out, but already overflows
        // 18446744073709551615 <- C value at R = 0
        // (diff is 1)
        // put in 1 C tokens at R = 2^64, should get 2^64 R out
        // note just like the above, the sqrt calculation overflows even with just 1 CC
        let result = curve.swap_without_fees(
            1,
            0, // this doesn't matter (it's the amount of token b left but we're going the other direction)
            2u128.pow(64),
            TradeDirection::BtoA,
        );
        assert!(result.is_none());

        // same as above but with a lower token C (doesn't matter though still overflows no matter what C price is)

        let curve = LinearPriceCurve {
            slope_numerator: u64::MAX,
            slope_denominator: u64::MAX - 1,
            initial_token_r_price: u64::MAX,
            initial_token_c_price: u32::MAX.into(),
        };

        // a -> b
        for exp in 0..128 {
            let result = curve.swap_without_fees(
                2_u128.pow(exp),
                0,
                1_00000_00000_00000_00000,
                TradeDirection::AtoB,
            );
            assert!(result.is_none());
        }

        // b -> a
        let result = curve.swap_without_fees(
            1,
            0, // this doesn't matter (it's the amount of token b left but we're going the other direction)
            2u128.pow(64),
            TradeDirection::BtoA,
        );
        assert!(result.is_none());
    }

    /// These swap_large_price_foo tests all test the overflow boundaries of u64/u128 test - mostly just to give
    /// some example curves with large numbers (and make sure they return None instead of something crazy)
    #[test]
    fn swap_large_price_r_u32() {
        // example curve with R price relatively low and everything else high
        let curve = LinearPriceCurve {
            slope_numerator: u64::MAX,
            slope_denominator: u64::MAX - 1,
            initial_token_r_price: u32::MAX.into(),
            initial_token_c_price: u64::MAX,
        };

        // testing a -> b

        // put in 2^64 - 1 R tokens, should move C value from
        // 18446744073709551615.00 <- C value at R = 0
        // 18446744076853685892.94 <- C value at R = 2^64 - 1
        // (diff is 31441_34276.94, should floor down)
        // this fails with bsearch (amt_r_locked_between_c_values_u128 calculation overflows) but works with quadratic
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
                // note this rounds down even more due to sqrt rounding, we could get closer to real value of
                // 31441_34276 if we pad some precision to sqrt
                destination_amount_swapped: 31441_34275
            }
        );

        // this R value causes overflow for this curve (it's relatively pretty close to u128::MAX, about 2^127)
        // 18446744073709551615.00 <- C value at R = 0
        // 36893488143124135935.00 <- C value at R = 170141183460469231713240559646469521406
        // (note due to PreciseNumber only having 12 decimals of precision, the runtime C_end value is
        // 34.9999 instead of 35.0000 - the runtime is basically using a slope of 1)
        // (runtime diff is 18446_74406_94145_84319.99)
        // this fails with bsearch (overflow) but works with quadratic
        let result = curve.swap_without_fees(
            170141183460469231713240559646469521406,
            0,
            1_00000_00000_00000_00000,
            TradeDirection::AtoB,
        );
        assert_eq!(
            result.unwrap(),
            SwapWithoutFeesResult {
                source_amount_swapped: 170141183460469231713240559646469521406,
                // note this rounds down due to sqrt rounding, we could get closer to real value of
                // 18446_74406_94145_84319.99 (floored to 19) if we pad some precision to sqrt
                destination_amount_swapped: 18446_74406_94145_84318
            }
        );

        // 1 more R and we overflow for this curve (the checked_add in sqrt_babylonian has an overflow and returns None)
        let result = curve.swap_without_fees(
            170141183460469231713240559646469521406 + 1,
            0,
            1_00000_00000_00000_00000,
            TradeDirection::AtoB,
        );
        assert!(result.is_none());

        // testing b -> a on the same curve
        // 170141183460469231713240559646469521406 <- R value at C = 36893488143124135935.00
        // 42535295884924348538429480234146332673.37 <- R value at C = 27670116108416843774 (halfway to initial C)
        // (this works for quadratic but overflows on bsearch)
        let result = curve
            .swap_without_fees(
                9223372034707292161, // amount C in = diff between C values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                170141183460469231713240559646469521406,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 9223372034707292161);
        assert_eq!(
            result.destination_amount_swapped,
            // note because PreciseNumber only has 12 decimals (and our slope doesn't round evenly
            // to 12 decimals) this is slightly different than the exact amount of
            // 127605887575544883174811079412323188733
            127605887575544883165587707373320929277 // amount R out = diff between R values
        );

        // now (with actual R numbers above), swap balance is 42535295884924348538429480234146332674
        // 42535295884924348538429480234146332674 <- R value at C = 27670116108416843773.25
        //  (using the rounded R value from above to make sure the rounding doesn't cause any compounding issues)
        // 10633823981134607446584569249589624832.09 <- R value at C = 23058430091063197695
        //  (another halfway down to initial)
        let result = curve
            .swap_without_fees(
                4611686017353646078, // amount C in = diff between C values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                42535295884924348538429480234146332674,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 4611686017353646078);
        assert_eq!(
            result.destination_amount_swapped,
            // same note as above - slightly off from exact amount of
            // 31901471903789741091844910984556707842
            31901471903789741082621538941259481089 // amount R out = diff between R values
        );

        // now (with actual R numbers above), swap balance is 10633823981134607451196255271238238208
        // 10633823981134607451196255271238238208 <- R value at C = 23058430091063197694.125
        // 0 <- R value at C = c initial
        let result = curve
            .swap_without_fees(
                // note due to sqrt rounding this requires a bit more than the actual amount
                // we could get closer to real value of 23058430091063197694 if we pad some precision to sqrt
                23058430091063197697 - (u64::MAX as u128), // amount C in = diff between C values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                10633823981134607451196255271238238208,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(
            result.source_amount_swapped,
            23058430091063197697 - (u64::MAX as u128)
        );
        assert_eq!(
            result.destination_amount_swapped,
            10633823981134607451196255271238238208 // amount R out = diff between R values
        );

        // note we got out 18446744069414584319 b tokens at the end of a->b and
        // we put in 18446744069414584320 b tokens at the end of b->a (to get all the a back
        // out) - it's off by 1 since we rounded such that there's no arbitrage opportunity

        // same as above but with a huge token b, make sure we only take the required amount
        let result = curve
            .swap_without_fees(
                u128::MAX, // way more token b than needed to get all the token a out
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                10633823981134607451196255271238238208,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(
            result.source_amount_swapped,
            23058430091063197697 - (u64::MAX as u128) // should still only take this much C
        );
        assert_eq!(
            result.destination_amount_swapped,
            10633823981134607451196255271238238208
        );
    }

    /// These swap_large_price_foo tests all test the overflow boundaries of u64/u128 test - mostly just to give
    /// some example curves with large numbers (and make sure they return None instead of something crazy)
    #[test]
    fn swap_large_price_almost_max_r() {
        // example curve with R price a little lower than u64 MAX and everything else high
        let curve = LinearPriceCurve {
            slope_numerator: u64::MAX,
            slope_denominator: u64::MAX - 1,
            initial_token_r_price: u64::MAX - 100,
            initial_token_c_price: u64::MAX,
        };

        // 2^70 R is about where this curve overflows
        // 18446744073709551615.00 <- C value at R = 0
        // 18446744073709551679.00 <- C value at R = 2^70
        // diff is 64
        // this fails with bsearch (overflow) but works with quadratic
        let result = curve.swap_without_fees(
            2u128.pow(70),
            0,
            1_00000_00000_00000_00000,
            TradeDirection::AtoB,
        );
        assert_eq!(
            result.unwrap(),
            SwapWithoutFeesResult {
                source_amount_swapped: 2u128.pow(70),
                destination_amount_swapped: 64
            }
        );

        // 2^71 overflows
        let result = curve.swap_without_fees(
            2u128.pow(71),
            0,
            1_00000_00000_00000_00000,
            TradeDirection::AtoB,
        );
        assert!(result.is_none());

        // testing b -> a on the same curve

        // put all 64 CC back in, should get all 2^70 out
        let result = curve
            .swap_without_fees(
                64, // amount C in = diff between C values
                0,  // this doesn't matter (amt of token b left but we're going the other direction)
                1180591620717411303424,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 64);
        assert_eq!(
            result.destination_amount_swapped,
            // note this rounds down due to sqrt rounding, we could get closer to desired behavior
            // of returning all the token b if we pad some precision to sqrt
            1162144876643701751908 // amount R out = diff between R values
        );

        // putting in one extra token b does return all the token a though
        let result = curve
            .swap_without_fees(
                65, // amount C in = diff between C values
                0,  // this doesn't matter (amt of token b left but we're going the other direction)
                1180591620717411303424,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 65);
        assert_eq!(
            result.destination_amount_swapped,
            1180591620717411303424 // amount R out = diff between R values
        );

        // since this curve has a very drastic a:b ratio, the rounding errors can compound
        // from swap to swap so make sure we don't round in favor of the user
        // e.g. if we used c_end directly instead of c_end.ceiling() in swap_b_to_a, we'd
        // get all the RLY out with only 63 CC instead of 64 with the below 32 -> 16 -> 16 txns

        // 1180591620717411303424.00 <- R value at C = 18446744073709551679
        // 590295810358705648992.00 <- R value at C = 18446744073709551647 (halfway to initial C)
        let result = curve
            .swap_without_fees(
                32, // amount C in = diff between C values
                0,  // this doesn't matter (amt of token b left but we're going the other direction)
                1180591620717411303424,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 32);
        assert_eq!(
            result.destination_amount_swapped,
            // note this rounds down due to sqrt rounding, we could get closer to real value of
            // 590295810358705654432 if we pad some precision to sqrt
            571849066284996102884 // amount R out = diff between R values
        );

        // 590295810358705648992 <- R value at C = 18446744073709551647.00...
        // 295147905179352824368.00 <- R value at C = 18446744073709551631
        let result = curve
            .swap_without_fees(
                16, // amount C in = diff between C values
                0,  // this doesn't matter (amt of token b left but we're going the other direction)
                590295810358705648992,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 16);
        assert_eq!(
            result.destination_amount_swapped,
            // note this rounds down due to sqrt rounding, we could get closer to real value of
            // 295147905179352824624 if we pad some precision to sqrt
            276701161105643273092 // amount R out = diff between R values
        );

        // 295147905179352824368 <- R value at C = 18446744073709551631
        // 0 <- R value at C = c initial 18446744073709551615 (u64 MAX)
        let result = curve
            .swap_without_fees(
                // note due to sqrt rounding this requires 1 more token b than the actual amount
                // we could get closer to real value of 16 if we pad some precision to sqrt
                17, // amount C in = diff between C values
                0,  // this doesn't matter (amt of token b left but we're going the other direction)
                295147905179352824368,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 17);
        assert_eq!(
            result.destination_amount_swapped,
            295147905179352824368 // amount R out = diff between R values
        );

        // same as above but with a huge token b, make sure we only take the required amount
        let result = curve
            .swap_without_fees(
                u128::MAX, // way more token b than needed to get all the token a out
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                295147905179352824368,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(
            result.source_amount_swapped,
            17 // should still only take this much C
        );
        assert_eq!(result.destination_amount_swapped, 295147905179352824368);
    }

    /// These swap_large_price_foo tests all test the overflow boundaries of u64/u128 test - mostly just to give
    /// some example curves with large numbers (and make sure they return None instead of something crazy)
    #[test]
    fn swap_large_price_low_slope_u128() {
        // example curve with lowest possible slope and 0 starting R price (costs very little R to get a lot of C out)
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            // since PreciseNumber only has 12 decimals of precision, anything that
            // doesn't divide evenly or is < 1e-12 will be treated as 0 slope
            // (see swap_too_low_slope_workaround_example below for smaller slopes/more precision)
            slope_denominator: 1_000_000_000_000,
            initial_token_r_price: 0,
            initial_token_c_price: u64::MAX.into(),
        };

        // 18446744073709551615.00 <- C value at R = 0
        // 18446744073710965828.56 <- C value at R = 1
        // diff is 1414213.56
        let result = curve.swap_without_fees(1, 0, u128::MAX - 1, TradeDirection::AtoB);
        assert_eq!(
            result.unwrap(),
            SwapWithoutFeesResult {
                source_amount_swapped: 1,
                destination_amount_swapped: 1414213
            }
        );

        // 18446744073709551615.00 <- C value at R = 0
        // 26087654097409638134250758.61 <- C value at R = 2^128-1
        let result = curve.swap_without_fees(u128::MAX, 0, u128::MAX - 1, TradeDirection::AtoB);
        assert_eq!(
            result.unwrap(),
            SwapWithoutFeesResult {
                source_amount_swapped: u128::MAX,
                // due to u128 sqrt precision, this is slightly off from exact value of
                // 26087635650665564424699143
                destination_amount_swapped: 26087635650665000000000000
            }
        );

        // testing b -> a on the same curve

        // put all 26087635650665000000000000 CC back in, should get all u128 max out
        let result = curve
            .swap_without_fees(
                26087635650665000000000000, // amount C in = diff between C values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                u128::MAX,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 26087635650665000000000000);
        assert_eq!(
            result.destination_amount_swapped,
            // due to u128 sqrt precision, this is slightly rounded down from exact value of
            // 340282366920938463463374607431768211455 (u128 max max)
            340282366920938463463374606931768211455 // amount R out = diff between R values
        );

        // 128::MAX <- R value at C = 26087654097409638134250758.61
        // (this C is just for swap's bookkeeping so no need to match the value above,
        // as long as the total C swapped at the end makes sense it's okay)
        // 85070591730230934739367778125000000000 <- R value at C = 13043836272076573709551615 (halfway to initial C)
        let result = curve
            .swap_without_fees(
                13043817825333064424699143, // amount C in = diff between C values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                u128::MAX,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 13043817825333064424699143);
        assert_eq!(
            result.destination_amount_swapped,
            // due to u128 sqrt precision, this is slightly off from exact value of
            // 255211775190707528724006829306768211455
            255211775190701847159133236108741730144 // amount R out = diff between R values
        );

        // now (with actual R numbers above), swap balance is 85070591730236616304241371323026481311
        // 85070591730236616304241371323026481311 <- R value at C = 13043836272077009284852472.00
        //  (using the rounded R value from above to make sure the rounding doesn't cause any compounding issues)
        // 21267647932555893121604012982817251446.44 <- R value at C = 6521927359410041497202044
        //  (another halfway down to initial)
        let result = curve
            .swap_without_fees(
                6521908912666967787650428, // amount C in = diff between C values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                85070591730236616304241371323026481311,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 6521908912666967787650428);
        assert_eq!(
            result.destination_amount_swapped,
            // same note as above - slightly off from exact amount of
            // 63802943797680723182637358340209229865
            63802943797680303010617821782897184626 // amount R out = diff between R values
        );

        // now (with actual R numbers above), swap balance is 21267647932556313293623549540129296685
        // 21267647932556313293623549540129296685 <- R value at C = 6521927359410105921901187.00
        // 0 <- R value at C = c initial (18446744073709551615)
        // (due to u128 sqrt rounding though, we need to use a bit more C to exactly
        // swap out all the R, i.e. c initial + 6521908912667000000000000 instead of
        // 6521908912666032212349572)
        let result = curve
            .swap_without_fees(
                6521908912667000000000000, // amount C in = diff between C values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                21267647932556313293623549540129296685,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 6521908912667000000000000);
        assert_eq!(
            result.destination_amount_swapped,
            21267647932556313293623549540129296685 // amount R out = diff between R values
        );

        // note we got out 26087635650665000000000000 b tokens at the end of a->b and
        // we put in 26087635650667032212349571 b tokens at the end of b->a (to get all the a back
        // out) - this is due to rounding down u128 sqrt issues (safely, not in the user's favor)

        // same as above but with a huge token b, make sure we only take the required amount
        let result = curve
            .swap_without_fees(
                u128::MAX, // way more token b than needed to get all the token a out
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                21267647932556313293623549540129296685,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(
            result.source_amount_swapped,
            6521908912667000000000000 // should still only take this much C
        );
        assert_eq!(
            result.destination_amount_swapped,
            21267647932556313293623549540129296685
        );
    }

    /// These swap_large_price_foo tests all test the overflow boundaries of u64/u128 test - mostly just to give
    /// some example curves with large numbers (and make sure they return None instead of something crazy)
    /// This is similar to swap_large_price_low_slope_u128 but with a more realistic curve
    #[test]
    fn swap_large_price_low_slope_u64() {
        // same curve as above but we only use u64 values (realistically that's the maximum unless SPL
        // max supply changes)
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 1_000_000_000_000,
            initial_token_r_price: 0,
            initial_token_c_price: 0,
        };

        // 0 <- C value at R = 0
        // 6074000999952099.38 <- C value at R = 2^64-1
        let result =
            curve.swap_without_fees(u64::MAX.into(), 0, u128::MAX - 1, TradeDirection::AtoB);
        assert_eq!(
            result.unwrap(),
            SwapWithoutFeesResult {
                source_amount_swapped: u64::MAX.into(),
                // due to u128 sqrt precision, this is slightly off from exact value of
                // 6074000999952099
                destination_amount_swapped: 6074000000000000
            }
        );

        // testing b -> a on the same curve

        // put all 6074000000000000 CC back in, should get all u64 max out
        let result = curve
            .swap_without_fees(
                6074000000000000, // amount C in = diff between C values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                u64::MAX.into(),
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 6074000000000000);
        assert_eq!(
            result.destination_amount_swapped,
            // due to u128 sqrt precision, this is slightly rounded down from exact value of
            // 18446744073709551615 (u64 max)
            18446743573709551615 // amount R out = diff between R values
        );

        // swap from initial R locked of u64 max all the way down to 0 - make sure
        // any rounding is not in user's favor to prevent arbitrage

        // u64 max <- R value at C = 6074000999952099
        // (this C is just for swap's bookkeeping so no need to match the 6074000000000000 above,
        // as long as the total C swapped at the end makes sense it's okay)
        // 4611684500000000000 <- R value at C = 3037000000000000 (~halfway to initial C)
        let result = curve
            .swap_without_fees(
                3037000999952099, // amount C in = diff between C values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                u64::MAX.into(),
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 3037000999952099);
        assert_eq!(
            result.destination_amount_swapped,
            // due to u128 sqrt precision, this is slightly off from exact value of
            // 13835059573709551615
            13832025111563528424 // amount R out = diff between R values
        );

        // now (with actual R numbers above), swap balance is 4614718962146023191
        // 4614718962146023191 <- R value at C = 3037999000047901.00
        //  (using the rounded R value from above to make sure the rounding doesn't cause any compounding issues)
        // 1152921125000000000 <- R value at C = 1518500000000000
        //  (another halfway down to initial)
        let result = curve
            .swap_without_fees(
                1519499000047901, // amount C in = diff between C values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                4614718962146023191,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 1519499000047901);
        assert_eq!(
            result.destination_amount_swapped,
            // same note as above - slightly off from exact amount of
            // 3461797837146023191
            3461796318718260907 // amount R out = diff between R values
        );

        // now (with actual R numbers above), swap balance is 1152922643427762284
        // 1152922643427762284 <- R value at C = 1518500999952099
        // (due to u128 sqrt rounding though, we need to use C = 1519000000000000 to exactly
        // swap out all the R)
        // 0 <- R value at C = 0 (c initial)
        let result = curve
            .swap_without_fees(
                1519000000000000, // amount C in = diff between C values
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                1152922643427762284,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(result.source_amount_swapped, 1519000000000000);
        assert_eq!(
            result.destination_amount_swapped,
            1152922643427762284 // amount R out = diff between R values
        );

        // note we got out 6074000000000000 b tokens at the end of a->b and
        // we put in 6075500000000000 b tokens at the end of b->a (to get all the a back
        // out) - this is due to rounding down u128 sqrt issues (safely, not in the user's favor)

        // same as above but with a huge token b, make sure we only take the required amount
        let result = curve
            .swap_without_fees(
                u128::MAX, // way more token b than needed to get all the token a out
                0, // this doesn't matter (amt of token b left but we're going the other direction)
                1152922643427762284,
                TradeDirection::BtoA,
            )
            .unwrap();

        assert_eq!(
            result.source_amount_swapped,
            1519000000000000 // should still only take this much C
        );
        assert_eq!(result.destination_amount_swapped, 1152922643427762284);
    }

    #[test]
    fn swap_is_slope_valid() {
        // 0 should be Err
        let curve = LinearPriceCurve {
            slope_numerator: 0,
            slope_denominator: 1_000_000_000_001,
            initial_token_r_price: 1,
            initial_token_c_price: 1,
        };
        assert!(!curve.validate().is_ok());

        // undef should be Err
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 0,
            initial_token_r_price: 1,
            initial_token_c_price: 1,
        };
        assert!(!curve.validate().is_ok());

        // less than 1e-12 should be Err
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 1_000_000_000_001,
            initial_token_r_price: 1,
            initial_token_c_price: 1,
        };
        assert!(!curve.validate().is_ok());

        // 1e-12 should be Ok
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 1_000_000_000_000,
            initial_token_r_price: 1,
            initial_token_c_price: 1,
        };
        assert!(curve.validate().is_ok());
    }

    #[test]
    fn swap_too_low_slope_workaround_example() {
        // note this is treated as 0 since the slope is < 1e-12
        let curve = LinearPriceCurve {
            slope_numerator: 1,
            slope_denominator: 10_000_000_000_000,
            initial_token_r_price: 2,
            initial_token_c_price: u64::MAX.into(),
        };

        // 18446744073709551615.00 <- C value at R = 0
        // 18446744073754272974.55 <- C value at R = 100
        // diff is 44721359.55 (but doesn't work)
        let result =
            curve.swap_without_fees(100, 0, 1_00000_00000_00000_00000, TradeDirection::AtoB);
        assert!(result.is_none());

        // as a workaround, we can scale slope and R token amounts (similar to when we give both R and C
        // extra decimals of precision - more examples in swap_a_to_b_basic too)
        let curve = LinearPriceCurve {
            slope_numerator: 1_000_000_000_000_000_000, // slope scaled by 1e18
            slope_denominator: 10_000_000_000_000, // (obviously we could reduce this slope but just illustrative)
            initial_token_r_price: 2, // no need to scale this (it's still 50 base RLY for 1 base CC)
            initial_token_c_price: u64::MAX.into(),
        };

        // 18446744073709551615.00 <- C value at R = 0
        // 18446744073714023750.95 <- C value at R = 100_000_000_000_000
        // diff is 44721359.55 (works now)
        let result = curve.swap_without_fees(
            100_000_000_000_000_000_000, // R values also scaled by 1e18
            0,
            1_00000_00000_00000_00000,
            TradeDirection::AtoB,
        );
        assert_eq!(
            result.unwrap(),
            SwapWithoutFeesResult {
                source_amount_swapped: 100_000_000_000_000_000_000,
                destination_amount_swapped: 44721359 // C value doesn't need any scaling
            }
        );
    }
}
