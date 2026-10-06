//! Prices and cost estimates.
//!
//! Prices are declared per million tokens in USD, as exact decimals. An absent
//! price means *unknown* — never "free". A cost is estimated only for the token
//! categories whose counter and price are both known, and the estimate says
//! when it is partial. Media billed per second, per image or per any other
//! non-token unit cannot be expressed with these prices, so it is reported as
//! unpriced rather than guessed.
//!
//! Named tariff variants (off-peak, volume, a different cache retention …) are
//! complete alternatives: a variant does not inherit from the base tariff, so
//! what it omits is unknown. One tariff is selected per estimate and its name is
//! reported; the result is an estimate under that tariff, not an invoice.

use crate::config::PricingConfig;
use crate::decimal::Decimal;
use cersei_types::{CostEstimate, Modality, Usage};
use std::collections::BTreeMap;

/// Name of the base tariff.
pub const DEFAULT_TARIFF: &str = "default";

/// Key under `Usage::provider_usage` where an adapter records cache-write
/// tokens split by retention variant (`{"5m": 120, "1h": 40}`).
pub const CACHE_WRITE_BY_VARIANT: &str = "cache_write_by_variant";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tariff {
    pub input: Option<Decimal>,
    pub output: Option<Decimal>,
    pub cache_read: Option<Decimal>,
    pub cache_write: Option<Decimal>,
    pub cache_write_variants: BTreeMap<String, Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pricing {
    tariffs: BTreeMap<String, Tariff>,
}

impl Pricing {
    /// Validate the written form. The error is `(field, message)`.
    pub fn from_config(c: &PricingConfig) -> Result<Pricing, (String, String)> {
        if c.currency != "USD" {
            return Err((
                "currency".into(),
                "only \"USD\" is supported (prices are USD per million tokens)".into(),
            ));
        }
        if c.per_tokens != 1_000_000 {
            return Err((
                "per_tokens".into(),
                "must be 1000000: prices are expressed per million tokens".into(),
            ));
        }
        let mut tariffs = BTreeMap::new();
        tariffs.insert(
            DEFAULT_TARIFF.to_string(),
            Tariff {
                input: c.input,
                output: c.output,
                cache_read: c.cache_read,
                cache_write: c.cache_write,
                cache_write_variants: c.cache_write_variants.clone(),
            },
        );
        for (name, v) in &c.variants {
            if name == DEFAULT_TARIFF {
                return Err((
                    format!("variants.{name}"),
                    "`default` is the base tariff's name; pick another variant name".into(),
                ));
            }
            if name.trim().is_empty() {
                return Err(("variants".into(), "a variant name must not be empty".into()));
            }
            tariffs.insert(
                name.clone(),
                Tariff {
                    input: v.input,
                    output: v.output,
                    cache_read: v.cache_read,
                    cache_write: v.cache_write,
                    cache_write_variants: v.cache_write_variants.clone(),
                },
            );
        }
        Ok(Pricing { tariffs })
    }

    pub fn tariff(&self, name: &str) -> Option<&Tariff> {
        self.tariffs.get(name)
    }

    pub fn tariff_names(&self) -> impl Iterator<Item = &str> {
        self.tariffs.keys().map(String::as_str)
    }

    /// Estimate the cost of `usage` under `tariff` (default tariff when `None`).
    ///
    /// * `Err` — the tariff name does not exist.
    /// * `Ok(None)` — tokens were consumed but no price at all was known:
    ///   the cost is unknown (not zero).
    /// * `Ok(Some(e))` — `e.partial` is true when some consumed category
    ///   could not be priced, or when `unmetered` lists non-token-billed media.
    ///
    /// `usage` follows the normalization of [`cersei_types::Usage`]: cached
    /// tokens are separate counters (never inside `input_tokens`), and
    /// reasoning tokens are already inside `output_tokens`, so neither is
    /// counted twice here.
    pub fn estimate(
        &self,
        usage: &Usage,
        tariff: Option<&str>,
        unmetered: &[Modality],
    ) -> Result<Option<CostEstimate>, String> {
        let name = tariff.unwrap_or(DEFAULT_TARIFF);
        let t = self.tariffs.get(name).ok_or_else(|| {
            format!(
                "unknown tariff `{name}`; available: {}",
                self.tariffs.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })?;

        let mut total = Decimal::ZERO;
        let mut priced_any = false;
        let mut unpriced: Vec<String> = Vec::new();
        let mut add = |tokens: u64, price: Option<Decimal>, label: &str| {
            if tokens == 0 {
                return;
            }
            match price.and_then(|p| p.mul_ratio(tokens, 1_000_000)) {
                Some(cost) => {
                    if let Some(sum) = total.checked_add(cost) {
                        total = sum;
                        priced_any = true;
                    } else {
                        unpriced.push(label.to_string());
                    }
                }
                None => unpriced.push(label.to_string()),
            }
        };

        add(usage.input_tokens, t.input, "input");
        add(usage.output_tokens, t.output, "output");
        add(usage.cache_read_input_tokens, t.cache_read, "cache_read");

        // Cache writes: price each retention variant the adapter reported with
        // its own price; the remainder uses the generic write price.
        let mut by_variant_total = 0u64;
        if let Some(map) = usage
            .provider_usage
            .get(CACHE_WRITE_BY_VARIANT)
            .and_then(|v| v.as_object())
        {
            for (variant, tokens) in map {
                let tokens = tokens.as_u64().unwrap_or(0);
                by_variant_total += tokens;
                let price = t
                    .cache_write_variants
                    .get(variant)
                    .copied()
                    .or(t.cache_write);
                add(tokens, price, &format!("cache_write[{variant}]"));
            }
        }
        add(
            usage
                .cache_creation_input_tokens
                .saturating_sub(by_variant_total),
            t.cache_write,
            "cache_write",
        );

        for m in unmetered {
            if *m != Modality::Text {
                unpriced.push(format!("{m} (billed per unit, not per token)"));
            }
        }

        let consumed = usage.input_tokens
            + usage.output_tokens
            + usage.cache_read_input_tokens
            + usage.cache_creation_input_tokens
            > 0;
        if !priced_any && (consumed || !unpriced.is_empty()) {
            return Ok(None);
        }
        unpriced.dedup();
        Ok(Some(CostEstimate {
            amount_usd: total.to_f64(),
            tariff: name.to_string(),
            partial: !unpriced.is_empty(),
            unpriced,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TariffConfig;

    fn dec(s: &str) -> Option<Decimal> {
        Some(Decimal::parse(s).unwrap())
    }

    fn full() -> PricingConfig {
        PricingConfig {
            currency: "USD".into(),
            per_tokens: 1_000_000,
            input: dec("3"),
            output: dec("15"),
            cache_read: dec("0.30"),
            cache_write: dec("3.75"),
            cache_write_variants: BTreeMap::from([(
                "1h".to_string(),
                Decimal::parse("6").unwrap(),
            )]),
            variants: BTreeMap::from([(
                "offpeak".to_string(),
                TariffConfig {
                    input: dec("1.5"),
                    output: dec("7.5"),
                    ..Default::default()
                },
            )]),
        }
    }

    fn usage(i: u64, o: u64, cr: u64, cw: u64) -> Usage {
        Usage {
            input_tokens: i,
            output_tokens: o,
            cache_read_input_tokens: cr,
            cache_creation_input_tokens: cw,
            ..Default::default()
        }
    }

    #[test]
    fn full_estimate_with_cache() {
        let p = Pricing::from_config(&full()).unwrap();
        let e = p
            .estimate(&usage(1_000_000, 100_000, 1_000_000, 1_000_000), None, &[])
            .unwrap()
            .unwrap();
        // 3 + 1.5 + 0.30 + 3.75
        assert!((e.amount_usd - 8.55).abs() < 1e-9, "{e:?}");
        assert!(!e.partial && e.tariff == "default" && e.unpriced.is_empty());
    }

    #[test]
    fn cache_write_variants_are_priced_separately() {
        let p = Pricing::from_config(&full()).unwrap();
        let mut u = usage(0, 0, 0, 2_000_000);
        u.provider_usage = serde_json::json!({ CACHE_WRITE_BY_VARIANT: { "1h": 1_000_000 } });
        let e = p.estimate(&u, None, &[]).unwrap().unwrap();
        // 1M at the 1h price (6) + remaining 1M at the generic price (3.75).
        assert!((e.amount_usd - 9.75).abs() < 1e-9, "{e:?}");
    }

    #[test]
    fn missing_price_is_partial_never_free() {
        let mut c = full();
        c.cache_read = None;
        let p = Pricing::from_config(&c).unwrap();
        let e = p
            .estimate(&usage(1_000_000, 0, 500_000, 0), None, &[])
            .unwrap()
            .unwrap();
        assert!(e.partial);
        assert_eq!(e.unpriced, vec!["cache_read".to_string()]);
        assert!(
            (e.amount_usd - 3.0).abs() < 1e-9,
            "only the priced part counts: {e:?}"
        );
    }

    #[test]
    fn no_known_price_is_unknown_not_zero() {
        let mut c = full();
        c.input = None;
        c.output = None;
        c.cache_read = None;
        c.cache_write = None;
        c.cache_write_variants.clear();
        let p = Pricing::from_config(&c).unwrap();
        assert!(p
            .estimate(&usage(10, 10, 0, 0), None, &[])
            .unwrap()
            .is_none());
        // No tokens consumed: a genuine zero.
        let e = p.estimate(&usage(0, 0, 0, 0), None, &[]).unwrap().unwrap();
        assert_eq!(e.amount_usd, 0.0);
        assert!(!e.partial);
    }

    #[test]
    fn named_variant_does_not_inherit() {
        let p = Pricing::from_config(&full()).unwrap();
        let e = p
            .estimate(&usage(1_000_000, 1_000_000, 100, 0), Some("offpeak"), &[])
            .unwrap()
            .unwrap();
        assert_eq!(e.tariff, "offpeak");
        assert!((e.amount_usd - 9.0).abs() < 1e-9, "{e:?}");
        assert!(e.partial, "cache_read is unknown in the variant: {e:?}");
        assert!(p
            .estimate(&usage(1, 1, 0, 0), Some("nope"), &[])
            .unwrap_err()
            .contains("available"));
    }

    #[test]
    fn media_billed_by_unit_makes_it_partial() {
        let p = Pricing::from_config(&full()).unwrap();
        let e = p
            .estimate(
                &usage(1_000_000, 0, 0, 0),
                None,
                &[Modality::Text, Modality::Image],
            )
            .unwrap()
            .unwrap();
        assert!(e.partial);
        assert!(e.unpriced.iter().any(|u| u.starts_with("image")));
    }

    #[test]
    fn config_validation() {
        let mut c = full();
        c.variants.insert("default".into(), TariffConfig::default());
        assert!(Pricing::from_config(&c)
            .unwrap_err()
            .0
            .starts_with("variants"));
        let mut c = full();
        c.currency = "EUR".into();
        assert_eq!(Pricing::from_config(&c).unwrap_err().0, "currency");
    }

    #[test]
    fn usage_merge_flags_partial_across_requests() {
        let p = Pricing::from_config(&full()).unwrap();
        let mut a = usage(1_000_000, 0, 0, 0);
        a.cost_estimate = p.estimate(&a, None, &[]).unwrap();
        let mut total = Usage::default();
        total.merge(&a);
        assert!(!total.cost_estimate.as_ref().unwrap().partial);
        // A later request that consumed tokens with no estimate -> partial.
        let b = usage(10, 10, 0, 0);
        total.merge(&b);
        assert!(total.cost_estimate.as_ref().unwrap().partial);
    }
}
