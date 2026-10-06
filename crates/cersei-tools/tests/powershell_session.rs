//! The persistent PowerShell, with a real `pwsh`/`powershell`.
//!
//! Ignored by default: run with `cargo test -p cersei-tools --test
//! powershell_session -- --ignored` on a machine where PowerShell is
//! installed (Windows, or pwsh on macOS/Linux). Not executed in the
//! environment where this sprint was developed (no PowerShell there).

use cersei_tools::shell::powershell::PwshStatus;
use cersei_tools::shell::{self, ShellConfig};
use std::time::Duration;

#[tokio::test]
#[ignore = "requires PowerShell (pwsh, or powershell on Windows)"]
async fn state_persists_and_statuses_are_distinguished() {
    let cfg = ShellConfig::default();
    let id = format!("ps-{}", uuid::Uuid::new_v4());
    let s = shell::session(&id, &cfg);
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("sous dossier")).unwrap();
    let ps = s.pwsh(dir.path(), &cfg).await.unwrap();
    let run = |c: &'static str| {
        let ps = ps.clone();
        async move { ps.run(c, Duration::from_secs(60), None).await.unwrap() }
    };

    run("$env:MA_VAR = 'Bricks42'; Set-Location 'sous dossier'; function Greet { 'hello ' + $args[0] }; Set-Alias gg Greet").await;
    let o = run("$env:MA_VAR; Split-Path -Leaf (Get-Location); gg Bricks").await;
    assert_eq!(
        o.stdout.text.lines().map(str::trim).collect::<Vec<_>>(),
        vec!["Bricks42", "sous dossier", "hello Bricks"]
    );

    // A cmdlet error without a native program: no exit code reported.
    let o = run("Get-Item ./does-not-exist").await;
    assert!(
        matches!(
            o.status,
            PwshStatus::Completed {
                native_exit: None,
                cmdlet_errors: 1..,
                ..
            }
        ),
        "{:?}",
        o.status
    );
    // A native program's code, then a command without one: not recycled.
    let native = if cfg!(windows) {
        "cmd /c exit 3"
    } else {
        "sh -c 'exit 3'"
    };
    let o = ps.run(native, Duration::from_secs(30), None).await.unwrap();
    assert!(
        matches!(
            o.status,
            PwshStatus::Completed {
                native_exit: Some(3),
                ..
            }
        ),
        "{:?}",
        o.status
    );
    let o = run("'no native here'").await;
    assert!(
        matches!(
            o.status,
            PwshStatus::Completed {
                ok: true,
                native_exit: None,
                ..
            }
        ),
        "{:?}",
        o.status
    );

    // A timeout replaces the session, explicitly.
    let o = ps
        .run("Start-Sleep -Seconds 30", Duration::from_millis(500), None)
        .await
        .unwrap();
    assert_eq!(o.status, PwshStatus::TimedOut);
    assert!(ps.is_dead());
    let fresh = s.pwsh(dir.path(), &cfg).await.unwrap();
    assert!(s.take_reset_notice().is_some());
    let o = fresh
        .run("\"[$env:MA_VAR]\"", Duration::from_secs(30), None)
        .await
        .unwrap();
    assert_eq!(o.stdout.text.trim(), "[]");
    shell::close_session(&id).await;
}
