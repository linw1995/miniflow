use serde_json::Number;
use std::cmp::Ordering;

fn integer_fits_float(magnitude: u64, precision: u32) -> bool {
    u64::BITS - magnitude.leading_zeros() <= precision + magnitude.trailing_zeros()
}

fn integral_in_range(value: f64, range: std::ops::Range<f64>) -> bool {
    value.is_finite()
        && value.fract() == 0.0
        && range.contains(&value)
        && value.to_bits() != (-0.0_f64).to_bits()
}

/// Converts to i64 only when the number is integral and its value and sign are preserved.
pub fn number_to_i64(number: &Number) -> Option<i64> {
    if let Some(value) = number.as_i64() {
        return Some(value);
    }
    let value = number.as_f64().filter(|_| number.is_f64())?;
    integral_in_range(
        value,
        -9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0,
    )
    .then_some(value as i64)
}

/// Converts to u64 without fractional loss, saturation, or loss of negative zero.
pub fn number_to_u64(number: &Number) -> Option<u64> {
    if let Some(value) = number.as_u64() {
        return Some(value);
    }
    let value = number.as_f64().filter(|_| number.is_f64())?;
    integral_in_range(value, 0.0..18_446_744_073_709_551_616.0).then_some(value as u64)
}

pub fn number_to_usize(number: &Number) -> Option<usize> {
    usize::try_from(number_to_u64(number)?).ok()
}

/// Converts to f64 only when integer significand bits are preserved.
pub fn number_to_f64(number: &Number) -> Option<f64> {
    if number.is_f64() {
        return number.as_f64().filter(|value| value.is_finite());
    }
    let magnitude = number
        .as_i64()
        .map(i64::unsigned_abs)
        .or_else(|| number.as_u64())?;
    integer_fits_float(magnitude, f64::MANTISSA_DIGITS)
        .then(|| number.as_f64())
        .flatten()
}

pub fn number_to_f32(number: &Number) -> Option<f32> {
    if number.is_f64() {
        let value = number.as_f64()?;
        let narrowed = value as f32;
        return (narrowed.is_finite() && f64::from(narrowed).to_bits() == value.to_bits())
            .then_some(narrowed);
    }
    let magnitude = number
        .as_i64()
        .map(i64::unsigned_abs)
        .or_else(|| number.as_u64())?;
    integer_fits_float(magnitude, f32::MANTISSA_DIGITS)
        .then(|| number.as_f64().map(|value| value as f32))
        .flatten()
}

struct Decimal {
    negative: bool,
    digits: Vec<u8>,
    magnitude: i32,
}

impl Decimal {
    fn from_number(number: &Number) -> Self {
        let text = if number.is_f64() {
            // Shortest float formatting can spell an exact large integer differently.
            number_to_i64(number)
                .map(|value| value.to_string())
                .or_else(|| number_to_u64(number).map(|value| value.to_string()))
                .unwrap_or_else(|| number.to_string())
        } else {
            number.to_string()
        };
        let negative = text.starts_with('-');
        let unsigned = text.trim_start_matches('-');
        let (coefficient, exponent) = unsigned
            .split_once(['e', 'E'])
            .map(|(coefficient, exponent)| (coefficient, exponent.parse::<i32>().unwrap()))
            .unwrap_or((unsigned, 0));
        let fractional = coefficient
            .split_once('.')
            .map_or(0, |(_, digits)| digits.len() as i32);
        let digits: Vec<u8> = coefficient
            .bytes()
            .filter(|byte| *byte != b'.')
            .skip_while(|byte| *byte == b'0')
            .collect();
        Self {
            negative: negative && !digits.is_empty(),
            magnitude: digits.len() as i32 + exponent - fractional,
            digits,
        }
    }
    fn absolute_cmp(&self, other: &Self) -> Ordering {
        if self.digits.is_empty() || other.digits.is_empty() {
            return self
                .digits
                .is_empty()
                .cmp(&other.digits.is_empty())
                .reverse();
        }
        self.magnitude.cmp(&other.magnitude).then_with(|| {
            (0..self.digits.len().max(other.digits.len()))
                .map(|index| {
                    self.digits
                        .get(index)
                        .unwrap_or(&b'0')
                        .cmp(other.digits.get(index).unwrap_or(&b'0'))
                })
                .find(|order| *order != Ordering::Equal)
                .unwrap_or(Ordering::Equal)
        })
    }
}

pub fn compare_json_numbers(left: &Number, right: &Number) -> Ordering {
    // Decimal normalization preserves integer distinctions that f64 conversion would erase.
    let left = Decimal::from_number(left);
    let right = Decimal::from_number(right);
    match (left.negative, right.negative) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (true, true) => left.absolute_cmp(&right).reverse(),
        (false, false) => left.absolute_cmp(&right),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compares_runtime_numbers_without_rounding_integers() {
        for (left, right, expected) in [
            ("9007199254740993", "9007199254740992", Ordering::Greater),
            (
                "18446744073709551615",
                "18446744073709551614",
                Ordering::Greater,
            ),
            (
                "-9223372036854775808",
                "9223372036854775807",
                Ordering::Less,
            ),
            ("1", "1.0", Ordering::Equal),
            (
                "9223372036854775808",
                "9.223372036854776e18",
                Ordering::Equal,
            ),
            (
                "9223372036854775809",
                "9.223372036854776e18",
                Ordering::Greater,
            ),
            (
                "-9223372036854775808",
                "-9.223372036854776e18",
                Ordering::Equal,
            ),
            (
                "18446744073709549568",
                "1.844674407370955e19",
                Ordering::Equal,
            ),
            ("-0.0", "0", Ordering::Equal),
            ("0", "1e-300", Ordering::Less),
            ("-1e-300", "0", Ordering::Less),
            ("-2", "-1.9", Ordering::Less),
            ("1.01", "1.1", Ordering::Less),
            ("100", "1e2", Ordering::Equal),
            ("1e300", "18446744073709551615", Ordering::Greater),
            ("0.001", "0.0009", Ordering::Greater),
            ("1.0001", "1", Ordering::Greater),
        ] {
            let left: Number = serde_json::from_str(left).unwrap();
            let right: Number = serde_json::from_str(right).unwrap();
            assert_eq!(
                compare_json_numbers(&left, &right),
                expected,
                "{left} vs {right}"
            );
            assert_eq!(compare_json_numbers(&right, &left), expected.reverse());
        }
    }
}
