//! Recognise the commands a shell line actually runs.
//!
//! Rules are chosen from the program and sub-command that produced an output,
//! never from a substring of the command line: `echo "cargo test"` runs
//! `echo`, and `uv run pytest -q` runs `pytest`. This module tokenises a
//! command line with shell quoting, splits it into lists (`&&`, `||`, `;`)
//! and pipelines (`|`), drops environment assignments and redirections, and
//! unwraps the common wrappers (`sudo`, `env`, `time`, `timeout`, `npx`,
//! `uv run`, `python -m`, …).
//!
//! It is deliberately not a full shell parser: anything it cannot follow
//! (sub-shells, `eval`, heredocs) makes the result [`Analysis::ambiguous`], and
//! the caller then falls back to conservative handling.

/// One simple command, after wrappers and assignments were removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// Basename of the executable (`/usr/bin/cargo` → `cargo`).
    pub program: String,
    /// Arguments after the program, as written (quotes removed).
    pub args: Vec<String>,
    /// Wrappers that were peeled off, outermost first (`["uv run"]`).
    pub wrappers: Vec<String>,
}

impl Invocation {
    /// The leading positional words after the program, skipping options.
    /// `options_with_value` lists flags whose value is a separate word
    /// (`-n prod` for kubectl); `--flag=value` forms are always skipped.
    pub fn positionals(&self, options_with_value: &[String]) -> Vec<&str> {
        let mut out = Vec::new();
        let mut skip_next = false;
        for a in &self.args {
            if skip_next {
                skip_next = false;
                continue;
            }
            if a == "--" {
                break;
            }
            if a.starts_with('-') && a.len() > 1 {
                if options_with_value.iter().any(|o| o == a) {
                    skip_next = true;
                }
                continue;
            }
            // `cargo +nightly test`: a toolchain selector, not a sub-command.
            if a.starts_with('+') && self.program == "cargo" {
                continue;
            }
            out.push(a.as_str());
            if out.len() >= 4 {
                break;
            }
        }
        out
    }

    /// `program sub sub …` for display.
    pub fn display(&self) -> String {
        let pos = self.positionals(&[]);
        let mut s = self.program.clone();
        for p in pos.iter().take(2) {
            s.push(' ');
            s.push_str(p);
        }
        s
    }
}

/// A pipeline: its first stage produced the data, later stages transformed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pipeline {
    pub stages: Vec<Invocation>,
}

impl Pipeline {
    pub fn producer(&self) -> Option<&Invocation> {
        self.stages.first()
    }

    /// True when a later stage reshapes the producer's output (`| grep`,
    /// `| tail -n 20`, `| jq`), so the text is no longer in the producer's
    /// format. Pass-through stages (`tee`, `cat`) do not count.
    pub fn is_filtered(&self) -> bool {
        self.stages
            .iter()
            .skip(1)
            .any(|s| !matches!(s.program.as_str(), "tee" | "cat" | "less" | "more"))
    }
}

/// What a command line runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Analysis {
    /// Every pipeline, in order, excluding trivial built-ins (`cd`, `export`…).
    pub pipelines: Vec<Pipeline>,
    /// Set when the line used a construct this analysis does not follow.
    pub ambiguous: Option<String>,
}

impl Analysis {
    /// The single invocation whose output dominates the result, when there is
    /// exactly one significant pipeline and nothing ambiguous.
    pub fn primary(&self) -> Option<&Pipeline> {
        if self.ambiguous.is_some() || self.pipelines.len() != 1 {
            return None;
        }
        self.pipelines.first()
    }
}

/// Built-ins and commands whose own output is negligible; they do not make a
/// line "multi-command".
const TRIVIAL: &[&str] = &[
    "cd", "pushd", "popd", "export", "set", "unset", "source", ".", "true", ":", "mkdir", "sleep",
    "shopt", "ulimit", "umask", "alias", "trap", "wait",
];

pub fn analyze(line: &str) -> Analysis {
    let tokens = match tokenize(line) {
        Ok(t) => t,
        Err(why) => {
            return Analysis {
                pipelines: Vec::new(),
                ambiguous: Some(why),
            }
        }
    };
    let mut analysis = Analysis::default();
    let mut current_pipeline: Vec<Invocation> = Vec::new();
    let mut words: Vec<String> = Vec::new();

    let flush_command =
        |words: &mut Vec<String>, pipeline: &mut Vec<Invocation>, a: &mut Analysis| {
            if words.is_empty() {
                return;
            }
            match unwrap_command(std::mem::take(words)) {
                Ok(Some(inv)) => pipeline.push(inv),
                Ok(None) => {}
                Err(why) => {
                    a.ambiguous.get_or_insert(why);
                }
            }
        };
    let flush_pipeline = |pipeline: &mut Vec<Invocation>, a: &mut Analysis| {
        if pipeline.is_empty() {
            return;
        }
        let stages = std::mem::take(pipeline);
        if stages.len() == 1 && TRIVIAL.contains(&stages[0].program.as_str()) {
            return;
        }
        a.pipelines.push(Pipeline { stages });
    };

    for tok in tokens {
        match tok {
            Token::Word(w) => words.push(w),
            Token::Pipe => flush_command(&mut words, &mut current_pipeline, &mut analysis),
            Token::ListSep => {
                flush_command(&mut words, &mut current_pipeline, &mut analysis);
                flush_pipeline(&mut current_pipeline, &mut analysis);
            }
            Token::Unsupported(what) => {
                analysis.ambiguous.get_or_insert(what);
            }
        }
    }
    flush_command(&mut words, &mut current_pipeline, &mut analysis);
    flush_pipeline(&mut current_pipeline, &mut analysis);
    analysis
}

// ─── Tokeniser ───────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
enum Token {
    Word(String),
    Pipe,
    ListSep,
    Unsupported(String),
}

fn tokenize(line: &str) -> Result<Vec<Token>, String> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    let mut word = String::new();
    let mut in_word = false;
    // A redirection operator's target word must be dropped, not kept as an
    // argument (`> out.txt`).
    let mut drop_next_word = false;

    macro_rules! end_word {
        () => {
            if in_word {
                let w = std::mem::take(&mut word);
                if drop_next_word {
                    drop_next_word = false;
                } else if is_redirection(&w) {
                    // `2>&1`, `>out`, `2>/dev/null`: complete in one word.
                    if w.ends_with('>') || w.ends_with('<') {
                        drop_next_word = true;
                    }
                } else {
                    out.push(Token::Word(w));
                }
                in_word = false;
            }
        };
    }

    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => word.push(ch),
                        None => return Err("unterminated single quote".into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(ch @ ('"' | '\\' | '$' | '`')) => word.push(ch),
                            Some(ch) => {
                                word.push('\\');
                                word.push(ch);
                            }
                            None => return Err("unterminated double quote".into()),
                        },
                        Some(ch) => word.push(ch),
                        None => return Err("unterminated double quote".into()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next() {
                    Some('\n') => {}
                    Some(ch) => word.push(ch),
                    None => {}
                }
            }
            ' ' | '\t' => end_word!(),
            '\n' | ';' => {
                end_word!();
                out.push(Token::ListSep);
            }
            '&' => {
                end_word!();
                if chars.peek() == Some(&'&') {
                    chars.next();
                } else if chars.peek() == Some(&'>') {
                    // `&>file`: redirection of both streams.
                    chars.next();
                    if chars.peek() == Some(&'>') {
                        chars.next();
                    }
                    drop_next_word = true;
                    continue;
                }
                // A lone `&` backgrounds the command; either way a new list
                // item starts.
                out.push(Token::ListSep);
            }
            '|' => {
                end_word!();
                if chars.peek() == Some(&'|') {
                    chars.next();
                    out.push(Token::ListSep);
                } else {
                    if chars.peek() == Some(&'&') {
                        chars.next();
                    }
                    out.push(Token::Pipe);
                }
            }
            '>' | '<' => {
                // Part of a redirection such as `2>&1` when glued to a fd.
                let glued_fd = in_word && word.chars().all(|ch| ch.is_ascii_digit());
                if !glued_fd {
                    end_word!();
                }
                if c == '<' && chars.peek() == Some(&'<') {
                    out.push(Token::Unsupported("heredoc".into()));
                    // Ignore the rest of the line: its body is not commands.
                    return Ok(out);
                }
                // `end_word!` already cleared `in_word` unless the fd is glued.
                let mut op = if glued_fd {
                    in_word = false;
                    std::mem::take(&mut word)
                } else {
                    String::new()
                };
                op.push(c);
                while let Some(&n) = chars.peek() {
                    if n == '>' || n == '&' {
                        op.push(n);
                        chars.next();
                    } else {
                        break;
                    }
                }
                // `2>&1` / `>&2`: the target is the next characters (a digit).
                if op.ends_with('&') {
                    while let Some(&n) = chars.peek() {
                        if n.is_ascii_digit() || n == '-' {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                } else {
                    drop_next_word = true;
                }
            }
            '(' | ')' => {
                end_word!();
                out.push(Token::Unsupported("sub-shell".into()));
                out.push(Token::ListSep);
            }
            '`' => {
                in_word = true;
                word.push(c);
                out.push(Token::Unsupported("command substitution".into()));
            }
            '$' if chars.peek() == Some(&'(') => {
                // `$(...)`: keep it inside the current word, but note it.
                in_word = true;
                word.push('$');
                let mut depth = 0;
                for ch in chars.by_ref() {
                    word.push(ch);
                    match ch {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {
                in_word = true;
                word.push(c);
            }
        }
    }
    // Last word: the same rules as `end_word!`, without resetting state.
    if in_word && !drop_next_word && !is_redirection(&word) {
        out.push(Token::Word(word));
    }
    Ok(out)
}

fn is_redirection(w: &str) -> bool {
    let rest = w.trim_start_matches(|c: char| c.is_ascii_digit());
    rest.starts_with('>') || rest.starts_with('<')
}

// ─── Wrappers ────────────────────────────────────────────────────────────────

/// Strip assignments and wrappers. `Ok(None)` for an empty command.
fn unwrap_command(mut words: Vec<String>) -> Result<Option<Invocation>, String> {
    let mut wrappers = Vec::new();
    loop {
        // Leading `NAME=value` assignments.
        while words.first().is_some_and(|w| is_assignment(w)) {
            words.remove(0);
        }
        let Some(first) = words.first().cloned() else {
            return Ok(None);
        };
        let prog = basename(&first);
        let peeled = match prog.as_str() {
            "sudo" => skip_options(&words, 1, &["-u", "-g", "-h", "-p", "-C", "-D", "-r", "-t"]),
            "env" => {
                let mut i = skip_options(&words, 1, &["-u", "-C", "-S"]);
                while words.get(i).is_some_and(|w| is_assignment(w)) {
                    i += 1;
                }
                i
            }
            "time" | "nohup" | "command" | "builtin" | "exec" => skip_options(&words, 1, &[]),
            "nice" => skip_options(&words, 1, &["-n"]),
            "stdbuf" => skip_options(&words, 1, &["-i", "-o", "-e"]),
            "timeout" => {
                // timeout [options] DURATION COMMAND
                let i = skip_options(&words, 1, &["-s", "-k", "--signal", "--kill-after"]);
                i + 1
            }
            "xargs" | "watch" | "eval" | "bash" | "sh" | "zsh" => {
                // The real command is built at run time or quoted inside a
                // string: following it would be guessing.
                return Err(format!("`{prog}` runs a command this analysis cannot see"));
            }
            "npx" | "bunx" | "pnpx" | "uvx" => skip_options(
                &words,
                1,
                &[
                    "-p",
                    "--package",
                    "--from",
                    "--with",
                    "--python",
                    "-c",
                    "--call",
                ],
            ),
            "pnpm" | "yarn" if matches!(words.get(1).map(String::as_str), Some("exec" | "dlx")) => {
                skip_options(&words, 2, &["--package", "-p"])
            }
            "npm" if matches!(words.get(1).map(String::as_str), Some("exec" | "x")) => {
                let i = skip_options(
                    &words,
                    2,
                    &["--package", "-p", "-c", "--call", "-w", "--workspace"],
                );
                if words.get(i).map(String::as_str) == Some("--") {
                    i + 1
                } else {
                    i
                }
            }
            "bun" if words.get(1).map(String::as_str) == Some("x") => skip_options(&words, 2, &[]),
            "uv" if words.get(1).map(String::as_str) == Some("run") => skip_options(
                &words,
                2,
                &[
                    "--with",
                    "--with-requirements",
                    "--with-editable",
                    "-p",
                    "--python",
                    "--package",
                    "--project",
                    "--directory",
                    "--extra",
                    "--group",
                    "--env-file",
                    "--index",
                    "--index-url",
                    "--extra-index-url",
                    "-C",
                    "--config-setting",
                    "--script",
                ],
            ),
            "poetry" | "pipenv" | "pdm" | "hatch" | "rye"
                if words.get(1).map(String::as_str) == Some("run") =>
            {
                skip_options(&words, 2, &["-e", "--env"])
            }
            "python" | "python3" | "py" | "pypy3" => {
                // `python -m module args` runs `module`; a script path does not.
                let mut i = 1;
                while let Some(w) = words.get(i) {
                    if w == "-m" {
                        break;
                    }
                    if w.starts_with('-') {
                        i += 1;
                        continue;
                    }
                    break;
                }
                if words.get(i).map(String::as_str) == Some("-m") && words.len() > i + 1 {
                    i + 1
                } else {
                    0
                }
            }
            _ => 0,
        };
        if peeled == 0 || peeled >= words.len() {
            if peeled >= words.len() && peeled != 0 {
                // A bare wrapper (`time`, `env`): nothing else runs.
                words.truncate(1);
            }
            let program = basename(&words[0]);
            let args = words.split_off(1);
            return Ok(Some(Invocation {
                program,
                args,
                wrappers,
            }));
        }
        let label =
            if peeled >= 2 && !words[1].starts_with('-') && peeled_two_words(&prog, &words[1]) {
                format!("{} {}", prog, words[1])
            } else {
                prog.clone()
            };
        wrappers.push(label);
        words.drain(..peeled);
    }
}

fn peeled_two_words(prog: &str, second: &str) -> bool {
    matches!(
        (prog, second),
        ("uv", "run")
            | ("pnpm", "exec" | "dlx")
            | ("yarn", "exec" | "dlx")
            | ("npm", "exec" | "x")
            | ("bun", "x")
            | ("poetry" | "pipenv" | "pdm" | "hatch" | "rye", "run")
    )
}

/// Index of the first non-option word at or after `from`.
fn skip_options(words: &[String], from: usize, with_value: &[&str]) -> usize {
    let mut i = from;
    while let Some(w) = words.get(i) {
        if w == "--" {
            return i + 1;
        }
        if !w.starts_with('-') || w.len() == 1 {
            break;
        }
        i += if with_value.contains(&w.as_str()) {
            2
        } else {
            1
        };
    }
    i
}

fn is_assignment(w: &str) -> bool {
    match w.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.chars().next().is_some_and(|c| c.is_ascii_digit())
        }
        None => false,
    }
}

fn basename(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn primary(line: &str) -> Option<(String, Vec<String>, Vec<String>)> {
        let a = analyze(line);
        let p = a.primary()?;
        let inv = p.producer()?;
        Some((
            inv.program.clone(),
            inv.positionals(&[]).iter().map(|s| s.to_string()).collect(),
            inv.wrappers.clone(),
        ))
    }

    #[test]
    fn plain_command_and_subcommand() {
        let (p, pos, w) = primary("cargo test --workspace -- --nocapture").unwrap();
        assert_eq!(p, "cargo");
        assert_eq!(pos, vec!["test"]);
        assert!(w.is_empty());
    }

    #[test]
    fn toolchain_selector_is_not_a_subcommand() {
        assert_eq!(primary("cargo +nightly clippy").unwrap().1, vec!["clippy"]);
    }

    #[test]
    fn wrappers_are_peeled() {
        let (p, pos, w) = primary("uv run --with rich pytest -q tests/").unwrap();
        assert_eq!((p.as_str(), pos[0].as_str()), ("pytest", "tests/"));
        assert_eq!(w, vec!["uv run"]);

        let (p, _, w) = primary("sudo -u ci env RUST_LOG=debug timeout 300 cargo test").unwrap();
        assert_eq!(p, "cargo");
        assert_eq!(w, vec!["sudo", "env", "timeout"]);

        assert_eq!(primary("python3 -m pytest -x").unwrap().0, "pytest");
        assert_eq!(primary("npx --yes vitest run").unwrap().0, "vitest");
        assert_eq!(primary("pnpm exec eslint .").unwrap().0, "eslint");
        assert_eq!(primary("FOO=1 BAR=2 go test ./...").unwrap().0, "go");
        assert_eq!(
            primary("./node_modules/.bin/vitest run").unwrap().0,
            "vitest"
        );
    }

    #[test]
    fn a_script_run_by_python_is_python() {
        assert_eq!(primary("python3 manage.py test").unwrap().0, "python3");
    }

    #[test]
    fn quoted_text_is_never_a_command() {
        // A substring match would see `cargo test` here.
        let (p, _, _) = primary("echo \"cargo test failed\"").unwrap();
        assert_eq!(p, "echo");
        let (p, _, _) = primary("git commit -m 'fix: cargo test && go test'").unwrap();
        assert_eq!(p, "git");
    }

    #[test]
    fn trivial_prefixes_do_not_count() {
        let (p, pos, _) = primary("cd crates/x && cargo test 2>&1").unwrap();
        assert_eq!((p.as_str(), pos[0].as_str()), ("cargo", "test"));
    }

    #[test]
    fn redirections_are_not_arguments() {
        let (_, pos, _) = primary("go test ./... > out.txt 2>&1").unwrap();
        assert_eq!(pos, vec!["test", "./..."]);
        let a = analyze("cargo build &> build.log");
        assert_eq!(a.primary().unwrap().producer().unwrap().args, vec!["build"]);
    }

    #[test]
    fn pipelines_record_filters() {
        let a = analyze("cargo test 2>&1 | tail -n 50");
        let p = a.primary().unwrap();
        assert_eq!(p.producer().unwrap().program, "cargo");
        assert!(p.is_filtered());
        let a = analyze("cargo test 2>&1 | tee log.txt");
        assert!(!a.primary().unwrap().is_filtered());
    }

    #[test]
    fn several_commands_have_no_primary() {
        let a = analyze("cargo build && cargo test");
        assert_eq!(a.pipelines.len(), 2);
        assert!(a.primary().is_none());
    }

    #[test]
    fn opaque_constructs_are_ambiguous() {
        assert!(analyze("bash -c 'cargo test'").ambiguous.is_some());
        assert!(analyze("(cd x; make)").ambiguous.is_some());
        assert!(analyze("cat <<EOF\ncargo test\nEOF").ambiguous.is_some());
        assert!(analyze("echo 'unterminated").ambiguous.is_some());
    }

    #[test]
    fn options_with_values_are_skipped() {
        let a = analyze("kubectl -n prod get pods");
        let inv = a.primary().unwrap().producer().unwrap();
        assert_eq!(inv.positionals(&["-n".to_string()]), vec!["get", "pods"]);
        let a = analyze("docker compose -f dev.yml up -d");
        let inv = a.primary().unwrap().producer().unwrap();
        assert_eq!(inv.positionals(&["-f".to_string()]), vec!["compose", "up"]);
    }

    #[test]
    fn unicode_arguments_survive() {
        let (_, pos, _) = primary("grep -r 'clé' données/").unwrap();
        assert_eq!(pos, vec!["clé", "données/"]);
    }
}
