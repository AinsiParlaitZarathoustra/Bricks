//! Compression of command logs: diagnostics first, then rules, then budget.
//!
//! The order matters. Lines are cleaned (ANSI, carriage-return progress),
//! structured records are rendered, and diagnostic blocks are found *before*
//! any rule removes a line or any budget cuts one. Rules then act only on the
//! remaining lines; the budget keeps diagnostics in priority and shows every
//! omission in place. Nothing is merged except exact consecutive repeats, so
//! two distinct diagnostics never become one.

use crate::ansi;
use crate::errors::{self, Severity};
use crate::rules::{LineFilter, Rule};
use crate::structured;

/// Separator a tool may put between stdout and stderr; always kept.
pub const STDERR_MARKER: &str = "--- stderr ---";

/// Size limits of a compressed log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLimits {
    /// Budget for lines that are not diagnostics (a rule may lower it).
    pub max_lines: usize,
    /// Budget for diagnostic lines (errors first, then warnings).
    pub max_error_lines: usize,
    /// A longer diagnostic block keeps its head and tail.
    pub max_block_lines: usize,
    /// Lines kept around each error block.
    pub context_lines: usize,
    /// Longest ordinary line, in characters (a rule may lower it).
    pub max_line_chars: usize,
    /// Longest diagnostic line, in characters.
    pub max_diagnostic_line_chars: usize,
}

impl Default for LogLimits {
    fn default() -> Self {
        Self {
            max_lines: 300,
            max_error_lines: 250,
            max_block_lines: 80,
            context_lines: 2,
            max_line_chars: 2000,
            max_diagnostic_line_chars: 4000,
        }
    }
}

/// The compressed log and what happened to it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogReport {
    pub text: String,
    pub changed: bool,
    pub exit_code: Option<i32>,
    pub error_blocks: usize,
    pub warning_blocks: usize,
    /// Diagnostic blocks that did not fit at all.
    pub omitted_blocks: usize,
    /// Lines removed by rules (noise) — not shown in place.
    pub filtered_lines: usize,
    /// Lines omitted by the budget — shown in place as `… [N lines omitted]`.
    pub omitted_lines: usize,
    /// Of `omitted_lines`, how many belonged to diagnostics.
    pub omitted_diagnostic_lines: usize,
    pub notes: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Class {
    Rest,
    Protected,
    Warning,
    Error,
    Fixed,
}

#[derive(Debug)]
struct Line {
    text: String,
    class: Class,
    alive: bool,
    keep: bool,
    block: Option<usize>,
    repeat: usize,
}

pub fn compress_log(raw: &str, rule: Option<&Rule>, limits: &LogLimits) -> LogReport {
    compress_log_with_exit(raw, rule, limits, None)
}

/// Section headers of rendered process output; always kept.
fn is_section_marker(line: &str) -> bool {
    line == STDERR_MARKER || line == "--- stdout ---" || line.starts_with("--- sortie (")
}

/// [`compress_log`] with the exit code known from outside the text (a
/// leading `Exit code N` line is still recognised).
pub fn compress_log_with_exit(
    raw: &str,
    rule: Option<&Rule>,
    limits: &LogLimits,
    exit_code: Option<i32>,
) -> LogReport {
    let mut report = LogReport::default();
    let mut src: Vec<String> = raw.lines().map(ansi::clean_line).collect();

    let leading_code: Option<i32> = src
        .first()
        .and_then(|l| l.strip_prefix("Exit code "))
        .and_then(|c| c.trim().parse().ok());
    report.exit_code = exit_code.or(leading_code);

    if let Some(conv) = structured::convert(&src, rule.and_then(|r| r.structured)) {
        report.notes.push(conv.note);
        src = conv.lines;
    }

    let detectors = rule.map(|r| r.detectors.as_slice()).unwrap_or(&[]);
    let blocks = errors::detect(&src, detectors);
    report.error_blocks = blocks
        .iter()
        .filter(|b| b.severity == Severity::Error)
        .count();
    report.warning_blocks = blocks.len() - report.error_blocks;

    let mut lines: Vec<Line> = src
        .into_iter()
        .map(|text| Line {
            text,
            class: Class::Rest,
            alive: true,
            keep: false,
            block: None,
            repeat: 0,
        })
        .collect();

    // ── classify ──
    if leading_code.is_some() {
        lines[0].class = Class::Fixed;
    }
    for l in lines.iter_mut() {
        if is_section_marker(&l.text) {
            l.class = Class::Fixed;
        }
    }
    for (bi, b) in blocks.iter().enumerate() {
        let class = match b.severity {
            Severity::Error => Class::Error,
            Severity::Warning => Class::Warning,
        };
        let ctx = if b.severity == Severity::Error {
            limits.context_lines
        } else {
            0
        };
        let from = b.start.saturating_sub(ctx);
        let to = (b.end + ctx).min(lines.len() - 1);
        for (i, l) in lines.iter_mut().enumerate().take(to + 1).skip(from) {
            let inside = i >= b.start && i <= b.end;
            if inside || l.class == Class::Rest {
                if l.class != Class::Fixed {
                    l.class = class;
                }
                l.block.get_or_insert(bi);
            }
        }
    }
    if let Some(set) = rule.and_then(|r| r.protect.as_ref()) {
        for l in lines.iter_mut().filter(|l| l.class == Class::Rest) {
            if set.is_match(&l.text) {
                l.class = Class::Protected;
            }
        }
    }

    // ── match_output: only for a clean success ──
    if let Some(rule) = rule {
        let success = matches!(report.exit_code, None | Some(0));
        if success && blocks.is_empty() && !rule.match_output.is_empty() {
            let blob = lines
                .iter()
                .map(|l| l.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            for m in &rule.match_output {
                if m.pattern.is_match(&blob)
                    && !m.unless.as_ref().is_some_and(|u| u.is_match(&blob))
                {
                    report.text = m.message.clone();
                    report.changed = true;
                    report.filtered_lines = lines.len();
                    report.notes.push(format!(
                        "the whole output was replaced by the summary of rule `{}`",
                        rule.id
                    ));
                    return report;
                }
            }
        }
    }

    // ── rule transformations on ordinary lines ──
    if let Some(rule) = rule {
        for l in lines.iter_mut().filter(|l| l.class == Class::Rest) {
            for (re, rep) in &rule.replace {
                l.text = re.replace_all(&l.text, rep.as_str()).into_owned();
            }
            let drop = match &rule.line_filter {
                LineFilter::None => false,
                LineFilter::Strip(set) => set.is_match(&l.text),
                LineFilter::Keep(set) => !set.is_match(&l.text),
            };
            if drop {
                l.alive = false;
                report.filtered_lines += 1;
            }
        }
        summarize_runs(&mut lines, rule);
    }

    // ── exact consecutive repeats ──
    let mut prev: Option<usize> = None;
    for i in 0..lines.len() {
        if !lines[i].alive || lines[i].class == Class::Fixed {
            continue;
        }
        if let Some(p) = prev {
            if lines[p].text == lines[i].text && !lines[i].text.trim().is_empty() {
                lines[p].repeat += 1;
                lines[i].alive = false;
                report.filtered_lines += 1;
                continue;
            }
        }
        prev = Some(i);
    }

    // ── long lines ──
    let rest_chars = rule
        .and_then(|r| r.truncate_lines_at)
        .unwrap_or(limits.max_line_chars)
        .min(limits.max_line_chars);
    for l in lines.iter_mut().filter(|l| l.alive) {
        let cap = if l.class >= Class::Warning {
            limits.max_diagnostic_line_chars
        } else {
            rest_chars
        };
        if l.text.len() > cap {
            l.text = ansi::cut_chars(&l.text, cap);
        }
    }

    // ── budget ──
    select(&mut lines, &blocks, rule, limits, &mut report);

    // ── render ──
    let mut out: Vec<String> = Vec::new();
    let (mut gap, mut gap_diag) = (0usize, 0usize);
    let flush = |out: &mut Vec<String>, gap: &mut usize, gap_diag: &mut usize| {
        if *gap > 0 {
            out.push(if *gap_diag > 0 {
                format!("… [{} lines omitted, {} of them diagnostic]", gap, gap_diag)
            } else {
                format!("… [{gap} {} omitted]", plural(*gap))
            });
            *gap = 0;
            *gap_diag = 0;
        }
    };
    for l in &lines {
        if !l.alive {
            continue;
        }
        if l.keep {
            flush(&mut out, &mut gap, &mut gap_diag);
            if l.repeat > 0 {
                out.push(format!("{}  [repeated {}×]", l.text, l.repeat + 1));
            } else {
                out.push(l.text.clone());
            }
        } else {
            gap += 1 + l.repeat;
            if l.class >= Class::Warning {
                gap_diag += 1 + l.repeat;
            }
        }
    }
    flush(&mut out, &mut gap, &mut gap_diag);

    if out.iter().all(|l| l.trim().is_empty()) && report.filtered_lines > 0 {
        let msg = rule.and_then(|r| r.on_empty.clone()).unwrap_or_else(|| {
            format!(
                "(no output left after removing {} progress/noise lines)",
                report.filtered_lines
            )
        });
        out = vec![msg];
    }

    if report.filtered_lines > 0 {
        let by = rule
            .map(|r| format!(" by rule `{}`", r.id))
            .unwrap_or_default();
        report.notes.push(format!(
            "{} noise or repeated lines removed{by}",
            report.filtered_lines
        ));
    }
    if report.omitted_blocks > 0 {
        report.notes.push(format!(
            "{} diagnostic blocks did not fit the budget and are omitted (see the full output)",
            report.omitted_blocks
        ));
    }

    report.text = out.join("\n");
    report.changed = report.text != raw.trim_end_matches('\n');
    report
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        "line"
    } else {
        "lines"
    }
}

/// Collapse runs of lines matching the same `summarize_lines` pattern into
/// one marker line counting them.
fn summarize_runs(lines: &mut [Line], rule: &Rule) {
    if rule.summarize.is_empty() {
        return;
    }
    let mut run: Option<(usize, usize, usize)> = None; // (pattern, first, count)
    let close = |lines: &mut [Line], run: &mut Option<(usize, usize, usize)>| {
        if let Some((p, first, count)) = run.take() {
            lines[first].text =
                format!("… [{count} {}: {}]", plural(count), rule.summarize[p].label);
            lines[first].class = Class::Protected;
        }
    };
    for i in 0..lines.len() {
        if !lines[i].alive {
            continue;
        }
        let matched = if lines[i].class == Class::Rest {
            rule.summarize
                .iter()
                .position(|s| s.pattern.is_match(&lines[i].text))
        } else {
            None
        };
        match (matched, run) {
            (Some(p), Some((rp, first, count))) if p == rp => {
                lines[i].alive = false;
                run = Some((rp, first, count + 1));
            }
            (Some(p), _) => {
                close(lines, &mut run);
                run = Some((p, i, 1));
            }
            (None, _) => close(lines, &mut run),
        }
    }
    close(lines, &mut run);
}

fn select(
    lines: &mut [Line],
    blocks: &[errors::Block],
    rule: Option<&Rule>,
    limits: &LogLimits,
    report: &mut LogReport,
) {
    for l in lines.iter_mut() {
        if l.alive && l.class == Class::Fixed {
            l.keep = true;
        }
    }

    // Diagnostics: errors first, then warnings, block by block.
    let mut budget = limits.max_error_lines;
    for severity in [Severity::Error, Severity::Warning] {
        for (bi, b) in blocks.iter().enumerate() {
            if b.severity != severity {
                continue;
            }
            let idx: Vec<usize> = (0..lines.len())
                .filter(|&i| {
                    lines[i].alive && lines[i].block == Some(bi) && lines[i].class != Class::Fixed
                })
                .collect();
            if idx.is_empty() {
                continue;
            }
            let want = idx.len().min(limits.max_block_lines);
            let take = want.min(budget);
            if take == 0 || (take < want && take < 8) {
                report.omitted_blocks += 1;
                continue;
            }
            // Head and tail of a long block: the cause and the location.
            let head = if take == idx.len() {
                take
            } else {
                (take * 2 / 3).max(1)
            };
            let tail = take - head;
            for &i in idx.iter().take(head) {
                lines[i].keep = true;
            }
            for &i in idx.iter().rev().take(tail) {
                lines[i].keep = true;
            }
            budget -= take;
        }
    }

    // Everything else shares the ordinary budget, protected lines first.
    let max = rule
        .and_then(|r| r.max_lines)
        .unwrap_or(limits.max_lines)
        .min(limits.max_lines);
    let protected: Vec<usize> = (0..lines.len())
        .filter(|&i| lines[i].alive && !lines[i].keep && lines[i].class == Class::Protected)
        .collect();
    let mut remaining = max;
    keep_head_tail(
        lines,
        &protected,
        remaining,
        remaining / 2,
        remaining - remaining / 2,
    );
    remaining -= protected.len().min(remaining);

    let rest: Vec<usize> = (0..lines.len())
        .filter(|&i| lines[i].alive && !lines[i].keep && lines[i].class <= Class::Warning)
        .collect();
    let (h, t) = match rule.map(|r| (r.head_lines, r.tail_lines)) {
        Some((Some(h), Some(t))) => (h, t),
        Some((Some(h), None)) => (h, remaining.saturating_sub(h)),
        Some((None, Some(t))) => (remaining.saturating_sub(t), t),
        _ => (remaining / 3, remaining - remaining / 3),
    };
    let (h, t) = if h + t > remaining && h + t > 0 {
        let h2 = remaining * h / (h + t);
        (h2, remaining - h2)
    } else {
        (h, t)
    };
    keep_head_tail(lines, &rest, h + t, h, t);

    for l in lines.iter() {
        if l.alive && !l.keep {
            report.omitted_lines += 1 + l.repeat;
            if l.class >= Class::Warning {
                report.omitted_diagnostic_lines += 1 + l.repeat;
            }
        }
    }
}

fn keep_head_tail(lines: &mut [Line], idx: &[usize], cap: usize, head: usize, tail: usize) {
    if idx.len() <= cap {
        for &i in idx {
            lines[i].keep = true;
        }
        return;
    }
    for &i in idx.iter().take(head) {
        lines[i].keep = true;
    }
    for &i in idx.iter().rev().take(tail) {
        lines[i].keep = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::RuleSet;

    fn run(raw: &str, rule: Option<&str>) -> LogReport {
        let set = RuleSet::builtin();
        let r = rule.map(|id| set.get(id).unwrap().clone());
        compress_log(raw, r.as_ref(), &LogLimits::default())
    }

    #[test]
    fn an_error_in_the_middle_of_a_long_log_survives() {
        let mut raw = String::new();
        for i in 0..2000 {
            raw.push_str(&format!("step {i}: building module {i}\n"));
            if i == 1000 {
                raw.push_str(
                    "error[E0425]: cannot find value `x` in this scope\n  --> src/a.rs:3:5\n\n",
                );
            }
        }
        let r = run(&raw, Some("generic"));
        assert!(
            r.text.contains("error[E0425]: cannot find value `x`"),
            "{}",
            r.text
        );
        assert!(r.text.contains("--> src/a.rs:3:5"));
        assert!(r.text.contains("lines omitted"));
        assert!(r.text.lines().count() < 400);
        assert_eq!(r.error_blocks, 1);
    }

    #[test]
    fn distinct_diagnostics_are_never_merged_but_exact_repeats_are() {
        let raw = "warning: a\nwarning: b\nsame\nsame\nsame\nwarning: a\n";
        let r = run(raw, Some("generic"));
        assert_eq!(r.text.matches("warning: a").count(), 2, "{}", r.text);
        assert!(r.text.contains("warning: b"));
        assert!(r.text.contains("same  [repeated 3×]"), "{}", r.text);
    }

    #[test]
    fn exit_code_and_stderr_marker_are_kept() {
        let raw = format!(
            "Exit code 2\n{}\n{STDERR_MARKER}\nboom\n",
            "noise\n".repeat(900)
        );
        let r = run(&raw, Some("generic"));
        assert!(r.text.starts_with("Exit code 2"));
        assert!(r.text.contains(STDERR_MARKER));
        assert_eq!(r.exit_code, Some(2));
    }

    #[test]
    fn rules_never_remove_diagnostics() {
        // A rule that strips every line still keeps the error.
        let set = RuleSet::from_texts(
            &[(
                "x.toml",
                "schema_version = 1\n[filters.all]\nmatch = [{ program = \"x\" }]\nstrip_lines_matching = ['.*']\n",
            )],
            None,
        );
        let raw = "far 1\nfar 2\nnear\nnear\nerror: disk full\nnear\nnear\nfar 4\nfar 5";
        let r = compress_log(raw, set.get("all"), &LogLimits::default());
        // The error and its context lines survive; the rule removes the rest.
        assert!(r.text.contains("error: disk full"), "{}", r.text);
        assert!(!r.text.contains("far"), "{}", r.text);
    }

    #[test]
    fn diagnostics_beyond_the_budget_are_reported_not_hidden() {
        let raw: String = (0..400)
            .map(|i| format!("error: problem {i}\n\n"))
            .collect();
        let limits = LogLimits {
            max_error_lines: 50,
            ..LogLimits::default()
        };
        let r = compress_log(&raw, None, &limits);
        assert!(r.omitted_blocks > 0);
        assert!(r
            .notes
            .iter()
            .any(|n| n.contains("diagnostic blocks did not fit")));
        assert!(
            r.text.contains("diagnostic]"),
            "omission must be visible in place"
        );
    }

    #[test]
    fn progress_and_ansi_are_cleaned_unicode_kept() {
        let raw = "\x1b[1mTélécharge\x1b[0m 10%\rTélécharge 100%\nrésultat: ✓ terminé\n";
        let r = run(raw, Some("generic"));
        assert_eq!(r.text, "Télécharge 100%\nrésultat: ✓ terminé");
    }

    #[test]
    fn match_output_never_hides_a_failure() {
        let set = RuleSet::from_texts(
            &[(
                "x.toml",
                "schema_version = 1\n[filters.m]\nmatch = [{ program = \"m\" }]\nmatch_output = [{ pattern = 'done', message = 'all good' }]\n",
            )],
            None,
        );
        let rule = set.get("m");
        assert_eq!(
            compress_log("work\ndone", rule, &LogLimits::default()).text,
            "all good"
        );
        let failed = compress_log("Exit code 1\nwork\ndone", rule, &LogLimits::default());
        assert!(failed.text.contains("done"), "{}", failed.text);
        let errored = compress_log("error: x\ndone", rule, &LogLimits::default());
        assert!(errored.text.contains("error: x"));
    }

    #[test]
    fn stream_sections_are_kept_and_the_exit_code_comes_from_outside() {
        let set = RuleSet::from_texts(
            &[(
                "x.toml",
                "schema_version = 1\n[filters.m]\nmatch = [{ program = \"m\" }]\nstrip_lines_matching = ['^noise']\nmatch_output = [{ pattern = 'done', message = 'all good' }]\n",
            )],
            None,
        );
        let raw = "--- stdout ---\nnoise 1\ndone\n--- stderr ---\nnoise 2\n";
        let r = compress_log_with_exit(raw, set.get("m"), &LogLimits::default(), Some(2));
        // A failed run is never summarised away, and the sections survive.
        assert_eq!(r.text, "--- stdout ---\ndone\n--- stderr ---");
        assert_eq!(r.exit_code, Some(2));
        let ok = compress_log_with_exit(raw, set.get("m"), &LogLimits::default(), Some(0));
        assert_eq!(ok.text, "all good");
    }
}
