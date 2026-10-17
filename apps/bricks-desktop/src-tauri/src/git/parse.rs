//! Pure parsers for git's machine-readable output. No I/O: unit-tested here.

use crate::contract::{GitArea, GitCommit};

/// Branch information from the `# branch.*` headers of porcelain v2.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Header {
    pub branch: Option<String>,
    pub detached: bool,
    pub head: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

/// One path in one area, before counts are attached. Paths are relative
/// to the repository root, as git prints them. (`GitArea` has no
/// `PartialEq`, so tests match on it.)
#[derive(Debug)]
pub(super) struct RawEntry {
    pub path: String,
    pub orig_path: Option<String>,
    pub area: GitArea,
    pub code: String,
}

#[derive(Debug)]
pub(super) struct StatusOutput {
    pub header: Header,
    pub entries: Vec<RawEntry>,
}

/// Line counts of one path. `binary` paths have no counts.
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub(super) struct Counts {
    pub added: Option<u32>,
    pub removed: Option<u32>,
    pub binary: bool,
}

/// Parse `git status --porcelain=v2 -z --branch`.
///
/// With `-z`, every record ends with NUL and a rename record carries its
/// original path as the following NUL-terminated token.
pub(super) fn parse_status_v2(input: &[u8]) -> StatusOutput {
    let mut header = Header::default();
    let mut entries = Vec::new();
    let mut tokens = input.split(|b| *b == 0).filter(|t| !t.is_empty());

    while let Some(token) = tokens.next() {
        let line = String::from_utf8_lossy(token).into_owned();
        if let Some(rest) = line.strip_prefix("# ") {
            parse_header_line(rest, &mut header);
            continue;
        }
        match line.as_bytes().first() {
            // Ordinary change: `1 XY sub mH mI mW hH hI path`.
            Some(b'1') => {
                let fields: Vec<&str> = line.splitn(9, ' ').collect();
                if fields.len() == 9 {
                    push_xy(fields[1], fields[8], None, &mut entries);
                }
            }
            // Rename or copy: `2 XY sub mH mI mW hH hI score path`, then origPath.
            Some(b'2') => {
                let fields: Vec<&str> = line.splitn(10, ' ').collect();
                let orig = tokens.next().map(|t| String::from_utf8_lossy(t).into_owned());
                if fields.len() == 10 {
                    push_xy(fields[1], fields[9], orig, &mut entries);
                }
            }
            // Unmerged: `u XY sub m1 m2 m3 mW h1 h2 h3 path`.
            Some(b'u') => {
                let fields: Vec<&str> = line.splitn(11, ' ').collect();
                if fields.len() == 11 {
                    entries.push(RawEntry {
                        path: fields[10].to_string(),
                        orig_path: None,
                        area: GitArea::Conflicted,
                        code: "U".into(),
                    });
                }
            }
            // Untracked: `? path`.
            Some(b'?') => {
                if let Some(path) = line.strip_prefix("? ") {
                    entries.push(RawEntry {
                        path: path.to_string(),
                        orig_path: None,
                        area: GitArea::Untracked,
                        code: "?".into(),
                    });
                }
            }
            // Ignored entries are not requested; anything else is skipped.
            _ => {}
        }
    }

    StatusOutput { header, entries }
}

fn parse_header_line(rest: &str, header: &mut Header) {
    if let Some(oid) = rest.strip_prefix("branch.oid ") {
        if oid != "(initial)" {
            header.head = Some(oid.chars().take(7).collect());
        }
    } else if let Some(head) = rest.strip_prefix("branch.head ") {
        if head == "(detached)" {
            header.detached = true;
        } else {
            header.branch = Some(head.to_string());
        }
    } else if let Some(ab) = rest.strip_prefix("branch.ab ") {
        let mut parts = ab.split_whitespace();
        header.ahead = parts
            .next()
            .and_then(|s| s.strip_prefix('+'))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        header.behind = parts
            .next()
            .and_then(|s| s.strip_prefix('-'))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
    }
}

/// `XY` is index status then worktree status; `.` means unchanged.
fn push_xy(xy: &str, path: &str, orig: Option<String>, entries: &mut Vec<RawEntry>) {
    let mut chars = xy.chars();
    let x = chars.next().unwrap_or('.');
    let y = chars.next().unwrap_or('.');
    if x != '.' {
        entries.push(RawEntry {
            path: path.to_string(),
            orig_path: orig,
            area: GitArea::Staged,
            code: x.to_string(),
        });
    }
    if y != '.' {
        entries.push(RawEntry {
            path: path.to_string(),
            orig_path: None,
            area: GitArea::Unstaged,
            code: y.to_string(),
        });
    }
}

/// Parse `git diff --numstat -z`. Returns `(path, counts)` keyed by the new
/// path. Rename records are `A\tD\t\0old\0new\0`; others `A\tD\tpath\0`.
pub(super) fn parse_numstat_z(input: &[u8]) -> Vec<(String, Counts)> {
    let mut out = Vec::new();
    let mut tokens = input.split(|b| *b == 0).filter(|t| !t.is_empty());
    while let Some(token) = tokens.next() {
        let text = String::from_utf8_lossy(token).into_owned();
        let mut parts = text.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let path = if path.is_empty() {
            let _old = tokens.next();
            match tokens.next() {
                Some(new) => String::from_utf8_lossy(new).into_owned(),
                None => break,
            }
        } else {
            path.to_string()
        };
        out.push((path, counts(added, removed)));
    }
    out
}

fn counts(added: &str, removed: &str) -> Counts {
    if added == "-" || removed == "-" {
        return Counts { added: None, removed: None, binary: true };
    }
    Counts {
        added: added.parse().ok(),
        removed: removed.parse().ok(),
        binary: false,
    }
}

/// Parse `git log -z --format=%H%x00%h%x00%an%x00%at%x00%s`. Fields are
/// NUL-separated and so are records: a fixed run of five tokens per commit.
pub(super) fn parse_log_z(input: &[u8]) -> Vec<GitCommit> {
    let mut fields: Vec<String> = input
        .split(|b| *b == 0)
        .map(|t| String::from_utf8_lossy(t).into_owned())
        .collect();
    // Output ends with a separator, which leaves one empty trailing token.
    if fields.last().is_some_and(|s| s.is_empty()) {
        fields.pop();
    }
    fields
        .chunks_exact(5)
        .map(|c| GitCommit {
            hash: c[0].clone(),
            short: c[1].clone(),
            author: c[2].clone(),
            at: c[3].parse().unwrap_or(0),
            subject: c[4].clone(),
        })
        .collect()
}

/// One record of `--name-status -z`.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct NameStatus {
    /// Status letter: M A D R C T U ...
    pub code: char,
    /// Source path of a rename or copy.
    pub orig: Option<String>,
    /// Path after the change.
    pub path: String,
}

/// Parse `git diff --name-status -z` or `git show --name-status -z`. Renames
/// and copies carry two paths: the source, then the destination.
pub(super) fn parse_name_status_z(input: &[u8]) -> Vec<NameStatus> {
    let mut out = Vec::new();
    let mut tokens = input
        .split(|b| *b == 0)
        .filter(|t| !t.is_empty())
        .map(|t| String::from_utf8_lossy(t).into_owned());
    while let Some(status) = tokens.next() {
        let code = status.chars().next().unwrap_or('?');
        let (orig, path) = if matches!(code, 'R' | 'C') {
            let orig = tokens.next();
            (orig, tokens.next())
        } else {
            (None, tokens.next())
        };
        match path {
            Some(path) => out.push(NameStatus { code, orig, path }),
            None => break,
        }
    }
    out
}

/// Split a patch into one section per file, at each `diff --git` header.
/// Content lines always start with `+`, `-` or space, so a header can only
/// begin a line.
pub(super) fn split_patch_sections(patch: &str) -> Vec<&str> {
    let mut starts = Vec::new();
    if patch.starts_with("diff --git ") {
        starts.push(0);
    }
    for (index, _) in patch.match_indices("\ndiff --git ") {
        starts.push(index + 1);
    }
    let mut sections = Vec::with_capacity(starts.len());
    for (k, &start) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(patch.len());
        sections.push(&patch[start..end]);
    }
    sections
}

/// Path from a `diff --git a/X b/Y` header, used only when name-status
/// and the patch disagree.
pub(super) fn header_path(section: &str) -> String {
    let first = section.lines().next().unwrap_or("");
    let Some(rest) = first.strip_prefix("diff --git a/") else {
        return String::new();
    };
    match rest.rfind(" b/") {
        Some(index) => rest[index + 3..].to_string(),
        None => String::new(),
    }
}

/// Whether a patch describes a binary file (git prints no hunks for it).
pub(super) fn is_binary_patch(patch: &str) -> bool {
    patch
        .lines()
        .any(|l| l.starts_with("Binary files ") || l == "GIT binary patch")
}

/// Git's own heuristic: a NUL byte in the first 8000 bytes.
pub(super) fn is_binary_content(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|b| *b == 0)
}

/// Number of lines, counting a final line without newline.
pub(super) fn count_lines(bytes: &[u8]) -> u32 {
    let newlines = bytes.iter().filter(|b| **b == b'\n').count();
    let partial = !bytes.is_empty() && bytes.last() != Some(&b'\n');
    (newlines + usize::from(partial)).min(u32::MAX as usize) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_v2_parses_branch_headers() {
        let input = b"# branch.oid 6c85919bb199e64d2ff44d32ca73e8d3fddf4929\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0";
        let out = parse_status_v2(input);
        assert_eq!(out.header.branch.as_deref(), Some("main"));
        assert!(!out.header.detached);
        assert_eq!(out.header.head.as_deref(), Some("6c85919"));
        assert_eq!((out.header.ahead, out.header.behind), (2, 1));
        assert!(out.entries.is_empty());
    }

    #[test]
    fn status_v2_detached_and_initial() {
        let detached = parse_status_v2(b"# branch.oid abcdef1234\0# branch.head (detached)\0");
        assert!(detached.header.detached);
        assert_eq!(detached.header.branch, None);

        let initial = parse_status_v2(b"# branch.oid (initial)\0# branch.head main\0");
        assert_eq!(initial.header.head, None);
        assert_eq!(initial.header.branch.as_deref(), Some("main"));
    }

    #[test]
    fn status_v2_splits_index_and_worktree_changes() {
        // Staged add, then modified again in the worktree.
        let input = b"1 AM N... 000000 100644 100644 0000 1111 a.txt\0";
        let out = parse_status_v2(input);
        assert_eq!(out.entries.len(), 2);
        assert!(matches!(out.entries[0].area, GitArea::Staged));
        assert_eq!(out.entries[0].code, "A");
        assert!(matches!(out.entries[1].area, GitArea::Unstaged));
        assert_eq!(out.entries[1].code, "M");
        assert_eq!(out.entries[1].path, "a.txt");
    }

    #[test]
    fn status_v2_rename_takes_orig_path_token() {
        let input = b"2 R. N... 100644 100644 100644 aaaa bbbb R100 new dir/name with space.txt\0old dir/name.txt\0? after.txt\0";
        let out = parse_status_v2(input);
        assert_eq!(out.entries.len(), 2);
        assert_eq!(out.entries[0].path, "new dir/name with space.txt");
        assert_eq!(out.entries[0].orig_path.as_deref(), Some("old dir/name.txt"));
        assert_eq!(out.entries[0].code, "R");
        assert!(matches!(out.entries[1].area, GitArea::Untracked));
        assert_eq!(out.entries[1].path, "after.txt");
    }

    #[test]
    fn status_v2_unmerged_is_conflicted() {
        let input = "u UU N... 100644 100644 100644 100644 a b c e/é.txt\0".as_bytes();
        let out = parse_status_v2(input);
        assert_eq!(out.entries.len(), 1);
        assert!(matches!(out.entries[0].area, GitArea::Conflicted));
        assert_eq!(out.entries[0].code, "U");
        assert_eq!(out.entries[0].path, "e/é.txt");
    }

    #[test]
    fn numstat_parses_plain_binary_and_rename_records() {
        // Plain record, binary record, then a rename (empty path column).
        let input: &[u8] = b"3\t1\tsrc/a.rs\0-\t-\tdata.bin\x000\t0\t\0old.txt\0new.txt\0";
        let parsed = parse_numstat_z(input);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].0, "src/a.rs");
        assert_eq!(parsed[0].1, Counts { added: Some(3), removed: Some(1), binary: false });
        assert_eq!(parsed[1].0, "data.bin");
        assert_eq!(parsed[1].1, Counts { added: None, removed: None, binary: true });
        assert_eq!(parsed[2].0, "new.txt");
        assert_eq!(parsed[2].1.added, Some(0));
    }

    #[test]
    fn log_parses_fixed_field_records() {
        let input = b"aaa\0aa\0Ada Lovelace\01700000000\0first commit\0bbb\0bb\0Grace\01700000100\0\0";
        let commits = parse_log_z(input);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].short, "aa");
        assert_eq!(commits[0].author, "Ada Lovelace");
        assert_eq!(commits[0].at, 1700000000);
        assert_eq!(commits[1].subject, "");
    }

    #[test]
    fn name_status_keeps_rename_source_and_destination() {
        let input = b"M\0a.txt\0R100\0old.txt\0new name.txt\0D\0gone.txt\0";
        let parsed = parse_name_status_z(input);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0], NameStatus { code: 'M', orig: None, path: "a.txt".into() });
        assert_eq!(
            parsed[1],
            NameStatus { code: 'R', orig: Some("old.txt".into()), path: "new name.txt".into() }
        );
        assert_eq!(parsed[2].path, "gone.txt");
    }

    #[test]
    fn patch_sections_split_on_headers_only() {
        let patch = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n\ndiff --git a/y b/y\nnew file mode 100644\n";
        let sections = split_patch_sections(patch);
        assert_eq!(sections.len(), 2);
        assert!(sections[0].starts_with("diff --git a/x"));
        assert!(sections[1].starts_with("diff --git a/y"));
        assert_eq!(header_path(sections[1]), "y");
    }

    #[test]
    fn binary_markers_are_detected() {
        assert!(is_binary_patch("diff --git a/x b/x\nBinary files a/x and b/x differ\n"));
        assert!(!is_binary_patch("diff --git a/x b/x\n+Binary files are fun\n"));
        assert!(is_binary_content(b"ab\0cd"));
        assert!(!is_binary_content("héllo\n".as_bytes()));
    }

    #[test]
    fn line_count_includes_unterminated_last_line() {
        assert_eq!(count_lines(b""), 0);
        assert_eq!(count_lines(b"a\nb\n"), 2);
        assert_eq!(count_lines(b"a\nb"), 2);
    }
}
