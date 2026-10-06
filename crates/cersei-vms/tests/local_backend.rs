//! Integration tests for `LocalProcessRuntime`.

use cersei_vms::prelude::*;
use tempfile::TempDir;
use tokio_stream::StreamExt;

async fn make_runtime() -> (LocalProcessRuntime, TempDir) {
    let dir = TempDir::new().unwrap();
    let rt = LocalProcessRuntime::with_root(dir.path()).unwrap();
    (rt, dir)
}

#[tokio::test]
async fn create_and_run_echo() {
    let (rt, _g) = make_runtime().await;
    let sb = rt
        .create(SandboxOpts::image("local").with_workdir("/work"))
        .await
        .unwrap();
    let out = sb
        .commands()
        .run(RunRequest::new("echo hello"))
        .await
        .unwrap();
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.contains("hello"));
}

#[tokio::test]
async fn filesystem_roundtrip() {
    let (rt, _g) = make_runtime().await;
    let sb = rt.create(SandboxOpts::image("local")).await.unwrap();
    let fs = sb.filesystem();
    fs.write("/work/hello.txt", b"world").await.unwrap();
    let read = fs.read("/work/hello.txt").await.unwrap();
    assert_eq!(&read[..], b"world");
    let entry = fs.stat("/work/hello.txt").await.unwrap();
    assert_eq!(entry.size, 5);
}

#[tokio::test]
async fn stream_emits_lines_then_exit() {
    let (rt, _g) = make_runtime().await;
    let sb = rt.create(SandboxOpts::image("local")).await.unwrap();
    let mut stream = sb
        .commands()
        .stream(RunRequest::new("for i in 1 2 3; do echo $i; done"))
        .await
        .unwrap();
    let mut lines = Vec::new();
    let mut exit = None;
    while let Some(chunk) = stream.next().await {
        match chunk {
            StreamChunk::Stdout { data } => lines.push(data),
            StreamChunk::Exit { code } => {
                exit = Some(code);
                break;
            }
            _ => {}
        }
    }
    assert_eq!(lines, vec!["1", "2", "3"]);
    assert_eq!(exit, Some(0));
}

#[tokio::test]
async fn snapshot_and_restore_preserves_fs() {
    let (rt, _g) = make_runtime().await;
    let sb = rt.create(SandboxOpts::image("local")).await.unwrap();
    sb.filesystem()
        .write("/work/state.txt", b"keep me")
        .await
        .unwrap();
    rt.kv().set("progress", b"42".to_vec()).unwrap();
    let snap = sb.snapshot().await.unwrap();
    sb.kill().await.unwrap();

    let restored = rt.restore(&snap).await.unwrap();
    let bytes = restored.filesystem().read("/work/state.txt").await.unwrap();
    assert_eq!(&bytes[..], b"keep me");
    assert_eq!(rt.kv().get("progress").unwrap().value, b"42".to_vec());
}
