//! Opening a project (`--workspace`, alias `--cd`), resuming a session of
//! another project with that project's settings, and the history filtered
//! by project — through the real binary, a local model server, no key.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// One Bricks home (one session store) and several project folders.
struct Home {
    base: Project,
    root: tempfile::TempDir,
}

impl Home {
    fn new(url: &str) -> Self {
        Home {
            base: Project::new(url, ""),
            root: tempfile::tempdir().unwrap(),
        }
    }

    /// A project folder under the root, with its own bricks.toml.
    fn project(&self, name: &str, toml: &str) -> PathBuf {
        let d = self.root.path().join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("bricks.toml"),
            format!("[agent]\nmodel = \"local/m\"\n\n{toml}"),
        )
        .unwrap();
        d.canonicalize().unwrap()
    }

    /// `bricks args…` started from `from`.
    fn run(&self, from: &Path, args: &[&str]) -> Output {
        let mut c: Command = self.base.command(args);
        c.current_dir(from)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c.output().unwrap()
    }

    fn sessions_dir(&self) -> PathBuf {
        self.base.sessions_dir()
    }
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn opened(o: &Output) -> Value {
    envelopes(&o.stdout)
        .into_iter()
        .find(|e| e["type"] == "session_opened")
        .unwrap_or_else(|| panic!("no session_opened: {}", err(o)))
}

fn session_ids(o: &Output) -> Vec<String> {
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect()
}

#[test]
fn the_workspace_option_opens_a_project_from_anywhere() {
    let m = model(vec![]);
    let h = Home::new(&m.url);
    let a = h.project("Mon projet", "");
    let launch = h.root.path().join("ailleurs");
    std::fs::create_dir(&launch).unwrap();

    // Absolute, before the command.
    let o = h.run(
        &launch,
        &["--workspace", a.to_str().unwrap(), "run", "--json", "x"],
    );
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert_eq!(opened(&o)["working_dir"], a.display().to_string());
    // Relative (from the launch folder), alias, after the command.
    let o = h.run(&launch, &["run", "--json", "--cd", "../Mon projet", "x"]);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert_eq!(opened(&o)["working_dir"], a.display().to_string());
    // Without the option: the launch folder (its bricks.toml is absent, so
    // the model must be given).
    let o = h.run(&a, &["run", "--json", "x"]);
    assert_eq!(opened(&o)["working_dir"], a.display().to_string());

    // A missing folder or a file: refused before anything, nothing stored.
    let before = std::fs::read_dir(h.sessions_dir()).unwrap().count();
    let calls = m.seen.lock().unwrap().len();
    for bad in ["absent", "Mon projet/bricks.toml"] {
        let o = h.run(h.root.path(), &["--workspace", bad, "run", "--json", "x"]);
        assert_eq!(o.status.code(), Some(2), "{bad}");
        assert!(o.stdout.is_empty(), "no event for {bad}");
        assert!(
            err(&o).contains("does not exist") || err(&o).contains("is a file"),
            "{}",
            err(&o)
        );
    }
    assert!(!h.root.path().join("absent").exists(), "never created");
    assert_eq!(std::fs::read_dir(h.sessions_dir()).unwrap().count(), before);
    assert_eq!(m.seen.lock().unwrap().len(), calls, "no model call");
}

#[test]
fn a_resumed_session_works_with_its_own_project() {
    let m = model(vec![
        text("B: premier"),
        // The resumed session of B writes a relative file.
        tool(
            "w1",
            "Write",
            json!({"file_path": "créé.txt", "content": "x"}),
        ),
        text("écrit"),
        // The same request in a new session of A.
        tool(
            "w1",
            "Write",
            json!({"file_path": "créé.txt", "content": "x"}),
        ),
    ]);
    let h = Home::new(&m.url);
    // A refuses writes without a person; B allows them. A's long-term
    // memory section is broken: a new session of A cannot open.
    let a = h.project("A", "[memory]\nenabled = \"oui\"\n");
    let b = h.project("B", "[permissions]\nwrite = \"allow\"\n");

    let o = h.run(&b, &["run", "--json", "premier"]);
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let id_b = opened(&o)["session_id"].as_str().unwrap().to_string();

    // From A, explicitly: B's folder, B's rules; A's settings never read.
    let o = h.run(
        &a,
        &[
            "run",
            "--json",
            "--non-interactive",
            "--session",
            &id_b,
            "écris",
        ],
    );
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let ev = opened(&o);
    assert_eq!(ev["working_dir"], b.display().to_string());
    assert_eq!(ev["resumed"], true);
    assert!(b.join("créé.txt").exists(), "relative to B");
    assert!(!a.join("créé.txt").exists());

    // A new session of A: A's (broken) project is the one read.
    let o = h.run(&a, &["run", "--json", "--non-interactive", "écris"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(err(&o).contains("bricks.toml"), "{}", err(&o));
    // Fixed, A refuses the write that B allowed.
    std::fs::write(a.join("bricks.toml"), "[agent]\nmodel = \"local/m\"\n").unwrap();
    let o = h.run(&a, &["run", "--json", "--non-interactive", "écris"]);
    assert_eq!(o.status.code(), Some(3), "{}", err(&o));
    assert!(!a.join("créé.txt").exists());

    // B's folder is gone: refused, nothing resumed elsewhere.
    std::fs::remove_dir_all(&b).unwrap();
    let o = h.run(&a, &["run", "--json", "--session", &id_b, "encore"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(err(&o).contains("no longer exists"), "{}", err(&o));
    assert!(o.stdout.is_empty());
}

#[cfg(unix)]
#[test]
fn the_history_is_the_projects_unless_all_is_asked() {
    let m = model(vec![]);
    let h = Home::new(&m.url);
    let atlas = h.project("atlas", "");
    let old = h.project("atlas-old", "");
    let sub = atlas.join("sub");
    std::fs::create_dir(&sub).unwrap();
    std::os::unix::fs::symlink(&atlas, h.root.path().join("alias")).unwrap();
    let new_session = |dir: &Path| {
        let o = h.run(dir, &["run", "--json", "x"]);
        assert_eq!(o.status.code(), Some(0), "{}", err(&o));
        opened(&o)["session_id"].as_str().unwrap().to_string()
    };
    let s_atlas = new_session(&atlas);
    let s_old = new_session(&old);
    // A worktree of atlas is another workspace.
    let git = |args: &[&str]| {
        let o = Command::new("git")
            .args(args)
            .current_dir(&atlas)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["add", "bricks.toml"]);
    git(&["commit", "-q", "-m", "init"]);
    let wt = h.root.path().join("atlas-wt");
    git(&["worktree", "add", "-q", wt.to_str().unwrap()]);
    let s_wt = new_session(&wt);
    // An older session without settings, and an internal file.
    let store = h.sessions_dir();
    std::fs::write(
        store.join("ancienne.jsonl"),
        "{\"role\":\"user\",\"content\":\"hi\"}\n",
    )
    .unwrap();
    std::fs::write(store.join(format!("{s_atlas}.compaction-1.jsonl")), "{}\n").unwrap();
    let snapshot = |d: &Path| {
        let mut v: Vec<(String, u64)> = std::fs::read_dir(d)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (
                    e.file_name().to_string_lossy().into_owned(),
                    e.metadata().unwrap().len(),
                )
            })
            .collect();
        v.sort();
        v
    };
    let before = snapshot(&store);

    let list = |from: &Path, args: &[&str]| {
        let o = h.run(from, args);
        assert_eq!(o.status.code(), Some(0), "{}", err(&o));
        o
    };
    // The project's own, by path identity (the alias too).
    assert_eq!(
        session_ids(&list(&atlas, &["sessions", "--json"])),
        vec![s_atlas.clone()]
    );
    assert_eq!(
        session_ids(&list(
            h.root.path(),
            &["--workspace", "alias", "sessions", "--json"]
        )),
        vec![s_atlas.clone()]
    );
    assert_eq!(
        session_ids(&list(&old, &["sessions", "--json"])),
        vec![s_old.clone()]
    );
    assert_eq!(
        session_ids(&list(&wt, &["sessions", "--json"])),
        vec![s_wt.clone()]
    );
    // A sub-folder is not its parent's project.
    let o = list(&sub, &["sessions", "--json"]);
    assert!(o.stdout.is_empty(), "zero object, no prose");
    let o = list(&sub, &["sessions"]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("--all"));
    // Text and JSON select the same sessions.
    let text = String::from_utf8_lossy(&list(&atlas, &["sessions"]).stdout).into_owned();
    assert!(
        text.contains(&s_atlas) && !text.contains(&s_old) && !text.contains(&s_wt),
        "{text}"
    );
    // Everything with --all: the older session included, the internal file not.
    let all = session_ids(&list(&atlas, &["sessions", "--all", "--json"]));
    for id in [&s_atlas, &s_old, &s_wt, &"ancienne".to_string()] {
        assert!(all.contains(id), "{id} in {all:?}");
    }
    assert!(!all.iter().any(|i| i.contains("compaction")), "{all:?}");
    let text = String::from_utf8_lossy(&list(&atlas, &["sessions", "--all"]).stdout).into_owned();
    assert!(text.contains("no folder recorded"), "{text}");

    // Without any provider configured: listing still works.
    std::fs::remove_file(h.base.home.path().join("providers.toml")).unwrap();
    assert_eq!(
        session_ids(&list(&atlas, &["sessions", "--json"])),
        vec![s_atlas.clone()]
    );
    // A deleted project shows in --all, said so.
    std::fs::remove_dir_all(&old).unwrap();
    let text = String::from_utf8_lossy(&list(&atlas, &["sessions", "--all"]).stdout).into_owned();
    assert!(text.contains("folder no longer found"), "{text}");
    assert_eq!(snapshot(&store), before, "listing changes nothing");
}

#[test]
fn a_repeated_workspace_is_refused_explicitly() {
    let m = model(vec![]);
    let h = Home::new(&m.url);
    let a = h.project("A", "");
    let o = h.run(&a, &["--workspace", ".", "--cd", ".", "sessions"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        err(&o).contains("cannot be used multiple times"),
        "{}",
        err(&o)
    );
    let o = h.run(&a, &["--help"]);
    let help = String::from_utf8_lossy(&o.stdout);
    assert!(
        help.contains("--workspace <DIR>") && help.contains("[alias: --cd]"),
        "{help}"
    );
    assert!(help.contains("Default: the current folder"), "{help}");
    assert!(help.contains("--hyperlinks"), "{help}");
}

#[test]
fn headless_output_never_carries_terminal_links() {
    let link = "Voir [la doc](https://example.com/doc).";
    let m = model(vec![text(link), text(link)]);
    let h = Home::new(&m.url);
    let a = h.project("A", "");
    for args in [
        &["--hyperlinks", "always", "run", "--json", "x"][..],
        &["--hyperlinks", "always", "run", "x"][..],
    ] {
        let o = h.run(&a, args);
        assert_eq!(o.status.code(), Some(0), "{}", err(&o));
        let out = String::from_utf8_lossy(&o.stdout);
        assert!(!out.contains("\x1b]8"), "{out:?}");
        assert!(
            out.contains("https://example.com/doc"),
            "the Markdown is kept: {out}"
        );
    }
}
