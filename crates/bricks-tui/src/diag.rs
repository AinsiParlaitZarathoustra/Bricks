//! How cost and context figures are presented (`/cost`, `/context`, the
//! status bar). Every number comes from the engine's views (`Usage`,
//! `ContextStatus`); the only derivations here are for display —
//! percentages, availability, grouping, rounding — and they are named.
//! Nothing is priced or tokenised again.

use cersei_agent::context::{ContextStatus, Provenance, SessionTotals};
use cersei_types::Usage;
use ratatui::text::Line;

/// The state of a cost, from the engine's estimate.
#[derive(Debug, Clone, PartialEq)]
pub enum CostState {
    /// Nothing consumed yet.
    NoUsage,
    /// Every consumed category was priced.
    Complete { amount: f64, tariff: Option<String> },
    /// Some usage could be priced, some could not: `known` covers only the
    /// priced part (a lower bound, not a bill).
    Partial { known: f64, unpriced: Vec<String> },
    /// No price applies to what was consumed (unknown, not zero).
    NotPriced,
}

fn consumed(u: &Usage) -> bool {
    u.input_tokens + u.output_tokens + u.cache_creation_input_tokens + u.cache_read_input_tokens > 0
}

impl CostState {
    pub fn of(u: &Usage) -> Self {
        if !consumed(u) && u.cost_usd.is_none() {
            return CostState::NoUsage;
        }
        match (&u.cost_estimate, u.cost_usd) {
            (Some(e), _) if e.partial => CostState::Partial {
                known: e.amount_usd,
                unpriced: e.unpriced.clone(),
            },
            (Some(e), _) => CostState::Complete {
                amount: e.amount_usd,
                tariff: Some(e.tariff.clone()),
            },
            // An older usage without completeness data: its amount is the
            // estimate the engine made then.
            (None, Some(c)) => CostState::Complete {
                amount: c,
                tariff: None,
            },
            (None, None) => CostState::NotPriced,
        }
    }

    /// The short form (status bar, summary line).
    pub fn short(&self) -> String {
        match self {
            CostState::NoUsage => "No usage yet".into(),
            CostState::Complete { amount, .. } => usd(*amount),
            CostState::Partial { .. } | CostState::NotPriced => "Not priced".into(),
        }
    }
}

/// A dollar amount: `$0.00` only for an exact zero; a small positive amount
/// never reads as zero.
pub fn usd(x: f64) -> String {
    if x == 0.0 {
        "$0.00".into()
    } else if x > 0.0 && x < 0.0001 {
        "<$0.0001".into()
    } else if x < 1.0 {
        format!("${x:.4}")
    } else {
        format!("${x:.2}")
    }
}

/// `25668` → `25,668`.
pub fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `18813` → `18.8k`, `983616` → `984k`, `1000000` → `1.0M`.
pub fn compact(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=99_999 => format!("{:.1}k", n as f64 / 1_000.0),
        100_000..=999_999 => format!("{:.0}k", n as f64 / 1_000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// The context figures, derived for display only.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextFigures {
    /// `≈` when the occupation is (partly) estimated.
    pub approx: bool,
    pub used: u64,
    /// `used / input_limit`, `None` when the budget is zero.
    pub percent: Option<f64>,
    /// `input_limit − margin − upper_bound` (saturating): what the next
    /// request can add and still be admitted — prudent, not a guarantee.
    pub safe_available: u64,
    /// `input_limit − used` (saturating), the plain difference.
    pub raw_available: u64,
}

impl ContextFigures {
    pub fn of(c: &ContextStatus) -> Self {
        let used = c.context_used.tokens;
        Self {
            approx: matches!(
                c.context_used.provenance,
                Provenance::Estimated | Provenance::Mixed
            ),
            used,
            percent: (c.input_limit > 0).then(|| used as f64 / c.input_limit as f64 * 100.0),
            safe_available: c
                .input_limit
                .saturating_sub(c.margin)
                .saturating_sub(c.context_used.upper_bound),
            raw_available: c.input_limit.saturating_sub(used),
        }
    }

    pub fn mark(&self) -> &'static str {
        if self.approx { "≈" } else { "" }
    }

    /// `≈1.9%`, `120.4%`, or `—` without a budget.
    pub fn percent_text(&self) -> String {
        match self.percent {
            Some(p) => format!("{}{p:.1}%", self.mark()),
            None => "—".into(),
        }
    }
}

pub fn provenance_label(p: Provenance) -> &'static str {
    match p {
        Provenance::Measured => "Measured by the server",
        Provenance::Counted => "Counted by the server",
        Provenance::Mixed => "Measured + local estimate",
        Provenance::Estimated => "Local estimate",
    }
}

/// Aligned `label  value` lines.
struct Table {
    width: usize,
    lines: Vec<Line<'static>>,
}

impl Table {
    fn new(width: usize) -> Self {
        Self {
            width,
            lines: Vec::new(),
        }
    }
    fn title(&mut self, t: &str) {
        self.lines.push(Line::from(t.to_string()));
        self.lines.push(Line::default());
    }
    fn section(&mut self, t: &str) {
        self.lines.push(Line::default());
        self.lines.push(Line::from(t.to_string()));
    }
    fn row(&mut self, label: &str, value: impl Into<String>) {
        let pad = self.width.saturating_sub(label.chars().count()).max(1);
        self.lines.push(Line::from(format!(
            "{label}{}{}",
            " ".repeat(pad),
            value.into()
        )));
    }
    /// A number right-aligned in a column of `w` characters.
    fn num(&mut self, label: &str, n: u64, w: usize) {
        self.row(label, format!("{:>w$}", group(n)));
    }
    fn text(&mut self, t: impl Into<String>) {
        self.lines.push(Line::from(t.into()));
    }
}

/// `/cost`: the session's usage, from the engine's totals.
pub fn cost_summary(model: &str, usage: &Usage, totals: &SessionTotals) -> Vec<Line<'static>> {
    let mut t = Table::new(16);
    t.title("Session Usage");
    let state = CostState::of(usage);
    t.row("Current model", model.to_string());
    let mut requests = group(totals.requests);
    if totals.compaction_requests > 0 {
        requests.push_str(&format!(
            " ({} for compaction)",
            group(totals.compaction_requests)
        ));
    }
    t.row("Requests", requests);
    if totals.requests_without_usage > 0 {
        t.row(
            "Without usage",
            format!(
                "{} (not counted below)",
                group(totals.requests_without_usage)
            ),
        );
    }
    t.row(
        "Pricing",
        match &state {
            CostState::Complete { amount, .. } => format!("{} (estimate)", usd(*amount)),
            CostState::Partial { .. } => "Not priced (partial)".into(),
            other => other.short(),
        },
    );
    if state == CostState::NoUsage {
        t.section("No usage observed yet in this session.");
        return t.lines;
    }
    let w = [
        usage.input_tokens,
        usage.cache_read_input_tokens,
        usage.cache_creation_input_tokens,
        usage.output_tokens,
    ]
    .iter()
    .map(|n| group(*n).len())
    .max()
    .unwrap_or(1);
    t.section("Tokens");
    t.num("Input", usage.input_tokens, w);
    t.num("Cache read", usage.cache_read_input_tokens, w);
    t.num("Cache write", usage.cache_creation_input_tokens, w);
    t.num("Output", usage.output_tokens, w);
    t.num("  Reasoning", usage.reasoning_tokens, w);
    t.lines.push(Line::default());
    t.text("Input excludes cache reads and writes; reasoning is part of output.");
    match &state {
        CostState::Partial { known, unpriced } => {
            t.text("Pricing unavailable for part of this usage.");
            t.text(format!(
                "Known subtotal: {} — priced categories only, not a total.",
                usd(*known)
            ));
            if !unpriced.is_empty() {
                t.text(format!("Not priced: {}.", unpriced.join(", ")));
            }
        }
        CostState::NotPriced => {
            t.text("Pricing unavailable for this usage: no price applies (unknown, not zero).")
        }
        CostState::Complete { .. } => {
            t.text("An estimate from the configured prices, not an invoice.")
        }
        CostState::NoUsage => {}
    }
    t.text("Totals cover the whole session, every model it used, compactions included.");
    t.lines
}

/// `/cost details`: per response, as this interface received them.
pub fn cost_details(rows: &[Usage], total: &Usage) -> Vec<Line<'static>> {
    let mut out = vec![Line::from("Usage per response"), Line::default()];
    if rows.is_empty() {
        out.push(Line::from(
            "No response received by this interface since the session was opened.",
        ));
        out.push(Line::from(
            "(A resumed session's earlier requests are only in its total: /cost.)",
        ));
        return out;
    }
    out.push(Line::from(format!(
        "{:>4}  {:>10}  {:>10}  {:>10}  {:>10}  {:>10}  {}",
        "#", "Input", "Cache rd", "Cache wr", "Output", "Reasoning", "Cost"
    )));
    for (i, u) in rows.iter().enumerate() {
        let state = CostState::of(u);
        let cost = match &state {
            CostState::Partial { known, .. } => format!("partial ≥ {}", usd(*known)),
            other => other.short(),
        };
        out.push(Line::from(format!(
            "{:>4}  {:>10}  {:>10}  {:>10}  {:>10}  {:>10}  {cost}",
            i + 1,
            group(u.input_tokens),
            group(u.cache_read_input_tokens),
            group(u.cache_creation_input_tokens),
            group(u.output_tokens),
            group(u.reasoning_tokens),
        )));
    }
    out.push(Line::default());
    out.push(Line::from(format!(
        "{} response(s) shown: those received by this interface since the session was \
         opened or resumed (compaction calls not listed).",
        rows.len()
    )));
    if let CostState::Partial { unpriced, .. } = CostState::of(total)
        && !unpriced.is_empty()
    {
        out.push(Line::from(format!(
            "Categories without a price: {}.",
            unpriced.join(", ")
        )));
    }
    out
}

/// `/context`: occupation of the active context, apart from the session's
/// cumulative consumption.
pub fn context_summary(c: &ContextStatus) -> Vec<Line<'static>> {
    let f = ContextFigures::of(c);
    let m = f.mark();
    let mut t = Table::new(19);
    t.title("Context");
    t.row("Model", c.context_window.model.clone());
    t.row(
        "Window",
        c.context_window
            .total
            .map(|w| format!("{} tokens", group(w)))
            .unwrap_or_else(|| "Not configured".into()),
    );
    t.row("Prompt budget", format!("{} tokens", group(c.input_limit)));
    t.lines.push(Line::default());
    t.row("Current usage", format!("{m}{} tokens", group(f.used)));
    t.row("Prompt occupation", f.percent_text());
    t.row(
        "Available (safe)",
        format!("{}{} tokens", m, group(f.safe_available)),
    );
    t.lines.push(Line::default());
    t.row("Source", provenance_label(c.context_used.provenance));
    t.row("Session total", format!("{} tokens", group(c.total_tokens)));
    t.row("Requests", group(c.totals.requests));
    t.lines.push(Line::default());
    t.text("Occupation is what the next request carries; the session total only grows.");
    t.text("`/context details` for the measurement, margins and notes.");
    t.lines
}

/// `/context details`.
pub fn context_details(c: &ContextStatus) -> Vec<Line<'static>> {
    let f = ContextFigures::of(c);
    let u = &c.context_used;
    let mut t = Table::new(24);
    t.title("Context — details");
    t.row("Provenance", provenance_label(u.provenance));
    t.row(
        "Measured or counted",
        format!("{} tokens", group(u.measured_tokens)),
    );
    t.row("Estimated", format!("{} tokens", group(u.estimated_tokens)));
    t.row("Estimate method", u.method.clone());
    t.row("Upper bound", format!("{} tokens", group(u.upper_bound)));
    t.section("Budget");
    t.row(
        "Window",
        c.context_window
            .total
            .map(|w| format!("{} tokens", group(w)))
            .unwrap_or_else(|| "Not configured".into()),
    );
    t.row(
        "Max input (model)",
        format!("{} tokens", group(c.context_window.max_input_tokens)),
    );
    t.row(
        "Max output (model)",
        c.context_window
            .max_output_tokens
            .map(|o| format!("{} tokens", group(o)))
            .unwrap_or_else(|| "not reported".into()),
    );
    t.row(
        "Output reserve",
        format!("{} tokens", group(c.reserved_output)),
    );
    t.row("Prompt budget", format!("{} tokens", group(c.input_limit)));
    t.row("Safety margin", format!("{} tokens", group(c.margin)));
    t.section("Availability");
    t.row(
        "Safe (admission)",
        format!("{}{} tokens", f.mark(), group(f.safe_available)),
    );
    t.text("  = prompt budget − margin − upper bound; prudent, not a server guarantee.");
    t.row(
        "Raw",
        format!("{}{} tokens", f.mark(), group(f.raw_available)),
    );
    t.text("  = prompt budget − current usage.");
    t.row("Occupation", f.percent_text());
    t.section("Session (cumulative)");
    let s = &c.totals;
    t.row("Total tokens", group(c.total_tokens));
    t.row("Input", group(s.input_tokens));
    t.row("Cache read", group(s.cache_read_tokens));
    t.row("Cache write", group(s.cache_write_tokens));
    t.row("Output", group(s.output_tokens));
    t.row("  Reasoning", group(s.reasoning_tokens));
    t.row("Requests", group(s.requests));
    t.row("  for compaction", group(s.compaction_requests));
    t.row("  without usage", group(s.requests_without_usage));
    if !c.notes.is_empty() {
        t.section("Notes");
        for n in &c.notes {
            t.text(format!("- {n}"));
        }
    }
    t.lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use cersei_types::CostEstimate;

    fn text(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn usage(input: u64, output: u64, est: Option<(f64, bool, &[&str])>) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_read_input_tokens: 89_600,
            cache_creation_input_tokens: 2_400,
            reasoning_tokens: 1_398,
            cost_usd: est.map(|e| e.0),
            cost_estimate: est.map(|(a, p, u)| CostEstimate {
                amount_usd: a,
                tariff: "standard".into(),
                partial: p,
                unpriced: u.iter().map(|s| s.to_string()).collect(),
            }),
            ..Default::default()
        }
    }

    fn totals(requests: u64) -> SessionTotals {
        SessionTotals {
            requests,
            ..Default::default()
        }
    }

    #[test]
    fn the_four_cost_states_and_no_usage() {
        assert_eq!(CostState::of(&Usage::default()), CostState::NoUsage);
        assert_eq!(CostState::of(&Usage::default()).short(), "No usage yet");
        let complete = usage(10, 5, Some((1.25, false, &[])));
        assert_eq!(CostState::of(&complete).short(), "$1.25");
        let zero = usage(10, 5, Some((0.0, false, &[])));
        assert_eq!(CostState::of(&zero).short(), "$0.00");
        let none = usage(10, 5, None);
        assert_eq!(CostState::of(&none).short(), "Not priced");
        let partial = usage(10, 5, Some((0.42, true, &["image input"])));
        assert_eq!(
            CostState::of(&partial).short(),
            "Not priced",
            "never a total"
        );
        let s = text(&cost_summary("p/m", &partial, &totals(3)));
        assert!(s.contains("Not priced (partial)"), "{s}");
        assert!(s.contains("Known subtotal: $0.4200 — priced categories only, not a total."));
        assert!(s.contains("Not priced: image input."));
        let s = text(&cost_summary("p/m", &none, &totals(3)));
        assert!(s.contains("unknown, not zero") && !s.contains("$0"), "{s}");
        let s = text(&cost_summary("p/m", &zero, &totals(3)));
        assert!(s.contains("$0.00 (estimate)"), "{s}");
        let s = text(&cost_summary("p/m", &Usage::default(), &totals(0)));
        assert!(s.contains("No usage observed yet"), "{s}");
        // An old usage without completeness metadata: its amount, as is.
        let old = Usage {
            input_tokens: 5,
            cost_usd: Some(0.5),
            ..Default::default()
        };
        assert_eq!(CostState::of(&old).short(), "$0.5000");
    }

    #[test]
    fn small_amounts_never_read_as_zero_and_numbers_are_grouped() {
        assert_eq!(usd(0.00004), "<$0.0001");
        assert_eq!(usd(0.0123), "$0.0123");
        assert_eq!(usd(12.345), "$12.35");
        assert_eq!(group(25_668), "25,668");
        assert_eq!(group(999), "999");
        assert_eq!(group(1_000_000), "1,000,000");
        assert_eq!(compact(18_813), "18.8k");
        assert_eq!(compact(983_616), "984k");
    }

    #[test]
    fn cache_and_reasoning_are_shown_apart_and_not_added_twice() {
        let u = usage(25_668, 3_557, None);
        let s = text(&cost_summary("Example", &u, &totals(12)));
        for want in ["25,668", "89,600", "2,400", "3,557", "  Reasoning", "1,398"] {
            assert!(s.contains(want), "{want}: {s}");
        }
        // No total mixing categories: 25,668 + 3,557 + 1,398 never appears.
        assert!(!s.contains("30,623") && !s.contains("29,225"));
        assert!(s.contains("reasoning is part of output"));
        // Accumulated across priced and unpriced requests: partial.
        let mut total = usage(10, 5, Some((0.1, false, &[])));
        total.merge(&usage(10, 5, None));
        assert!(matches!(CostState::of(&total), CostState::Partial { .. }));
    }

    #[test]
    fn details_say_what_they_cover() {
        let s = text(&cost_details(&[], &Usage::default()));
        assert!(s.contains("No response received by this interface"));
        let rows = vec![
            usage(100, 10, Some((0.00002, false, &[]))),
            usage(200, 20, None),
        ];
        let s = text(&cost_details(&rows, &rows[0]));
        assert!(s.contains("<$0.0001") && s.contains("Not priced"), "{s}");
        assert!(s.contains("2 response(s) shown"));
    }

    fn status(prov: &str, used: u64, upper: u64, limit: u64, total: Option<u64>) -> ContextStatus {
        serde_json::from_value(serde_json::json!({
            "context_used": {"tokens": used, "upper_bound": upper, "measured_tokens": if prov == "estimated" {0} else {used},
                "estimated_tokens": if prov == "estimated" {used} else {0}, "provenance": prov, "method": "chars/3.6"},
            "context_window": {"model": "Example Model", "total": total, "max_input_tokens": limit, "max_output_tokens": 16384},
            "input_limit": limit, "reserved_output": 16384, "margin": 19672, "total_tokens": 121225,
            "totals": {"input_tokens": 1, "cache_read_tokens": 0, "cache_write_tokens": 0, "output_tokens": 0,
                "reasoning_tokens": 0, "requests": 12, "requests_without_usage": 0, "compaction_requests": 0},
            "notes": ["estimated since: compaction #1 rewrote the history"]
        }))
        .unwrap()
    }

    #[test]
    fn context_marks_estimates_and_keeps_its_figures_apart() {
        for (prov, approx) in [
            ("measured", false),
            ("counted", false),
            ("mixed", true),
            ("estimated", true),
        ] {
            let c = status(prov, 18_813, 19_500, 983_616, Some(1_000_000));
            let f = ContextFigures::of(&c);
            assert_eq!(f.approx, approx, "{prov}");
            let s = text(&context_summary(&c));
            assert_eq!(s.contains("≈18,813"), approx, "{prov}: {s}");
            assert!(s.contains("1,000,000 tokens") && s.contains("983,616 tokens"));
            assert!(s.contains("121,225") && s.contains("Requests"));
        }
        let c = status("estimated", 18_813, 19_500, 983_616, Some(1_000_000));
        let f = ContextFigures::of(&c);
        assert_eq!(f.safe_available, 983_616 - 19_672 - 19_500);
        assert_eq!(f.raw_available, 983_616 - 18_813);
        assert_eq!(f.percent_text(), "≈1.9%");
        let d = text(&context_details(&c));
        assert!(d.contains("Safe (admission)") && d.contains("Raw"));
        assert!(d.contains(&group(f.safe_available)) && d.contains(&group(f.raw_available)));
        assert!(d.contains("compaction #1 rewrote"), "notes kept");
        // No window: not replaced by the budget.
        let s = text(&context_summary(&status("measured", 10, 10, 5_000, None)));
        assert!(s.contains("Not configured"));
        // Zero budget: no percentage; over budget: not capped.
        assert_eq!(
            ContextFigures::of(&status("measured", 10, 10, 0, None)).percent_text(),
            "—"
        );
        let over = ContextFigures::of(&status("measured", 6_000, 6_000, 5_000, None));
        assert_eq!(over.percent_text(), "120.0%");
        assert_eq!(over.safe_available, 0);
    }
}
