//! A small non-negative decimal for prices.
//!
//! Prices are configured as decimal strings (`"0.30"`) and must not pass
//! through binary floating point on the way in: `0.30` has no exact `f64`
//! form. `Decimal` keeps an exact integer count of 10⁻¹² units, which is far
//! finer than any per-million-token price needs, and converts to `f64` only
//! at the edge (display, [`cersei_types::Usage::cost_usd`]).

use serde::{Deserialize, Deserializer};
use std::fmt;

const SCALE_DIGITS: usize = 12;
const SCALE: u128 = 1_000_000_000_000;

/// Exact non-negative decimal with 12 fractional digits.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Decimal(u128);

impl Decimal {
    pub const ZERO: Decimal = Decimal(0);

    /// Parse `"12"`, `"0.30"`, `".5"`. No sign, exponent or separators.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("empty decimal".into());
        }
        let (int_part, frac_part) = match text.split_once('.') {
            Some((i, f)) => (i, f),
            None => (text, ""),
        };
        if int_part.is_empty() && frac_part.is_empty() {
            return Err("not a decimal number".into());
        }
        let digits_only = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
        if !digits_only(int_part) || !digits_only(frac_part) {
            return Err("must be a non-negative decimal number such as \"0.30\"".into());
        }
        if frac_part.len() > SCALE_DIGITS {
            return Err(format!("more than {SCALE_DIGITS} fractional digits"));
        }
        let int_val: u128 = if int_part.is_empty() {
            0
        } else {
            int_part
                .parse()
                .map_err(|_| "number too large".to_string())?
        };
        let mut frac = frac_part.to_string();
        while frac.len() < SCALE_DIGITS {
            frac.push('0');
        }
        let frac_val: u128 = frac.parse().map_err(|_| "invalid fraction".to_string())?;
        int_val
            .checked_mul(SCALE)
            .and_then(|v| v.checked_add(frac_val))
            .map(Decimal)
            .ok_or_else(|| "number too large".to_string())
    }

    /// `self * numerator / denominator`, rounded down at 10⁻¹². `None` on overflow.
    pub fn mul_ratio(self, numerator: u64, denominator: u64) -> Option<Decimal> {
        if denominator == 0 {
            return None;
        }
        self.0
            .checked_mul(numerator as u128)
            .map(|v| Decimal(v / denominator as u128))
    }

    pub fn checked_add(self, other: Decimal) -> Option<Decimal> {
        self.0.checked_add(other.0).map(Decimal)
    }

    pub fn to_f64(self) -> f64 {
        self.0 as f64 / SCALE as f64
    }
}

impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let int = self.0 / SCALE;
        let frac = self.0 % SCALE;
        if frac == 0 {
            write!(f, "{int}")
        } else {
            let frac = format!("{frac:0width$}", width = SCALE_DIGITS);
            write!(f, "{int}.{}", frac.trim_end_matches('0'))
        }
    }
}

impl fmt::Debug for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Decimal({self})")
    }
}

impl<'de> Deserialize<'de> for Decimal {
    /// Accepts a decimal string (preferred, exact) or a plain JSON/TOML number.
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Decimal;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a non-negative decimal string such as \"0.30\" (or a number)")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Decimal, E> {
                Decimal::parse(v).map_err(E::custom)
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Decimal, E> {
                Decimal::parse(&v.to_string()).map_err(E::custom)
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Decimal, E> {
                if v < 0 {
                    return Err(E::custom("price must not be negative"));
                }
                Decimal::parse(&v.to_string()).map_err(E::custom)
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Decimal, E> {
                if !v.is_finite() || v < 0.0 {
                    return Err(E::custom("price must be a finite non-negative number"));
                }
                // Shortest round-trip text of the float: `0.3` stays "0.3".
                Decimal::parse(&format!("{v}")).map_err(E::custom)
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exact_decimals() {
        assert_eq!(Decimal::parse("0.30").unwrap().to_string(), "0.3");
        assert_eq!(Decimal::parse("12").unwrap().to_string(), "12");
        assert_eq!(Decimal::parse(".5").unwrap().to_string(), "0.5");
        assert_eq!(
            Decimal::parse("0.000000000001").unwrap().to_string(),
            "0.000000000001"
        );
    }

    #[test]
    fn rejects_bad_input() {
        for bad in [
            "",
            "-1",
            "1e3",
            "1,5",
            "abc",
            ".",
            "1.2.3",
            "0.0000000000001",
        ] {
            assert!(Decimal::parse(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn price_per_million_is_exact() {
        // 1,000 tokens at $0.30 per million tokens = $0.0003 exactly.
        let price = Decimal::parse("0.30").unwrap();
        let cost = price.mul_ratio(1_000, 1_000_000).unwrap();
        assert_eq!(cost.to_string(), "0.0003");
        assert_eq!(cost.checked_add(cost).unwrap().to_string(), "0.0006");
    }

    #[test]
    fn deserializes_strings_and_numbers() {
        let s: Decimal = serde_json::from_str("\"1.20\"").unwrap();
        let n: Decimal = serde_json::from_str("1.2").unwrap();
        let i: Decimal = serde_json::from_str("3").unwrap();
        assert_eq!(s, n);
        assert_eq!(i.to_string(), "3");
        assert!(serde_json::from_str::<Decimal>("-0.5").is_err());
        assert!(serde_json::from_str::<Decimal>("\"-0.5\"").is_err());
    }
}
