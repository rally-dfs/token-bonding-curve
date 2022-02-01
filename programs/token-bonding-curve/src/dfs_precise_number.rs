//! Similar to spl_math::PreciseNumber, a U256 wrapper with float-like operations
//! but instead of having 12 decimals of Precision with `ONE`, we use 30 decimals
//! (so roughly 100 bits of U256 is for decimals and the remaining 156 bits is for the value)
//! The maximum amount supported is lower than spl-math, but should be fine for our purposes
//! since we're only ever operating on wrapped u64 type numbers
//! Also fixes some quirks from PreciseNumber around to_imprecise and removes pow/root
//! since we don't need those (could add them back in if we did more testing around precision)

use spl_math::uint::U256;

// Allows for easy swapping between different internal representations
type InnerUint = U256;

/// The representation of the number one as a precise number as 10^18
/// This differs from spl_math::PreciseNumber's 10^12
/// From testing, any higher than this and linear_curve risks running into compute
/// limits from just the PreciseNumber arithmetic (even ignoring sqrt)
pub const ONE: u128 = 1_000000_000000_000000;
/// Used for sqrt_u64 to correct precision calculation
pub const SQRT_ONE: u128 = 1000_000000;

/// Struct encapsulating a fixed-point number that allows for decimal calculations
#[derive(Clone, Debug, PartialEq)]
pub struct DFSPreciseNumber {
    /// Wrapper over the inner value, which is multiplied by ONE
    pub value: InnerUint,
}

/// The precise-number 1 as a InnerUint
fn one() -> InnerUint {
    InnerUint::from(ONE)
}

/// The number 0 as a PreciseNumber, used for easier calculations.
fn zero() -> InnerUint {
    InnerUint::from(0)
}

impl DFSPreciseNumber {
    /// Correction to apply to avoid truncation errors on division.  Since
    /// integer operations will always floor the result, we artifically bump it
    /// up by one half to get the expect result.
    fn rounding_correction() -> InnerUint {
        InnerUint::from(ONE / 2)
    }

    fn zero() -> Self {
        Self { value: zero() }
    }

    /// Create a precise number from an imprecise u128, should always succeed
    pub fn new(value: u128) -> Option<Self> {
        let value = InnerUint::from(value).checked_mul(one())?;
        Some(Self { value })
    }

    /// Convert a precise number back to u128
    pub fn to_imprecise(&self) -> Option<u128> {
        let corrected = self
            .value
            .checked_add(Self::rounding_correction())?
            .checked_div(one());

        // don't panic if self > u128 max (this differs from spl_math::PreciseNumber)
        match corrected.le(&(Some(InnerUint::from(u128::MAX)))) {
            true => corrected.map(|v| v.as_u128()),
            false => None,
        }
    }

    /// Checks that two PreciseNumbers are equal within some tolerance
    pub fn almost_eq(&self, rhs: &Self, precision: InnerUint) -> bool {
        let (difference, _) = self.unsigned_sub(rhs);
        difference.value < precision
    }

    /// Checks that a number is less than another
    pub fn less_than(&self, rhs: &Self) -> bool {
        self.value < rhs.value
    }

    /// Checks that a number is greater than another
    pub fn greater_than(&self, rhs: &Self) -> bool {
        self.value > rhs.value
    }

    /// Checks that a number is less than another
    pub fn less_than_or_equal(&self, rhs: &Self) -> bool {
        self.value <= rhs.value
    }

    /// Checks that a number is greater than another
    pub fn greater_than_or_equal(&self, rhs: &Self) -> bool {
        self.value >= rhs.value
    }

    /// Floors a precise value to a precision of ONE
    pub fn floor(&self) -> Option<Self> {
        let value = self.value.checked_div(one())?.checked_mul(one())?;
        Some(Self { value })
    }

    /// Ceiling a precise value to a precision of ONE
    pub fn ceiling(&self) -> Option<Self> {
        let value = self
            .value
            .checked_add(one().checked_sub(InnerUint::from(1))?)?
            .checked_div(one())?
            .checked_mul(one())?;
        Some(Self { value })
    }

    /// Performs a checked division on two precise numbers
    pub fn checked_div(&self, rhs: &Self) -> Option<Self> {
        if *rhs == Self::zero() {
            return None;
        }
        match self.value.checked_mul(one()) {
            Some(v) => {
                let value = v
                    .checked_add(Self::rounding_correction())?
                    .checked_div(rhs.value)?;
                Some(Self { value })
            }
            None => {
                let value = self
                    .value
                    .checked_add(Self::rounding_correction())?
                    .checked_div(rhs.value)?
                    .checked_mul(one())?;
                Some(Self { value })
            }
        }
    }

    /// Performs a multiplication on two precise numbers
    pub fn checked_mul(&self, rhs: &Self) -> Option<Self> {
        match self.value.checked_mul(rhs.value) {
            Some(v) => {
                let value = v
                    .checked_add(Self::rounding_correction())?
                    .checked_div(one())?;
                Some(Self { value })
            }
            None => {
                let value = if self.value >= rhs.value {
                    self.value.checked_div(one())?.checked_mul(rhs.value)?
                } else {
                    rhs.value.checked_div(one())?.checked_mul(self.value)?
                };
                Some(Self { value })
            }
        }
    }

    /// Performs addition of two precise numbers
    pub fn checked_add(&self, rhs: &Self) -> Option<Self> {
        let value = self.value.checked_add(rhs.value)?;
        Some(Self { value })
    }

    /// Subtracts the argument from self
    pub fn checked_sub(&self, rhs: &Self) -> Option<Self> {
        let value = self.value.checked_sub(rhs.value)?;
        Some(Self { value })
    }

    /// Performs a subtraction, returning the result and whether the result is negative
    pub fn unsigned_sub(&self, rhs: &Self) -> (Self, bool) {
        match self.value.checked_sub(rhs.value) {
            None => {
                let value = rhs.value.checked_sub(self.value).unwrap();
                (Self { value }, true)
            }
            Some(value) => (Self { value }, false),
        }
    }

    pub fn to_spl_precise_number(&self) -> Option<spl_math::precise_number::PreciseNumber> {
        let value_u128 = self.to_imprecise()?;
        let spl_number = spl_math::precise_number::PreciseNumber::new(value_u128)?;

        // add on the decimals manually
        let decimals_u128 = (self.value % ONE).as_u128();
        let decimals_scaled = spl_math::precise_number::PreciseNumber::new(decimals_u128)?;
        let one = spl_math::precise_number::PreciseNumber::new(ONE)?;
        let decimals = decimals_scaled.checked_div(&one)?;

        spl_number.checked_add(&decimals)
    }

    /// Babylonian sqrt method
    /// Note this will round up to the nearest int depending on `should_round_up`
    fn sqrt_babylonian(x: u64, should_round_up: bool) -> Option<u64> {
        let mut z = match x.checked_add(1) {
            Some(val) => val.checked_div(2)?,
            None => x.checked_div(2)?, // handle u64 max
        };
        let mut y = x;
        while z < y {
            y = z;
            z = x.checked_div(z)?.checked_add(z)?.checked_div(2)?;
        }

        // make sure to add 1 if we're supposed to round up (and it wasn't a perfect square)
        let is_not_perfect_square = y.checked_mul(y)?.lt(&x);

        let rounded_sqrt = match should_round_up && is_not_perfect_square {
            true => y.checked_add(1),
            false => Some(y),
        };

        rounded_sqrt
    }

    /// Takes sqrt to a precision of u64
    /// Differs from spl_math::PreciseNumber's sqrt which just works on the actual U256 self.value
    /// Note we only use u64 here (~10K compute vs ~50K for u128), but we always pad to exactly
    /// 64 bits so we'll be guaranteed ~9 digits of precision at any order of magnitude, so should
    /// be fine
    /// Especially because we're using 18 decimals for ONE instead of 12, using the ~50K u128 version risks
    /// overflowing compute
    pub fn sqrt_u64(&self, should_round_up: bool) -> Option<Self> {
        let value_bits = self.value.bits();
        let max_bits = 64;

        let real_sqrt;
        if value_bits <= max_bits {
            // number is small enough that we should pad bits for more precision
            // make sure pad_bits is an even number since we'll correct by unpadding half the bits at the end
            let pad_bits = (max_bits - value_bits) / 2 * 2;
            // correction_factor is sqrt(2^pad_bits), used below
            let correction_factor = DFSPreciseNumber::new(2u128.pow((pad_bits as u32) / 2))?;

            // solving for real_sqrt below, i.e. the sqrt(real_value)
            // (real_value here is the actual value the PreciseNumber represents, i.e. self.value / ONE)

            // multiply by 2^pad_bits
            // so `padded_value = real_value * 2^pad_bits`
            let padded_value = self.value << pad_bits;

            // we're implicitly multiplying by ONE here (since we converted self.value to u128 directly)
            // so `padded_u128 = real_value * 2^pad_bits * ONE`
            let padded_u128 = padded_value.as_u64();

            // `sqrt_padded_u128 = real_sqrt * sqrt(2^pad_bits) * sqrt(ONE)`
            let sqrt_padded_u128 = Self::sqrt_babylonian(padded_u128, should_round_up)?;

            // since we're converting directly from u128 to PreciseNumber, we're implicitly dividing by ONE
            // so `sqrt_padded = real_sqrt * sqrt(2^pad_bits) * sqrt(ONE) / ONE`
            // -> `sqrt_padded = real_sqrt * sqrt(2^pad_bits) / sqrt(ONE)`
            let sqrt_padded = Self {
                value: InnerUint::from(sqrt_padded_u128),
            };

            // so real_sqrt = sqrt_padded * sqrt(ONE) / sqrt(2^pad_bits)
            // (do this after converting to PreciseNumber so we don't lose precision)
            let unrounded_numerator = sqrt_padded.checked_mul(&(Self::new(SQRT_ONE)?))?;
            let unrounded_sqrt = unrounded_numerator.checked_div(&correction_factor)?;

            // finally, round up if it wasn't a perfect division and we should round up
            real_sqrt = match should_round_up
                && unrounded_sqrt
                    .checked_mul(&correction_factor)?
                    .less_than(&unrounded_numerator)
            {
                true => unrounded_sqrt.checked_add(
                    &(Self {
                        value: InnerUint::from(1),
                    }),
                ),
                false => Some(unrounded_sqrt),
            }
        } else {
            // number is too large, we need to remove precision off the end to not overflow compute
            // this is very similar to the above but we unpad and multiply at the end instead of padding
            // and dividing at the end

            // make sure pad_bits is an even number since we'll correct by unpadding half the bits at the end (make sure we round pad_bits up here since we want to cut off enough to fit into 64 bits)
            let pad_bits = (value_bits - max_bits + 1) / 2 * 2;
            // correction_factor is sqrt(2^pad_bits), used below
            let correction_factor = DFSPreciseNumber::new(2u128.pow((pad_bits as u32) / 2))?;

            // solving for real_sqrt below, i.e. the sqrt(real_value)
            // (real_value here is the actual value the PreciseNumber represents, i.e. self.value / ONE)

            // divide by 2^pad_bits
            // so `padded_value = real_value / 2^pad_bits`
            let unrounded_padded_value = self.value >> pad_bits;

            // round up if it wasn't a perfect division and we should round up
            let padded_value =
                match should_round_up && (unrounded_padded_value << pad_bits).lt(&self.value) {
                    true => unrounded_padded_value.checked_add(InnerUint::from(1))?,
                    false => unrounded_padded_value,
                };

            // we're implicitly multiplying by ONE here (since we converted self.value to u128 directly)
            // so `padded_u128 = real_value * 2^pad_bits / ONE`
            let padded_u128 = padded_value.as_u64();

            // `sqrt_padded_u128 = real_sqrt * sqrt(2^pad_bits) / sqrt(ONE)`
            let sqrt_padded_u128 = Self::sqrt_babylonian(padded_u128, should_round_up)?;

            // since we're converting directly from u128 to PreciseNumber, we're implicitly dividing by ONE
            // so `sqrt_padded = real_sqrt / sqrt(2^pad_bits) * sqrt(ONE) / ONE`
            // -> `sqrt_padded = real_sqrt / sqrt(2^pad_bits) / sqrt(ONE)`
            let sqrt_padded = Self {
                value: InnerUint::from(sqrt_padded_u128),
            };

            // so real_sqrt = sqrt_padded * sqrt(ONE) * sqrt(2^pad_bits)
            // (do this after converting to PreciseNumber so we don't lose precision)
            real_sqrt = sqrt_padded
                .checked_mul(&(Self::new(SQRT_ONE)?))?
                .checked_mul(&correction_factor)
        }

        real_sqrt
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_program::msg;

    #[test]
    fn test_to_imprecise() {
        let number = DFSPreciseNumber::new(0).unwrap();
        assert_eq!(number.floor().unwrap().to_imprecise().unwrap(), 0);

        let number = DFSPreciseNumber::new(u128::MAX).unwrap();
        assert_eq!(number.to_imprecise().unwrap(), u128::MAX);

        // should just return None instead of panic if overflow
        let number = DFSPreciseNumber::new(u128::MAX).unwrap();
        let number = number.checked_add(&number).unwrap();
        assert!(number.to_imprecise().is_none());
    }

    #[test]
    fn test_sqrt_u64() {
        // number below 1 (with uneven number of bits) 1.23456789e-9
        let number = DFSPreciseNumber::new(123456789)
            .unwrap()
            .checked_div(&(DFSPreciseNumber::new(10u128.pow(17)).unwrap()))
            .unwrap();
        assert_eq!(number.value.bits(), 31);
        // sqrt is 3.51364182864446216-5
        let expected_sqrt = DFSPreciseNumber::new(351364182864446216)
            .unwrap()
            .checked_div(&(DFSPreciseNumber::new(10u128.pow(22)).unwrap()))
            .unwrap();
        assert!(
            number
                .sqrt_u64(false)
                .unwrap()
                // precise to first 9 decimals
                .almost_eq(&expected_sqrt, InnerUint::from(ONE / 1_000_000_000)),
            "sqrt {:?} not equal to expected {:?}",
            number.sqrt_u64(false).unwrap(),
            expected_sqrt,
        );

        // number below 1 (with even number of bits) 1e-8
        let number = DFSPreciseNumber::new(1)
            .unwrap()
            .checked_div(&(DFSPreciseNumber::new(10u128.pow(8)).unwrap()))
            .unwrap();
        assert_eq!(number.value.bits(), 34);
        // sqrt is 1-e4
        let expected_sqrt = DFSPreciseNumber::new(1)
            .unwrap()
            .checked_div(&(DFSPreciseNumber::new(10u128.pow(4)).unwrap()))
            .unwrap();
        assert!(
            number
                .sqrt_u64(false)
                .unwrap()
                // precise to first 9 decimals
                .almost_eq(&expected_sqrt, InnerUint::from(ONE / 1_000_000_000)),
            "sqrt {:?} not equal to expected {:?}",
            number.sqrt_u64(false).unwrap(),
            expected_sqrt,
        );

        // exactly max_bits 18446744073709551615e-18 (this is 64 bits of 1, then divided by ONE)
        let number = DFSPreciseNumber::new(18446744073709551615)
            .unwrap()
            .checked_div(&(DFSPreciseNumber::new(10u128.pow(18)).unwrap()))
            .unwrap();
        assert_eq!(number.value.bits(), 64);
        // sqrt is 4.29496729599999999988
        let expected_sqrt = DFSPreciseNumber::new(4294967295999999999)
            .unwrap()
            .checked_div(&(DFSPreciseNumber::new(10u128.pow(18)).unwrap()))
            .unwrap();
        assert!(
            number
                .sqrt_u64(false)
                .unwrap()
                // precise to first 9 decimals
                .almost_eq(&expected_sqrt, InnerUint::from(ONE / 1_000_000_000)),
            "sqrt {:?} not equal to expected {:?}",
            number.sqrt_u64(false).unwrap(),
            expected_sqrt,
        );

        // 1 exactly
        let number = DFSPreciseNumber::new(1).unwrap();
        // sqrt is 1
        let expected_sqrt = DFSPreciseNumber::new(1).unwrap();
        assert!(
            number
                .sqrt_u64(false)
                .unwrap()
                // precise to first 12 decimals
                .almost_eq(&expected_sqrt, InnerUint::from(ONE / 1_000_000_000_000)),
            "sqrt {:?} not equal to expected {:?}",
            number.sqrt_u64(false).unwrap(),
            expected_sqrt,
        );

        // large number, even bits 1234567890123456789
        let number = DFSPreciseNumber::new(1234567890123456789).unwrap();
        assert_eq!(number.value.bits(), 120);
        // sqrt is 1111111106.111111099355555502655555
        let decimals = DFSPreciseNumber::new(111111099355555502655555)
            .unwrap()
            .checked_div(&(DFSPreciseNumber::new(10u128.pow(24)).unwrap()))
            .unwrap();
        let expected_sqrt = DFSPreciseNumber::new(1111111106)
            .unwrap()
            .checked_add(&decimals)
            .unwrap();
        assert!(
            number
                .sqrt_u64(false)
                .unwrap()
                // we lose more precision on these big ones so just first 9 digits
                .almost_eq(&expected_sqrt, InnerUint::from(ONE * 10)),
            "sqrt {:?} not equal to expected {:?}",
            number.sqrt_u64(false).unwrap(),
            expected_sqrt,
        );

        // super large number, odd bits (pretty close to max value of u128) 1.23456789e38
        let number = DFSPreciseNumber::new(123456789)
            .unwrap()
            .checked_mul(&(DFSPreciseNumber::new(10u128.pow(30)).unwrap()))
            .unwrap();
        assert_eq!(number.value.bits(), 187);
        // sqrt is 11111111060555555440.5
        let expected_sqrt = DFSPreciseNumber::new(11111111060555555440).unwrap();
        assert!(
            number
                .sqrt_u64(false)
                .unwrap()
                // we lose more precision on these big ones so just first 9 (of the 20) digits is fine
                .almost_eq(
                    &expected_sqrt,
                    InnerUint::from(ONE)
                        .checked_mul(InnerUint::from(10u128.pow(11)))
                        .unwrap(),
                ),
            "sqrt {:?} not equal to expected {:?}",
            number.sqrt_u64(false).unwrap(),
            expected_sqrt,
        );

        // small perfect square (4e-18), should_round_up=false
        let number = DFSPreciseNumber::new(4)
            .unwrap()
            .checked_div(&(DFSPreciseNumber::new(10u128.pow(18)).unwrap()))
            .unwrap();
        // 2e-9, shouldn't do any rounding
        let expected_sqrt = DFSPreciseNumber::new(2)
            .unwrap()
            .checked_div(&(DFSPreciseNumber::new(10u128.pow(9)).unwrap()))
            .unwrap();
        assert!(
            number.sqrt_u64(false).unwrap().eq(&expected_sqrt),
            "sqrt {:?} not equal to expected {:?}",
            number.sqrt_u64(false).unwrap(),
            expected_sqrt,
        );

        // small perfect square (4e-18), should_round_up=true
        let number = DFSPreciseNumber::new(4)
            .unwrap()
            .checked_div(&(DFSPreciseNumber::new(10u128.pow(18)).unwrap()))
            .unwrap();
        // 2e-9
        let expected_sqrt = DFSPreciseNumber::new(2)
            .unwrap()
            .checked_div(&(DFSPreciseNumber::new(10u128.pow(9)).unwrap()))
            .unwrap();
        assert!(
            number.sqrt_u64(true).unwrap().eq(&expected_sqrt),
            "sqrt {:?} not equal to expected {:?}",
            number.sqrt_u64(true).unwrap(),
            expected_sqrt,
        );

        // small imperfect square (3e-18), should_round_up=false
        let number = DFSPreciseNumber::new(3)
            .unwrap()
            .checked_div(&(DFSPreciseNumber::new(10u128.pow(18)).unwrap()))
            .unwrap();
        // 1.7320508075688e-9 (only room for first 10 digits), should round down to 1.732050807e-9
        let expected_sqrt = DFSPreciseNumber::new(1732050807)
            .unwrap()
            .checked_div(&(DFSPreciseNumber::new(10u128.pow(18)).unwrap()))
            .unwrap();
        assert!(
            number.sqrt_u64(false).unwrap().eq(&expected_sqrt),
            "sqrt {:?} not equal to expected {:?}",
            number.sqrt_u64(false).unwrap(),
            expected_sqrt,
        );

        // small imperfect square (3e-18), should_round_up=true
        let number = DFSPreciseNumber::new(3)
            .unwrap()
            .checked_div(&(DFSPreciseNumber::new(10u128.pow(18)).unwrap()))
            .unwrap();
        // 1.7320508075688e-9 (only room for first 10 digits), should round down to 1.732050808e-9
        let expected_sqrt = DFSPreciseNumber::new(1732050808)
            .unwrap()
            .checked_div(&(DFSPreciseNumber::new(10u128.pow(18)).unwrap()))
            .unwrap();
        assert!(
            number.sqrt_u64(true).unwrap().eq(&expected_sqrt),
            "sqrt {:?} not equal to expected {:?}",
            number.sqrt_u64(true).unwrap(),
            expected_sqrt,
        );

        // perfect square, should_round_up=false
        let number = DFSPreciseNumber::new(400).unwrap();
        let expected_sqrt = DFSPreciseNumber::new(20).unwrap();
        assert!(
            number.sqrt_u64(false).unwrap().eq(&expected_sqrt),
            "sqrt {:?} not equal to expected {:?}",
            number.sqrt_u64(false).unwrap(),
            expected_sqrt,
        );

        // perfect square, should_round_up=true
        let number = DFSPreciseNumber::new(400).unwrap();
        let expected_sqrt = DFSPreciseNumber::new(20).unwrap();
        assert!(
            number.sqrt_u64(true).unwrap().eq(&expected_sqrt),
            "sqrt {:?} not equal to expected {:?}",
            number.sqrt_u64(true).unwrap(),
            expected_sqrt,
        );

        // large imperfect square, should_round_up=false
        let number = DFSPreciseNumber::new(300).unwrap();
        // 17.32050807568
        let expected_sqrt = DFSPreciseNumber::new(1732050807568)
            .unwrap()
            .checked_div(&(DFSPreciseNumber::new(10u128.pow(11)).unwrap()))
            .unwrap();
        assert!(
            number
                .sqrt_u64(false)
                .unwrap()
                // just check first 9 digits (7 decimals) of precision
                .almost_eq(&expected_sqrt, InnerUint::from(ONE / 10_000_000)),
            "sqrt {:?} not equal to expected {:?}",
            number.sqrt_u64(false).unwrap(),
            expected_sqrt,
        );
        // make sure we rounded down though
        assert!(
            number.sqrt_u64(false).unwrap().less_than(&expected_sqrt),
            "sqrt {:?} did not round down from expected {:?}",
            number.sqrt_u64(false).unwrap(),
            expected_sqrt,
        );
        msg!(
            "sqrt {:?}  expected {:?}",
            number.sqrt_u64(false).unwrap(),
            expected_sqrt
        );

        // large imperfect square, should_round_up=true
        let number = DFSPreciseNumber::new(300).unwrap();
        // 17.32050807568
        let expected_sqrt = DFSPreciseNumber::new(1732050807568)
            .unwrap()
            .checked_div(&(DFSPreciseNumber::new(10u128.pow(11)).unwrap()))
            .unwrap();
        assert!(
            number
                .sqrt_u64(true)
                .unwrap()
                // just check first 9 digits (7 decimals) of precision
                .almost_eq(&expected_sqrt, InnerUint::from(ONE / 10_000_000)),
            "sqrt {:?} not equal to expected {:?}",
            number.sqrt_u64(true).unwrap(),
            expected_sqrt,
        );
        // make sure we rounded up though
        assert!(
            number.sqrt_u64(true).unwrap().greater_than(&expected_sqrt),
            "sqrt {:?} did not round down from expected {:?}",
            number.sqrt_u64(true).unwrap(),
            expected_sqrt,
        );
        msg!(
            "sqrt {:?}  expected {:?}",
            number.sqrt_u64(true).unwrap(),
            expected_sqrt
        );
    }

    #[test]
    fn test_floor() {
        let whole_number = DFSPreciseNumber::new(2).unwrap();
        let mut decimal_number = DFSPreciseNumber::new(2).unwrap();
        decimal_number.value += InnerUint::from(1);
        let floor = decimal_number.floor().unwrap();
        let floor_again = floor.floor().unwrap();
        assert_eq!(whole_number.value, floor.value);
        assert_eq!(whole_number.value, floor_again.value);
    }

    #[test]
    fn test_ceiling() {
        let whole_number = DFSPreciseNumber::new(2).unwrap();
        let mut decimal_number = DFSPreciseNumber::new(2).unwrap();
        decimal_number.value -= InnerUint::from(1);
        let ceiling = decimal_number.ceiling().unwrap();
        let ceiling_again = ceiling.ceiling().unwrap();
        assert_eq!(whole_number.value, ceiling.value);
        assert_eq!(whole_number.value, ceiling_again.value);
    }
}
