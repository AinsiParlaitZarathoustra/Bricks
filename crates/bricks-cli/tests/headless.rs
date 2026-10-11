//! `bricks run` and `bricks sessions`, as a script would use them: no
//! terminal, piped stdin, JSONL on stdout, exit codes.

mod common;

use common::*;
use serde_json::json;

fn kinds(evs: &[serde_json::Value]) -> Vec<String> {
    evs.iter()
        .map(|e| e["type"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn json_output_is_complete_versioned_and_alone_on_stdout() {
    let m = model(vec![text("Bonjour 👋, réponse")]);
    let p = Project::new(&m.url, "");
    let out = p.run(&["run", "--json", "--non-interactive", "Dis bonjour"], None);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let evs = envelopes(&out.stdout);
    let k = kinds(&evs);
    assert_eq!(k.first().map(String::as_str), Some("session_opened"));
    assert_eq!(
        k.iter().filter(|k| *k == "run_finished").count(),
        1,
        "{k:?}"
    );
    for (i, e) in evs.iter().enumerate() {
        assert_eq!(e["schema"], 5);
        assert_eq!(e["seq"], i as u64 + 1, "contiguous");
        assert!(e["session_id"].is_string());
    }
    let fin = evs.iter().find(|e| e["type"] == "run_finished").unwrap();
    assert_eq!(fin["outcome"], "succeeded");
    let streamed: String = evs
        .iter()
        .filter(|e| e["type"] == "text_delta")
        .map(|e| e["text"].as_str().unwrap().to_string())
        .collect();
    assert!(streamed.contains("Bonjour 👋, réponse"), "{streamed}");
    // No secret-shaped field anywhere.
    let all = String::from_utf8_lossy(&out.stdout).to_lowercase();
    assert!(!all.contains("authorization") && !all.contains("api_key"));

    // The session is stored and listed.
    let listed = p.run(&["sessions", "--json"], None);
    assert_eq!(listed.status.code(), Some(0));
    let sessions = envelopes(&listed.stdout);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["id"], evs[0]["session_id"]);
    assert_eq!(sessions[0]["title"], "Dis bonjour");
}

/// A run stopped before a final answer (here: no progress, the same
/// failing call again and again) is reported as incomplete, with its own
/// exit code: never as a success. A former `[agent] max_turns` is ignored
/// (turn limits were removed) and said so; it never stops the run.
#[test]
fn a_run_without_a_final_answer_is_incomplete() {
    let same = || tool("c1", "Read", json!({"file_path": "absent.txt"}));
    let m = model((0..8).map(|_| same()).collect());
    let p = Project::new(&m.url, "max_turns = 1\n");
    let out = p.run(&["run", "--json", "--non-interactive", "Lis"], None);
    assert_eq!(
        out.status.code(),
        Some(5),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let evs = envelopes(&out.stdout);
    let opened = &evs[0];
    assert!(
        opened["warnings"]
            .to_string()
            .contains("turn limits were removed"),
        "{opened}"
    );
    let fin: Vec<_> = evs.iter().filter(|e| e["type"] == "run_finished").collect();
    assert_eq!(fin.len(), 1);
    assert_eq!(fin[0]["outcome"], "incomplete");
    assert_eq!(fin[0]["termination"]["kind"], "no_progress");
    assert!(
        fin[0]["turns"].as_u64().unwrap() > 1,
        "not stopped at 1 turn"
    );
}

#[test]
fn the_prompt_can_come_from_stdin() {
    let m = model(vec![text("lu")]);
    let p = Project::new(&m.url, "");
    let out = p.run(&["run", "--non-interactive"], Some("Prompt depuis stdin\n"));
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("lu"));
    let first = &m.seen.lock().unwrap()[0];
    let body = first.to_string();
    assert!(body.contains("Prompt depuis stdin"), "{body}");

    // A positional prompt does not read stdin unless --stdin is given.
    let m = model(vec![text("ok")]);
    let p = Project::new(&m.url, "");
    let out = p.run(
        &["run", "--non-interactive", "Résume", "--stdin"],
        Some("le contenu"),
    );
    assert_eq!(out.status.code(), Some(0));
    let body = m.seen.lock().unwrap()[0].to_string();
    assert!(
        body.contains("Résume") && body.contains("le contenu"),
        "{body}"
    );
}

#[test]
fn a_needed_approval_without_a_person_stops_with_code_3() {
    let p_dir = tempfile::tempdir().unwrap();
    let target = p_dir.path().join("créé.txt");
    let m = model(vec![tool(
        "c1",
        "Write",
        json!({"file_path": target.to_string_lossy(), "content": "x"}),
    )]);
    let p = Project::new(&m.url, "");
    let out = p.run(
        &["run", "--json", "--non-interactive", "Écris le fichier"],
        None,
    );
    assert_eq!(
        out.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!target.exists(), "nothing was written");
    let evs = envelopes(&out.stdout);
    let fin = evs.iter().find(|e| e["type"] == "run_finished").unwrap();
    assert_eq!(fin["outcome"], "failed");
    assert_eq!(fin["failure"], "approval_required");
    assert_eq!(fin["approvals_unsatisfied"][0]["tool"], "Write");
    assert!(kinds(&evs).contains(&"approval_requested".to_string()));

    // Allowed by the configured policy: it runs, non-interactive or not.
    let m = model(vec![
        tool(
            "c1",
            "Write",
            json!({"file_path": target.to_string_lossy(), "content": "x"}),
        ),
        text("fait"),
    ]);
    let p = Project::new(&m.url, "[permissions]\nwrite = \"allow\"\n");
    let out = p.run(&["run", "--non-interactive", "Écris le fichier"], None);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "x");
}

#[test]
fn configuration_errors_are_reported_with_code_2() {
    let m = model(vec![]);
    let p = Project::new(&m.url, "");
    // Unknown model: the configured ones are listed, nothing is guessed.
    let out = p.run(&["run", "--model", "local/nope", "x"], None);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("local/m") || err.contains("its models: m"),
        "{err}"
    );
    assert!(out.stdout.is_empty());
    // Unknown reasoning profile.
    let out = p.run(&["run", "--reasoning", "turbo", "x"], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("turbo"));
    // Missing providers file.
    let out = p.run(
        &["--providers", "/nonexistent/providers.toml", "run", "x"],
        None,
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("providers"));
    // No prompt at all (stdin empty).
    let out = p.run(&["run"], Some(""));
    assert_eq!(out.status.code(), Some(2));
    // A missing attachment: refused before anything runs.
    let out = p.run(&["run", "--json", "x", "--file", "absent.rs"], None);
    assert_eq!(out.status.code(), Some(2));
    let evs = envelopes(&out.stdout);
    assert!(
        kinds(&evs).contains(&"command_rejected".to_string()),
        "{evs:?}"
    );
    assert!(
        m.seen.lock().unwrap().is_empty(),
        "the model was never called"
    );
}

#[test]
fn a_session_continues_headless_with_its_settings() {
    let m = model(vec![text("un"), text("un bis"), text("deux")]);
    let p = Project::new(&m.url, "");
    let out = p.run(&["run", "--json", "--reasoning", "deep", "premier"], None);
    assert_eq!(out.status.code(), Some(0));
    let id = envelopes(&out.stdout)[0]["session_id"]
        .as_str()
        .unwrap()
        .to_string();
    let out = p.run(&["run", "--json", "--session", &id, "second"], None);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let evs = envelopes(&out.stdout);
    assert_eq!(evs[0]["resumed"], true);
    assert_eq!(
        evs[0]["reasoning"], "deep",
        "the session's profile is restored"
    );
    let last = m.seen.lock().unwrap().last().unwrap().to_string();
    assert!(
        last.contains("premier") && last.contains("second"),
        "the history is sent: {last}"
    );
    let out = p.run(&["run", "--session", "nope", "x"], None);
    assert_eq!(out.status.code(), Some(2));
}

fn write_skill(root: &std::path::Path, name: &str, body: &str) {
    let d = root.join(name);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Procedure {name}.\n---\n{body}\n"),
    )
    .unwrap();
}

/// `--skill` activates before the prompt (separate from it), with the
/// same checks as `/skill`; a project skill needs `--trust-project-skills`
/// (this session only); a skill that cannot be used sends nothing (2).
#[test]
fn skills_are_chosen_explicitly_and_project_skills_need_explicit_trust() {
    let m = model(vec![text("fait"), text("fait")]);
    let p = Project::new(&m.url, "");
    write_skill(
        &p.path().join(".agents/skills"),
        "house-style",
        "HOUSE-STYLE-BODY",
    );
    write_skill(&p.home.path().join(".agents/skills"), "mine", "MINE-BODY");

    // Without trust: the project skill is refused, nothing is sent.
    let out = p.run(
        &[
            "run",
            "--json",
            "--non-interactive",
            "--skill",
            "house-style",
            "Écris",
        ],
        None,
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let evs = envelopes(&out.stdout);
    let rejected = evs
        .iter()
        .find(|e| e["type"] == "command_rejected")
        .unwrap();
    assert!(rejected["reason"]
        .as_str()
        .unwrap()
        .contains("not approved"));
    assert!(evs.iter().any(|e| e["type"] == "notice"
        && e["message"]
            .as_str()
            .unwrap()
            .contains("--trust-project-skills")));
    assert!(m.seen.lock().unwrap().is_empty(), "no request sent");

    // With trust and a personal skill: both activated, in order, before
    // the prompt, which stays its own message.
    let out = p.run(
        &[
            "run",
            "--json",
            "--non-interactive",
            "--trust-project-skills",
            "--skill",
            "house-style",
            "--skill",
            "mine",
            "Écris le README",
        ],
        None,
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let evs = envelopes(&out.stdout);
    let resolved = evs
        .iter()
        .find(|e| e["type"] == "skills_trust_resolved")
        .unwrap();
    assert_eq!(resolved["by"], "caller");
    let act: Vec<_> = evs
        .iter()
        .filter(|e| e["type"] == "skill_activated")
        .map(|e| {
            (
                e["name"].as_str().unwrap().to_string(),
                e["origin"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        act,
        [
            ("house-style".to_string(), "user".to_string()),
            ("mine".to_string(), "user".to_string())
        ]
    );
    for e in &evs {
        assert_eq!(e["schema"], 5, "additions keep the schema version");
    }
    let body = m.seen.lock().unwrap()[0].to_string();
    let (i, j, k) = (
        body.find("HOUSE-STYLE-BODY").unwrap(),
        body.find("MINE-BODY").unwrap(),
        body.find("Écris le README").unwrap(),
    );
    assert!(i < j && j < k, "skills first, then the request");
    // The catalog of the request lists the skills, never their bodies twice.
    assert_eq!(body.matches("HOUSE-STYLE-BODY").count(), 1);

    // An unknown skill: refused before anything is sent.
    let before = m.seen.lock().unwrap().len();
    let out = p.run(
        &["run", "--non-interactive", "--skill", "no-such", "x"],
        None,
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no-such"));
    assert_eq!(m.seen.lock().unwrap().len(), before);
}

/// Without any skill on the machine (temporary home), a session works as
/// before; the built-in skills are listed, nothing else.
#[test]
fn a_session_without_skills_works_and_lists_only_built_ins() {
    let m = model(vec![text("ok")]);
    let p = Project::new(&m.url, "");
    let out = p.run(&["run", "--json", "--non-interactive", "Bonjour"], None);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body = m.seen.lock().unwrap()[0].to_string();
    assert!(body.contains("<name>verify</name>") && body.contains("<name>debug</name>"));
    assert!(!body.contains("$ARGUMENTS"));
}
