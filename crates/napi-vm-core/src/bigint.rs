//! Arbitrary-precision integers, for `BigInt`.
//!
//! Sign-magnitude over base-2³² limbs, little-endian. Base 2³² keeps
//! multiplication and division in 64-bit intermediates without needing 128-bit
//! arithmetic, and makes the decimal conversions straightforward.
//!
//! Only what `BigInt` needs is here: the arithmetic and bitwise operators the
//! language defines on it, comparison, and conversion to and from strings and
//! `f64`.

use std::cmp::Ordering;

/// Guard against a single operation producing an unbounded allocation.
/// `1n << 10000000n` would otherwise try to build a megabyte-scale number in a
/// sandbox; the cap turns that into a catchable error.
const MAX_LIMBS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BigInt {
    /// `true` for a negative value. Zero is always positive.
    negative: bool,
    /// Magnitude, least-significant limb first, with no trailing zeros.
    /// Empty means zero.
    limbs: Vec<u32>,
}

pub type BigResult<T> = Result<T, String>;

fn overflow() -> String {
    "RangeError: BigInt is too large".to_string()
}

impl BigInt {
    pub fn zero() -> Self {
        Self {
            negative: false,
            limbs: Vec::new(),
        }
    }

    pub fn is_zero(&self) -> bool {
        self.limbs.is_empty()
    }

    pub fn is_negative(&self) -> bool {
        self.negative
    }

    fn normalized(negative: bool, mut limbs: Vec<u32>) -> Self {
        while limbs.last() == Some(&0) {
            limbs.pop();
        }
        let negative = negative && !limbs.is_empty();
        Self { negative, limbs }
    }

    pub fn from_i64(value: i64) -> Self {
        let negative = value < 0;
        let magnitude = value.unsigned_abs();
        Self::normalized(negative, vec![magnitude as u32, (magnitude >> 32) as u32])
    }

    /// Parse a literal or a `BigInt(string)` argument. Accepts an optional
    /// sign and the `0x`/`0o`/`0b` radix prefixes.
    pub fn parse(text: &str) -> BigResult<Self> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(Self::zero());
        }
        let (negative, rest) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text.strip_prefix('+').unwrap_or(text)),
        };
        let (radix, digits) = if let Some(d) = rest.strip_prefix("0x").or(rest.strip_prefix("0X")) {
            (16u32, d)
        } else if let Some(d) = rest.strip_prefix("0o").or(rest.strip_prefix("0O")) {
            (8, d)
        } else if let Some(d) = rest.strip_prefix("0b").or(rest.strip_prefix("0B")) {
            (2, d)
        } else {
            (10, rest)
        };
        if digits.is_empty() {
            return Err("SyntaxError: Cannot convert to a BigInt".to_string());
        }
        let mut value = Self::zero();
        let radix_big = Self::from_i64(radix as i64);
        for c in digits.chars() {
            if c == '_' {
                continue;
            }
            let digit = c
                .to_digit(radix)
                .ok_or_else(|| "SyntaxError: Cannot convert to a BigInt".to_string())?;
            value = value.mul(&radix_big)?;
            value = value.add(&Self::from_i64(digit as i64))?;
        }
        value.negative = negative && !value.is_zero();
        Ok(value)
    }

    /// The sign and 64-bit words N-API's `bigint` accessors use.
    pub fn to_words(&self) -> (bool, Vec<u64>) {
        let mut words = Vec::with_capacity(self.limbs.len().div_ceil(2));
        for pair in self.limbs.chunks(2) {
            let low = pair[0] as u64;
            let high = pair.get(1).copied().unwrap_or(0) as u64;
            words.push((high << 32) | low);
        }
        (self.negative, words)
    }

    /// Rebuild from the sign and 64-bit words N-API hands back.
    pub fn from_words(negative: bool, words: &[u64]) -> Self {
        let mut limbs = Vec::with_capacity(words.len() * 2);
        for word in words {
            limbs.push(*word as u32);
            limbs.push((word >> 32) as u32);
        }
        Self::normalized(negative, limbs)
    }

    pub fn to_f64(&self) -> f64 {
        let mut value = 0.0f64;
        for limb in self.limbs.iter().rev() {
            value = value * 4_294_967_296.0 + *limb as f64;
        }
        if self.negative { -value } else { value }
    }

    /// Convert an `f64` that is an exact integer. Fractional or non-finite
    /// input has no `BigInt` equivalent.
    pub fn from_f64(value: f64) -> BigResult<Self> {
        if !value.is_finite() || value.fract() != 0.0 {
            return Err("RangeError: Cannot convert a non-integer to a BigInt".to_string());
        }
        let negative = value < 0.0;
        let mut magnitude = value.abs();
        let mut limbs = Vec::new();
        while magnitude >= 1.0 {
            limbs.push((magnitude % 4_294_967_296.0) as u32);
            magnitude = (magnitude / 4_294_967_296.0).floor();
            if limbs.len() > MAX_LIMBS {
                return Err(overflow());
            }
        }
        Ok(Self::normalized(negative, limbs))
    }

    pub fn to_decimal(&self) -> String {
        if self.is_zero() {
            return "0".to_string();
        }
        // Repeated division by 10⁹, which is the largest power of ten that
        // fits a limb, so each step emits nine decimal digits at once.
        let mut limbs = self.limbs.clone();
        let mut chunks: Vec<u32> = Vec::new();
        while !limbs.is_empty() {
            let mut remainder: u64 = 0;
            for limb in limbs.iter_mut().rev() {
                let current = (remainder << 32) | *limb as u64;
                *limb = (current / 1_000_000_000) as u32;
                remainder = current % 1_000_000_000;
            }
            while limbs.last() == Some(&0) {
                limbs.pop();
            }
            chunks.push(remainder as u32);
        }
        let mut out = String::new();
        if self.negative {
            out.push('-');
        }
        out.push_str(&chunks.pop().unwrap_or(0).to_string());
        while let Some(chunk) = chunks.pop() {
            out.push_str(&format!("{:09}", chunk));
        }
        out
    }

    // --- Magnitude helpers --------------------------------------------------

    fn cmp_magnitude(a: &[u32], b: &[u32]) -> Ordering {
        if a.len() != b.len() {
            return a.len().cmp(&b.len());
        }
        for (x, y) in a.iter().rev().zip(b.iter().rev()) {
            if x != y {
                return x.cmp(y);
            }
        }
        Ordering::Equal
    }

    fn add_magnitude(a: &[u32], b: &[u32]) -> BigResult<Vec<u32>> {
        let mut out = Vec::with_capacity(a.len().max(b.len()) + 1);
        let mut carry = 0u64;
        for index in 0..a.len().max(b.len()) {
            let sum =
                carry + *a.get(index).unwrap_or(&0) as u64 + *b.get(index).unwrap_or(&0) as u64;
            out.push(sum as u32);
            carry = sum >> 32;
        }
        if carry > 0 {
            out.push(carry as u32);
        }
        if out.len() > MAX_LIMBS {
            return Err(overflow());
        }
        Ok(out)
    }

    /// `a - b`, where `a >= b`.
    fn sub_magnitude(a: &[u32], b: &[u32]) -> Vec<u32> {
        let mut out = Vec::with_capacity(a.len());
        let mut borrow = 0i64;
        for (index, limb) in a.iter().enumerate() {
            let mut diff = *limb as i64 - *b.get(index).unwrap_or(&0) as i64 - borrow;
            if diff < 0 {
                diff += 1 << 32;
                borrow = 1;
            } else {
                borrow = 0;
            }
            out.push(diff as u32);
        }
        out
    }

    fn mul_magnitude(a: &[u32], b: &[u32]) -> BigResult<Vec<u32>> {
        if a.is_empty() || b.is_empty() {
            return Ok(Vec::new());
        }
        if a.len() + b.len() > MAX_LIMBS {
            return Err(overflow());
        }
        let mut out = vec![0u32; a.len() + b.len()];
        for (i, x) in a.iter().enumerate() {
            let mut carry = 0u64;
            for (j, y) in b.iter().enumerate() {
                let slot = i + j;
                let total = out[slot] as u64 + (*x as u64) * (*y as u64) + carry;
                out[slot] = total as u32;
                carry = total >> 32;
            }
            let mut slot = i + b.len();
            while carry > 0 {
                let total = out[slot] as u64 + carry;
                out[slot] = total as u32;
                carry = total >> 32;
                slot += 1;
            }
        }
        Ok(out)
    }

    /// Schoolbook long division producing `(quotient, remainder)` magnitudes.
    ///
    /// Bit-at-a-time: `BigInt` division is rare next to the other operators,
    /// and this keeps the implementation small enough to be obviously correct.
    fn divmod_magnitude(a: &[u32], b: &[u32]) -> (Vec<u32>, Vec<u32>) {
        if Self::cmp_magnitude(a, b) == Ordering::Less {
            return (Vec::new(), a.to_vec());
        }
        let bits = a.len() * 32;
        let mut quotient = vec![0u32; a.len()];
        let mut remainder: Vec<u32> = Vec::new();
        for bit in (0..bits).rev() {
            // remainder = remainder * 2 + bit(a, bit)
            let mut carry = (a[bit / 32] >> (bit % 32)) & 1;
            for limb in remainder.iter_mut() {
                let shifted = ((*limb as u64) << 1) | carry as u64;
                *limb = shifted as u32;
                carry = (shifted >> 32) as u32;
            }
            if carry > 0 {
                remainder.push(carry);
            }
            while remainder.last() == Some(&0) {
                remainder.pop();
            }
            if Self::cmp_magnitude(&remainder, b) != Ordering::Less {
                remainder = Self::sub_magnitude(&remainder, b);
                while remainder.last() == Some(&0) {
                    remainder.pop();
                }
                quotient[bit / 32] |= 1 << (bit % 32);
            }
        }
        (quotient, remainder)
    }

    // --- Operators ----------------------------------------------------------

    pub fn add(&self, other: &Self) -> BigResult<Self> {
        if self.negative == other.negative {
            return Ok(Self::normalized(
                self.negative,
                Self::add_magnitude(&self.limbs, &other.limbs)?,
            ));
        }
        match Self::cmp_magnitude(&self.limbs, &other.limbs) {
            Ordering::Equal => Ok(Self::zero()),
            Ordering::Greater => Ok(Self::normalized(
                self.negative,
                Self::sub_magnitude(&self.limbs, &other.limbs),
            )),
            Ordering::Less => Ok(Self::normalized(
                other.negative,
                Self::sub_magnitude(&other.limbs, &self.limbs),
            )),
        }
    }

    pub fn negate(&self) -> Self {
        Self::normalized(!self.negative, self.limbs.clone())
    }

    pub fn sub(&self, other: &Self) -> BigResult<Self> {
        self.add(&other.negate())
    }

    pub fn mul(&self, other: &Self) -> BigResult<Self> {
        Ok(Self::normalized(
            self.negative != other.negative,
            Self::mul_magnitude(&self.limbs, &other.limbs)?,
        ))
    }

    /// Truncating division, as `BigInt` defines it: `7n / 2n` is `3n`, and
    /// `-7n / 2n` is `-3n`.
    pub fn div(&self, other: &Self) -> BigResult<Self> {
        if other.is_zero() {
            return Err("RangeError: Division by zero".to_string());
        }
        let (quotient, _) = Self::divmod_magnitude(&self.limbs, &other.limbs);
        Ok(Self::normalized(self.negative != other.negative, quotient))
    }

    /// Remainder, taking the sign of the dividend.
    pub fn rem(&self, other: &Self) -> BigResult<Self> {
        if other.is_zero() {
            return Err("RangeError: Division by zero".to_string());
        }
        let (_, remainder) = Self::divmod_magnitude(&self.limbs, &other.limbs);
        Ok(Self::normalized(self.negative, remainder))
    }

    pub fn pow(&self, exponent: &Self) -> BigResult<Self> {
        if exponent.negative {
            return Err("RangeError: Exponent must be non-negative".to_string());
        }
        // Square-and-multiply over the exponent's bits, stopping at the
        // highest set one — squaring past it would build an astronomically
        // large intermediate for an exponent as ordinary as 64.
        let mut result = Self::from_i64(1);
        let mut base = self.clone();
        let bits = exponent.bit_length();
        for bit in 0..bits {
            if (exponent.limbs[bit / 32] >> (bit % 32)) & 1 == 1 {
                result = result.mul(&base)?;
            }
            if bit + 1 < bits {
                base = base.mul(&base)?;
            }
        }
        Ok(result)
    }

    /// Position of the highest set bit, plus one. Zero for zero.
    fn bit_length(&self) -> usize {
        match self.limbs.last() {
            None => 0,
            Some(top) => (self.limbs.len() - 1) * 32 + (32 - top.leading_zeros() as usize),
        }
    }

    pub fn compare(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => Self::cmp_magnitude(&self.limbs, &other.limbs),
            (true, true) => Self::cmp_magnitude(&other.limbs, &self.limbs),
        }
    }

    /// Compare against a `Number`. Mixed comparison is allowed even though
    /// mixed *arithmetic* is not.
    pub fn compare_f64(&self, other: f64) -> Option<Ordering> {
        if other.is_nan() {
            return None;
        }
        self.to_f64().partial_cmp(&other)
    }

    pub fn shl(&self, amount: &Self) -> BigResult<Self> {
        if amount.negative {
            return self.shr(&amount.negate());
        }
        let bits = self.small_shift(amount)?;
        let mut limbs = vec![0u32; bits / 32];
        let offset = bits % 32;
        let mut carry = 0u32;
        for limb in &self.limbs {
            limbs.push((limb << offset) | carry);
            carry = if offset == 0 {
                0
            } else {
                limb >> (32 - offset)
            };
        }
        if carry > 0 {
            limbs.push(carry);
        }
        if limbs.len() > MAX_LIMBS {
            return Err(overflow());
        }
        Ok(Self::normalized(self.negative, limbs))
    }

    pub fn shr(&self, amount: &Self) -> BigResult<Self> {
        if amount.negative {
            return self.shl(&amount.negate());
        }
        let bits = self.small_shift(amount)?;
        let drop = bits / 32;
        if drop >= self.limbs.len() {
            // Arithmetic shift: a negative value floors towards -1n.
            return Ok(if self.negative {
                Self::from_i64(-1)
            } else {
                Self::zero()
            });
        }
        let offset = bits % 32;
        let mut limbs: Vec<u32> = Vec::with_capacity(self.limbs.len() - drop);
        for index in drop..self.limbs.len() {
            let low = self.limbs[index] >> offset;
            let high = if offset == 0 {
                0
            } else {
                self.limbs
                    .get(index + 1)
                    .map(|next| next << (32 - offset))
                    .unwrap_or(0)
            };
            limbs.push(low | high);
        }
        let shifted = Self::normalized(self.negative, limbs);
        // `>>` on a negative value rounds towards negative infinity, so a
        // truncated shift that lost any bit is one lower.
        if self.negative {
            let restored = shifted.shl(amount)?;
            if restored.compare(self) != Ordering::Equal {
                return shifted.sub(&Self::from_i64(1));
            }
        }
        Ok(shifted)
    }

    fn small_shift(&self, amount: &Self) -> BigResult<usize> {
        if amount.limbs.len() > 1 {
            return Err(overflow());
        }
        let bits = amount.limbs.first().copied().unwrap_or(0) as usize;
        if bits > MAX_LIMBS * 32 {
            return Err(overflow());
        }
        Ok(bits)
    }

    /// Bitwise operations, in two's-complement terms.
    pub fn bitand(&self, other: &Self) -> BigResult<Self> {
        self.bitwise(other, |a, b| a & b)
    }
    pub fn bitor(&self, other: &Self) -> BigResult<Self> {
        self.bitwise(other, |a, b| a | b)
    }
    pub fn bitxor(&self, other: &Self) -> BigResult<Self> {
        self.bitwise(other, |a, b| a ^ b)
    }
    pub fn bitnot(&self) -> BigResult<Self> {
        // `~x` is `-x - 1` for every integer, which sidesteps needing an
        // infinite sign-extended representation.
        self.negate().sub(&Self::from_i64(1))
    }

    /// Apply `op` limb-wise over the two's-complement forms, widened to a
    /// common length, then convert the result back to sign-magnitude.
    fn bitwise(&self, other: &Self, op: impl Fn(u32, u32) -> u32) -> BigResult<Self> {
        let width = self.limbs.len().max(other.limbs.len()) + 1;
        let a = self.twos_complement(width);
        let b = other.twos_complement(width);
        let out: Vec<u32> = a.iter().zip(b.iter()).map(|(x, y)| op(*x, *y)).collect();
        Ok(Self::from_twos_complement(out))
    }

    fn twos_complement(&self, width: usize) -> Vec<u32> {
        let mut limbs = self.limbs.clone();
        limbs.resize(width, 0);
        if !self.negative {
            return limbs;
        }
        for limb in limbs.iter_mut() {
            *limb = !*limb;
        }
        let mut carry = 1u64;
        for limb in limbs.iter_mut() {
            let sum = *limb as u64 + carry;
            *limb = sum as u32;
            carry = sum >> 32;
            if carry == 0 {
                break;
            }
        }
        limbs
    }

    fn from_twos_complement(mut limbs: Vec<u32>) -> Self {
        let negative = limbs.last().is_some_and(|limb| limb >> 31 == 1);
        if !negative {
            return Self::normalized(false, limbs);
        }
        for limb in limbs.iter_mut() {
            *limb = !*limb;
        }
        let mut carry = 1u64;
        for limb in limbs.iter_mut() {
            let sum = *limb as u64 + carry;
            *limb = sum as u32;
            carry = sum >> 32;
            if carry == 0 {
                break;
            }
        }
        Self::normalized(true, limbs)
    }

    /// `BigInt.asIntN` / `BigInt.asUintN`: wrap to `bits` two's-complement or
    /// unsigned bits.
    pub fn as_n_bit(&self, bits: usize, signed: bool) -> BigResult<Self> {
        if bits == 0 {
            return Ok(Self::zero());
        }
        if bits > MAX_LIMBS * 32 {
            return Err(overflow());
        }
        let width = bits.div_ceil(32) + 1;
        let mut limbs = self.twos_complement(width);
        // Mask everything above `bits`.
        for (index, limb) in limbs.iter_mut().enumerate() {
            let low = index * 32;
            if low >= bits {
                *limb = 0;
            } else if low + 32 > bits {
                *limb &= (1u32 << (bits - low)) - 1;
            }
        }
        let top_bit_set = {
            let index = (bits - 1) / 32;
            (limbs[index] >> ((bits - 1) % 32)) & 1 == 1
        };
        if signed && top_bit_set {
            // Sign-extend so `from_twos_complement` reads it as negative.
            for (index, limb) in limbs.iter_mut().enumerate() {
                let low = index * 32;
                if low >= bits {
                    *limb = u32::MAX;
                } else if low + 32 > bits {
                    *limb |= !((1u32 << (bits - low)) - 1);
                }
            }
            return Ok(Self::from_twos_complement(limbs));
        }
        Ok(Self::normalized(false, limbs))
    }
}

impl std::fmt::Display for BigInt {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.to_decimal())
    }
}

#[cfg(test)]
mod tests {
    use super::BigInt;

    fn big(text: &str) -> BigInt {
        BigInt::parse(text).expect("valid literal")
    }

    #[test]
    fn decimal_round_trips() {
        for text in [
            "0",
            "1",
            "-1",
            "4294967296",
            "9007199254740993",
            "-170141183460469231731687303715884105728",
        ] {
            assert_eq!(big(text).to_decimal(), text);
        }
    }

    #[test]
    fn arithmetic_beyond_f64() {
        // 2^53 + 1 is the first integer an `f64` cannot represent.
        let a = big("9007199254740992");
        let one = big("1");
        assert_eq!(a.add(&one).unwrap().to_decimal(), "9007199254740993");
        assert_eq!(
            big("12345678901234567890")
                .mul(&big("98765432109876543210"))
                .unwrap()
                .to_decimal(),
            "1219326311370217952237463801111263526900"
        );
    }

    #[test]
    fn division_truncates_towards_zero() {
        assert_eq!(big("7").div(&big("2")).unwrap().to_decimal(), "3");
        assert_eq!(big("-7").div(&big("2")).unwrap().to_decimal(), "-3");
        assert_eq!(big("-7").rem(&big("2")).unwrap().to_decimal(), "-1");
    }

    #[test]
    fn shifts_floor_negatives() {
        assert_eq!(
            big("1").shl(&big("64")).unwrap().to_decimal(),
            "18446744073709551616"
        );
        assert_eq!(big("-5").shr(&big("1")).unwrap().to_decimal(), "-3");
        assert_eq!(big("5").shr(&big("1")).unwrap().to_decimal(), "2");
    }

    #[test]
    fn bitwise_uses_twos_complement() {
        assert_eq!(big("-1").bitand(&big("255")).unwrap().to_decimal(), "255");
        assert_eq!(big("12").bitor(&big("3")).unwrap().to_decimal(), "15");
        assert_eq!(big("12").bitxor(&big("10")).unwrap().to_decimal(), "6");
        assert_eq!(big("5").bitnot().unwrap().to_decimal(), "-6");
    }

    #[test]
    fn radix_prefixes_parse() {
        assert_eq!(big("0xff").to_decimal(), "255");
        assert_eq!(big("0b1010").to_decimal(), "10");
        assert_eq!(big("0o17").to_decimal(), "15");
    }

    #[test]
    fn as_n_bit_wraps() {
        assert_eq!(big("255").as_n_bit(8, true).unwrap().to_decimal(), "-1");
        assert_eq!(big("255").as_n_bit(8, false).unwrap().to_decimal(), "255");
        assert_eq!(big("-1").as_n_bit(8, false).unwrap().to_decimal(), "255");
    }

    #[test]
    fn pow_squares_and_multiplies() {
        assert_eq!(
            big("2").pow(&big("64")).unwrap().to_decimal(),
            "18446744073709551616"
        );
        assert_eq!(big("3").pow(&big("0")).unwrap().to_decimal(), "1");
    }
}
