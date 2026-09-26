use serde_json::Number;
use std::cmp::Ordering;

struct Decimal {
    negative: bool,
    digits: Vec<u8>,
    magnitude: i32,
}

impl Decimal {
    fn from_number(number: &Number) -> Self {
        let text = number.to_string();
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

pub fn compare(left: &Number, right: &Number) -> Ordering {
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
            assert_eq!(compare(&left, &right), expected, "{left} vs {right}");
            assert_eq!(compare(&right, &left), expected.reverse());
        }
    }
}
