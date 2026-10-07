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
        assert_eq!(e["schema"], 3);
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

/// A run stopped at its turn limit is reported as incomplete, with its own
/// exit code: never as a success.
#[test]
fn a_run_at_its_turn_limit_is_incomplete() {
    let m = model(vec![
        tool("c1", "Glob", json!({"pattern": "*.md"})),
        text("jamais demandé"),
    ]);
    let p = Project::new(&m.url, "max_turns = 1\n");
    let out = p.run(&["run", "--json", "--non-interactive", "Liste"], None);
    assert_eq!(
        out.status.code(),
        Some(5),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let evs = envelopes(&out.stdout);
    let fin: Vec<_> = evs.iter().filter(|e| e["type"] == "run_finished").collect();
    assert_eq!(fin.len(), 1);
    assert_eq!(fin[0]["outcome"], "incomplete");
    assert_eq!(fin[0]["termination"]["kind"], "max_turns");
    assert_eq!(fin[0]["turns"], 1);
    assert_eq!(m.seen.lock().unwrap().len(), 1, "one request, no relaunch");
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
