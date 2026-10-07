//! Plain-text rendering for models and terminals. Positions are shown
//! 1-based (line, column in characters): the display convention, applied
//! here only.

use crate::result::*;

fn certainty_label(item: &CodeItem) -> String {
    let how = match item.certainty {
        Certainty::Confirmed => "confirmed",
        Certainty::Syntactic => "syntactic",
        Certainty::Textual => "textual",
    };
    let by: Vec<String> = item
        .provenance
        .iter()
        .map(|p| {
            let b = match p.backend {
                Backend::Lexical => "text",
                Backend::Syntax => "syntax",
                Backend::Lsp => "lsp",
                Backend::Cache => "cache",
            };
            format!("{b}: {}", p.method)
        })
        .collect();
    format!("{how} ({})", by.join("; "))
}

/// One item, numbered `n`.
pub fn render_item(n: usize, item: &CodeItem) -> String {
    let mut out = format!(
        "{n}. {}:{} · {} · {}",
        item.path,
        item.range.display_start(&item.line_text),
        item.relation.as_str(),
        certainty_label(item)
    );
    if let Some(sym) = &item.symbol {
        out.push_str(&format!(" · {} `{}`", sym.kind, sym.name));
    }
    if let Freshness::Stale { reason } = &item.freshness {
        out.push_str(&format!(" · STALE: {reason}"));
    }
    out.push('\n');
    out.push_str(&format!("   id: {}\n", item.id));
    if let Some(sig) = &item.signature {
        out.push_str(&format!("   in: {sig}\n"));
    }
    if let Some(doc) = &item.documentation {
        let doc = crate::syntax::truncate_chars(doc.trim(), 600);
        for l in doc.lines() {
            out.push_str(&format!("   │ {l}\n"));
        }
    }
    match &item.snippet {
        Some(sn) => {
            for (i, part) in sn.parts.iter().enumerate() {
                if i > 0 {
                    out.push_str("   ⋮\n");
                }
                let first = part.range.start.line + 1;
                for (k, l) in part.text.lines().enumerate() {
                    out.push_str(&format!("   {:>5} │ {}\n", first + k as u32, l));
                }
            }
            if sn.omitted_lines > 0 {
                out.push_str(&format!(
                    "   ({} of {} lines shown; {} omitted, ask with detail=deep or read the range)\n",
                    sn.scope_range.end.line - sn.scope_range.start.line + 1 - sn.omitted_lines,
                    sn.scope_range.end.line - sn.scope_range.start.line + 1,
                    sn.omitted_lines
                ));
            }
            if let Some(note) = &sn.syntax_note {
                out.push_str(&format!("   note: {note}\n"));
            }
        }
        None => {
            out.push_str(&format!(
                "   {:>5} │ {}\n",
                item.range.start.line + 1,
                item.line_text
            ));
        }
    }
    out
}

/// The header lines (status, plan).
pub fn render_header(resp: &CodeResponse) -> String {
    let mut out = String::new();
    let status = match &resp.status {
        ResultStatus::Unavailable { reason } => format!("unavailable ({reason})"),
        ResultStatus::Error { message } => format!("error ({message})"),
        s => s.label().to_string(),
    };
    out.push_str(&format!(
        "status: {status} · intent: {} · strategy: {}\n",
        resp.plan.intent, resp.plan.strategy
    ));
    if !resp.plan.reason.is_empty() {
        out.push_str(&format!("why: {}\n", resp.plan.reason));
    }
    let steps: Vec<String> = resp
        .plan
        .steps
        .iter()
        .map(|s| {
            let mut t = format!("{} {:?} {}ms", s.action, s.outcome, s.duration_ms).to_lowercase();
            if let Some(n) = &s.note {
                t.push_str(&format!(" ({n})"));
            }
            t
        })
        .collect();
    if !steps.is_empty() {
        out.push_str(&format!("steps: {}\n", steps.join("; ")));
    }
    for f in &resp.plan.fallbacks {
        out.push_str(&format!("fallback: {f}\n"));
    }
    if let Some(a) = &resp.ambiguity {
        out.push_str(&format!(
            "ambiguous: {} symbols named `{}` — {}\n",
            a.candidates, a.name, a.hint
        ));
    }
    if let Some(d) = &resp.diagnostics {
        let state = match &d.state {
            DiagnosticsState::Analyzed { version } => format!("analyzed (version {version})"),
            DiagnosticsState::Outdated { version } => format!(
                "outdated (last report for version {})",
                version.map(|v| v.to_string()).unwrap_or_else(|| "?".into())
            ),
            DiagnosticsState::Pending => "pending: not analyzed yet, no conclusion".into(),
            DiagnosticsState::Unavailable { reason } => {
                format!("unavailable ({reason}): no diagnostics proves nothing")
            }
        };
        out.push_str(&format!(
            "diagnostics: {} · {state} · {} error(s), {} warning(s) · via {}\n",
            d.path, d.errors, d.warnings, d.method
        ));
        if let (Some(i), Some(p), Some(r)) = (d.introduced, d.preexisting, d.resolved) {
            out.push_str(&format!(
                "compared with revision {}: {i} new, {p} already there, {r} resolved\n",
                d.baseline_revision.as_deref().unwrap_or("?")
            ));
        }
    }
    out
}

/// A whole response.
pub fn render_response(resp: &CodeResponse) -> String {
    let mut out = render_header(resp);
    if resp.items.is_empty() {
        out.push_str(match resp.status {
            ResultStatus::Complete => "no result in the searched scope.\n",
            _ => "no result (the search was not complete: absence proves nothing).\n",
        });
    }
    for (i, item) in resp.items.iter().enumerate() {
        out.push_str(&render_item(i + 1, item));
    }
    for o in &resp.omissions {
        out.push_str(&format!("omitted: {}\n", omission_text(o)));
    }
    out.push_str(&format!(
        "budget: ~{} tokens of {} (estimate, {}){}\n",
        resp.budget.used_tokens,
        resp.budget.limit_tokens,
        resp.budget.method,
        if resp.budget.truncated {
            ", truncated"
        } else {
            ""
        }
    ));
    for c in &resp.continuation {
        out.push_str(&format!("next: {c}\n"));
    }
    out
}

pub fn omission_text(o: &Omission) -> String {
    match o {
        Omission::LimitReached { limit, value } => {
            format!("stopped at the {limit} limit ({value})")
        }
        Omission::Deadline { ms } => format!("time limit reached ({ms} ms)"),
        Omission::FilesSkipped {
            reason,
            count,
            examples,
        } => format!(
            "{count} file(s) skipped: {reason} (e.g. {})",
            examples.join(", ")
        ),
        Omission::ItemsOmitted { reason, count } => format!("{count} item(s) left out: {reason}"),
        Omission::OutOfScope { paths } => format!("outside your scope: {}", paths.join(", ")),
    }
}
