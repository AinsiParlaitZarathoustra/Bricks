//! Isolated workspaces: git worktrees that start from the parent's
//! **effective** state, the work they produce (ChangeSets), and its
//! controlled local integration.
//!
//! * **Snapshot.** The parent's view is captured without touching it: the
//!   base commit, the tracked changes against it (`git diff --binary HEAD`:
//!   staged, unstaged, deletions, modes) and the untracked files git does
//!   not ignore (copied, under a manifest, with size limits). Reads run
//!   with `GIT_OPTIONAL_LOCKS=0`: the parent's index is never written; no
//!   stash, reset, checkout or commit. The capture is retried when the
//!   parent changes during it.
//! * **Worktree.** `git worktree add -b bricks/<run>/<agent> <path> <base>`
//!   (never `-B`), outside the project (in the session's files), user hooks
//!   disabled; then the snapshot is applied to it. Every child of a batch
//!   starts from the same snapshot.
//! * **Baseline.** The tree of the worktree right after that (built with a
//!   temporary index, never the worktree's or the parent's index, no
//!   commit). The child's ChangeSet is the diff from this baseline: changes
//!   inherited from the parent are never attributed to the child.
//! * **Apply.** `git apply --check` then `git apply` on the destination's
//!   working tree: all or nothing; a change that no longer applies (the
//!   destination changed the same content meanwhile) is a conflict and
//!   nothing is written. No commit, no PR.
//! * **Cleanup.** Only what the manifest says the runtime created. An
//!   unchanged worktree is removed at the end of its agent; a changed one is
//!   kept for review until applied or discarded. A branch is deleted only if
//!   it still points at its base commit (`update-ref -d <ref> <base>`).
//!
//! A worktree is not a sandbox: processes, network, ports, databases and
//! the repository's shared metadata stay shared.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// Limits of a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotLimits {
    /// Untracked files copied at most.
    pub max_untracked_files: usize,
    /// Largest untracked file copied.
    pub max_untracked_file_bytes: u64,
    /// Total bytes (tracked patch + untracked copies).
    pub max_total_bytes: u64,
    /// Largest ChangeSet patch kept.
    pub max_patch_bytes: u64,
}

impl Default for SnapshotLimits {
    fn default() -> Self {
        Self {
            max_untracked_files: 2_000,
            max_untracked_file_bytes: 5 * 1024 * 1024,
            max_total_bytes: 200 * 1024 * 1024,
            max_patch_bytes: 20 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsError {
    NotARepo(String),
    NoHead,
    Unsupported(String),
    Collision(String),
    Inconsistent(String),
    TooLarge(String),
    Git(String),
    Io(String),
    NotFound(String),
    NotOwned(String),
    /// The change no longer applies: nothing was written.
    Conflict {
        files: Vec<String>,
        detail: String,
    },
}

impl std::fmt::Display for WsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotARepo(d) => write!(f, "worktree isolation needs a git repository ({d})"),
            Self::NoHead => write!(f, "worktree isolation needs a first commit (HEAD does not exist)"),
            Self::Unsupported(m) => write!(f, "worktree isolation not supported here: {m}"),
            Self::Collision(m) => write!(f, "already exists, not overwritten: {m}"),
            Self::Inconsistent(m) => write!(f, "inconsistent snapshot: {m}"),
            Self::TooLarge(m) => write!(f, "{m}"),
            Self::Git(m) => write!(f, "git: {m}"),
            Self::Io(m) => write!(f, "{m}"),
            Self::NotFound(m) => write!(f, "not found: {m}"),
            Self::NotOwned(m) => write!(f, "not created by this runtime: {m}"),
            Self::Conflict { files, detail } => write!(
                f,
                "WorktreeConflict: the change no longer applies to {} (nothing was written): {detail}",
                if files.is_empty() { "the destination".to_string() } else { files.join(", ") }
            ),
        }
    }
}

/// The parent's state, captured once for a batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub repo_root: PathBuf,
    /// The parent's working directory relative to the repository root.
    pub rel_cwd: PathBuf,
    pub base_sha: String,
    /// Tracked changes against `base_sha` (may be empty).
    pub patch: PathBuf,
    pub patch_bytes: u64,
    /// Untracked files copied (relative paths) and where.
    pub untracked: Vec<String>,
    pub untracked_dir: PathBuf,
    /// Untracked files left out, and why.
    pub excluded: Vec<String>,
    pub fingerprint: String,
}

/// A worktree this runtime created.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedWorktree {
    pub agent_id: String,
    pub root: PathBuf,
    /// The agent's working directory inside it.
    pub cwd: PathBuf,
    pub branch: String,
    pub base_sha: String,
    pub snapshot_id: String,
    pub baseline_tree: String,
    pub repo_root: PathBuf,
    /// `active`, `kept`, `removed`.
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: String,
    /// `A`, `M`, `D`, `R<score>` (git's letters).
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    pub added: Option<u64>,
    pub removed: Option<u64>,
    /// Binary (no line counts).
    pub binary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeSetState {
    Ready,
    Applied,
    Conflict,
    Discarded,
}

/// The work of an isolated agent, recoverable without its transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeSet {
    pub id: String,
    pub agent_id: String,
    pub task: String,
    pub workspace: String,
    pub branch: String,
    pub base_sha: String,
    pub snapshot_id: String,
    pub baseline_tree: String,
    pub final_tree: String,
    pub files: Vec<ChangedFile>,
    /// The patch (`git apply`-able), stored with the session.
    pub patch: String,
    pub patch_bytes: u64,
    pub state: ChangeSetState,
    /// Validations the agent actually ran (commands and outcomes).
    #[serde(default)]
    pub validations: Vec<super::spawn::CommandRun>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    worktrees: Vec<ManagedWorktree>,
    changesets: Vec<ChangeSet>,
}

/// The session's workspaces.
pub struct WorkspaceManager {
    base: Option<PathBuf>,
    pub limits: SnapshotLimits,
    manifest: Mutex<Manifest>,
    git_version: Mutex<Option<String>>,
    /// `git worktree add/remove` one at a time: concurrent ones read each
    /// other's half-written administrative files.
    admin: tokio::sync::Mutex<()>,
}

async fn git(dir: &Path, args: &[&str], env: &[(&str, &Path)]) -> Result<Vec<u8>, WsError> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-c")
        .arg("core.hooksPath=/dev/null")
        .args(args)
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd
        .output()
        .await
        .map_err(|e| WsError::Git(format!("cannot run git: {e}")))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(WsError::Git(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ))
    }
}

fn text(b: Vec<u8>) -> String {
    String::from_utf8_lossy(&b).trim().to_string()
}

fn short_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}

impl WorkspaceManager {
    /// `base`: where worktrees, snapshots and ChangeSets live (the
    /// session's files; `None`: isolation unavailable).
    pub fn new(base: Option<PathBuf>, limits: SnapshotLimits) -> Self {
        let manifest = base
            .as_ref()
            .and_then(|b| std::fs::read_to_string(b.join("workspaces.json")).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Self {
            base,
            limits,
            manifest: Mutex::new(manifest),
            git_version: Mutex::new(None),
            admin: tokio::sync::Mutex::new(()),
        }
    }

    fn base(&self) -> Result<&Path, WsError> {
        self.base
            .as_deref()
            .ok_or_else(|| WsError::Unsupported("no session folder to keep worktrees in".into()))
    }

    fn save(&self) {
        let Ok(base) = self.base() else { return };
        let _ = std::fs::create_dir_all(base);
        if let Ok(body) = serde_json::to_vec_pretty(&*self.manifest.lock()) {
            let tmp = base.join("workspaces.json.tmp");
            if std::fs::write(&tmp, body).is_ok() {
                let _ = std::fs::rename(&tmp, base.join("workspaces.json"));
            }
        }
    }

    /// The installed git's version (checked once; worktrees need ≥ 2.17
    /// for `worktree remove`).
    pub async fn git_version(&self) -> Result<String, WsError> {
        if let Some(v) = self.git_version.lock().clone() {
            return Ok(v);
        }
        let v = text(git(Path::new("."), &["--version"], &[]).await?);
        let num = v.split_whitespace().nth(2).unwrap_or("0").to_string();
        let parts: Vec<u32> = num.split('.').filter_map(|p| p.parse().ok()).collect();
        if parts.len() < 2 || (parts[0], parts[1]) < (2, 17) {
            return Err(WsError::Unsupported(format!(
                "git {num} is too old (2.17 or later needed)"
            )));
        }
        *self.git_version.lock() = Some(v.clone());
        Ok(v)
    }

    async fn state_of(
        &self,
        root: &Path,
        limits: &SnapshotLimits,
    ) -> Result<(Vec<u8>, Vec<(String, u64)>), WsError> {
        let patch = git(
            root,
            &["diff", "--binary", "--full-index", "HEAD", "--"],
            &[],
        )
        .await?;
        let list = git(
            root,
            &["ls-files", "--others", "--exclude-standard", "-z"],
            &[],
        )
        .await?;
        let mut untracked = Vec::new();
        for p in list.split(|b| *b == 0).filter(|p| !p.is_empty()) {
            let rel = String::from_utf8_lossy(p).to_string();
            let size = std::fs::symlink_metadata(root.join(&rel))
                .map(|m| m.len())
                .unwrap_or(0);
            untracked.push((rel, size));
            if untracked.len() > limits.max_untracked_files * 4 {
                break;
            }
        }
        untracked.sort();
        Ok((patch, untracked))
    }

    fn fingerprint(patch: &[u8], untracked: &[(String, u64)], root: &Path) -> String {
        let mut h = Sha256::new();
        h.update(patch);
        for (p, size) in untracked {
            h.update(p.as_bytes());
            h.update(size.to_le_bytes());
            if let Ok(m) = std::fs::metadata(root.join(p)).and_then(|m| m.modified()) {
                if let Ok(d) = m.duration_since(std::time::UNIX_EPOCH) {
                    h.update(d.as_nanos().to_le_bytes());
                }
            }
        }
        h.finalize()
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// Capture the parent's effective state (see the module docs).
    pub async fn snapshot(&self, parent_dir: &Path) -> Result<Snapshot, WsError> {
        self.git_version().await?;
        let base = self.base()?.to_path_buf();
        let top = git(parent_dir, &["rev-parse", "--show-toplevel"], &[])
            .await
            .map_err(|e| WsError::NotARepo(e.to_string()))?;
        let repo_root = PathBuf::from(text(top));
        let repo_root = std::fs::canonicalize(&repo_root).unwrap_or(repo_root);
        let base_sha = git(
            &repo_root,
            &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
            &[],
        )
        .await
        .map(text)
        .map_err(|_| WsError::NoHead)?;
        if repo_root.join(".gitmodules").exists() {
            return Err(WsError::Unsupported("the repository has submodules".into()));
        }
        let pd = std::fs::canonicalize(parent_dir).unwrap_or_else(|_| parent_dir.to_path_buf());
        let rel_cwd = pd
            .strip_prefix(&repo_root)
            .unwrap_or(Path::new(""))
            .to_path_buf();
        let limits = self.limits.clone();

        // A consistent capture: the same state read twice in a row.
        let mut attempt = 0;
        let (patch, untracked, fp) = loop {
            attempt += 1;
            let (p1, u1) = self.state_of(&repo_root, &limits).await?;
            let f1 = Self::fingerprint(&p1, &u1, &repo_root);
            let (p2, u2) = self.state_of(&repo_root, &limits).await?;
            let f2 = Self::fingerprint(&p2, &u2, &repo_root);
            if f1 == f2 {
                break (p2, u2, f2);
            }
            if attempt >= 3 {
                return Err(WsError::Inconsistent(
                    "the parent's files kept changing during the capture (3 attempts)".into(),
                ));
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        };

        let id = format!("snap_{}", short_id());
        let dir = base.join("snapshots").join(&id);
        let files_dir = dir.join("untracked");
        std::fs::create_dir_all(&files_dir).map_err(|e| WsError::Io(e.to_string()))?;
        let patch_path = dir.join("tracked.patch");
        std::fs::write(&patch_path, &patch).map_err(|e| WsError::Io(e.to_string()))?;
        let mut total = patch.len() as u64;
        let mut copied = Vec::new();
        let mut excluded = Vec::new();
        for (rel, size) in untracked {
            if copied.len() >= limits.max_untracked_files {
                excluded.push(format!(
                    "{rel}: more than {} untracked files",
                    limits.max_untracked_files
                ));
                continue;
            }
            if size > limits.max_untracked_file_bytes {
                excluded.push(format!(
                    "{rel}: {size} bytes > {}",
                    limits.max_untracked_file_bytes
                ));
                continue;
            }
            if total + size > limits.max_total_bytes {
                excluded.push(format!(
                    "{rel}: snapshot limit of {} bytes reached",
                    limits.max_total_bytes
                ));
                continue;
            }
            let src = repo_root.join(&rel);
            let meta = match std::fs::symlink_metadata(&src) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if !meta.is_file() {
                excluded.push(format!("{rel}: not a regular file"));
                continue;
            }
            let dst = files_dir.join(&rel);
            if let Some(p) = dst.parent() {
                std::fs::create_dir_all(p).map_err(|e| WsError::Io(e.to_string()))?;
            }
            std::fs::copy(&src, &dst).map_err(|e| WsError::Io(format!("{rel}: {e}")))?;
            total += size;
            copied.push(rel);
        }
        let snap = Snapshot {
            id,
            repo_root,
            rel_cwd,
            base_sha,
            patch: patch_path,
            patch_bytes: patch.len() as u64,
            untracked: copied,
            untracked_dir: files_dir,
            excluded,
            fingerprint: fp,
        };
        let _ = std::fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec_pretty(&snap).unwrap_or_default(),
        );
        Ok(snap)
    }

    async fn tree_of(&self, worktree: &Path, name: &str) -> Result<String, WsError> {
        let base = self.base()?;
        let idx_dir = base.join("indexes");
        std::fs::create_dir_all(&idx_dir).map_err(|e| WsError::Io(e.to_string()))?;
        let idx = idx_dir.join(format!("{name}-{}", short_id()));
        let r = async {
            git(
                worktree,
                &["add", "-A", "--", "."],
                &[("GIT_INDEX_FILE", &idx)],
            )
            .await?;
            git(worktree, &["write-tree"], &[("GIT_INDEX_FILE", &idx)])
                .await
                .map(text)
        }
        .await;
        let _ = std::fs::remove_file(&idx);
        r
    }

    /// A worktree for `agent_id`, from `snap`.
    pub async fn create(
        &self,
        snap: &Snapshot,
        run_id: &str,
        agent_id: &str,
    ) -> Result<ManagedWorktree, WsError> {
        let base = self.base()?.to_path_buf();
        let root = base.join("worktrees").join(agent_id);
        if root.exists() {
            return Err(WsError::Collision(root.display().to_string()));
        }
        let branch = format!("bricks/{run_id}/{agent_id}");
        let refname = format!("refs/heads/{branch}");
        if git(
            &snap.repo_root,
            &["rev-parse", "--verify", "--quiet", &refname],
            &[],
        )
        .await
        .is_ok()
        {
            return Err(WsError::Collision(format!("branch {branch}")));
        }
        std::fs::create_dir_all(root.parent().unwrap_or(&base))
            .map_err(|e| WsError::Io(e.to_string()))?;
        let root_s = root.display().to_string();
        let admin = self.admin.lock().await;
        git(
            &snap.repo_root,
            &["worktree", "add", "-b", &branch, &root_s, &snap.base_sha],
            &[],
        )
        .await?;
        drop(admin);
        let mut wt = ManagedWorktree {
            agent_id: agent_id.to_string(),
            cwd: root.join(&snap.rel_cwd),
            root: root.clone(),
            branch,
            base_sha: snap.base_sha.clone(),
            snapshot_id: snap.id.clone(),
            baseline_tree: String::new(),
            repo_root: snap.repo_root.clone(),
            state: "active".into(),
        };
        // From here on, the worktree is ours: recorded before anything can
        // fail, so it can always be found and cleaned.
        self.manifest.lock().worktrees.push(wt.clone());
        self.save();
        let prepared = async {
            if snap.patch_bytes > 0 {
                let p = snap.patch.display().to_string();
                git(
                    &root,
                    &["apply", "--binary", "--whitespace=nowarn", &p],
                    &[],
                )
                .await?;
            }
            for rel in &snap.untracked {
                let dst = root.join(rel);
                if let Some(p) = dst.parent() {
                    std::fs::create_dir_all(p).map_err(|e| WsError::Io(e.to_string()))?;
                }
                std::fs::copy(snap.untracked_dir.join(rel), &dst)
                    .map_err(|e| WsError::Io(format!("{rel}: {e}")))?;
            }
            self.tree_of(&root, "baseline").await
        }
        .await;
        match prepared {
            Ok(tree) => {
                wt.baseline_tree = tree;
                self.update_worktree(&wt);
                Ok(wt)
            }
            Err(e) => {
                let _ = self.remove_worktree(agent_id, true).await;
                Err(e)
            }
        }
    }

    fn update_worktree(&self, wt: &ManagedWorktree) {
        {
            let mut m = self.manifest.lock();
            if let Some(w) = m.worktrees.iter_mut().find(|w| w.agent_id == wt.agent_id) {
                *w = wt.clone();
            }
        }
        self.save();
    }

    pub fn worktree(&self, agent_id: &str) -> Option<ManagedWorktree> {
        self.manifest
            .lock()
            .worktrees
            .iter()
            .find(|w| w.agent_id == agent_id)
            .cloned()
    }

    /// Whether `path` is inside a worktree this runtime manages.
    pub fn manages(&self, path: &Path) -> bool {
        let p = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.manifest.lock().worktrees.iter().any(|w| {
            let r = std::fs::canonicalize(&w.root).unwrap_or_else(|_| w.root.clone());
            p.starts_with(&r)
        })
    }

    /// The agent ended: its ChangeSet if it changed something (the worktree
    /// is kept for review), else the worktree is removed.
    pub async fn finalize(
        &self,
        agent_id: &str,
        task: &str,
        validations: Vec<super::spawn::CommandRun>,
    ) -> Result<Option<ChangeSet>, WsError> {
        let wt = self
            .worktree(agent_id)
            .ok_or_else(|| WsError::NotFound(agent_id.into()))?;
        let final_tree = self.tree_of(&wt.root, "final").await?;
        if final_tree == wt.baseline_tree {
            self.remove_worktree(agent_id, true).await?;
            return Ok(None);
        }
        let base = self.base()?.to_path_buf();
        let id = format!("cs_{}", short_id());
        let dir = base.join("changesets");
        std::fs::create_dir_all(&dir).map_err(|e| WsError::Io(e.to_string()))?;
        let patch = git(
            &wt.root,
            &[
                "diff",
                "--binary",
                "--full-index",
                "-M",
                &wt.baseline_tree,
                &final_tree,
            ],
            &[],
        )
        .await?;
        let patch_path = dir.join(format!("{id}.patch"));
        let mut note = None;
        if patch.len() as u64 > self.limits.max_patch_bytes {
            note = Some(format!(
                "patch of {} bytes exceeds the {}-byte limit: kept only in the worktree {}",
                patch.len(),
                self.limits.max_patch_bytes,
                wt.root.display()
            ));
        } else {
            std::fs::write(&patch_path, &patch).map_err(|e| WsError::Io(e.to_string()))?;
        }
        let numstat = text(
            git(
                &wt.root,
                &[
                    "diff",
                    "--numstat",
                    "-M",
                    "-z",
                    &wt.baseline_tree,
                    &final_tree,
                ],
                &[],
            )
            .await?,
        );
        let status = text(
            git(
                &wt.root,
                &[
                    "diff",
                    "--name-status",
                    "-M",
                    &wt.baseline_tree,
                    &final_tree,
                ],
                &[],
            )
            .await?,
        );
        let files = parse_files(&status, &numstat);
        let cs = ChangeSet {
            id,
            agent_id: agent_id.into(),
            task: task.chars().take(200).collect(),
            workspace: wt.root.display().to_string(),
            branch: wt.branch.clone(),
            base_sha: wt.base_sha.clone(),
            snapshot_id: wt.snapshot_id.clone(),
            baseline_tree: wt.baseline_tree.clone(),
            final_tree,
            files,
            patch: patch_path.display().to_string(),
            patch_bytes: patch.len() as u64,
            state: ChangeSetState::Ready,
            validations,
            note,
        };
        {
            let mut m = self.manifest.lock();
            m.changesets.push(cs.clone());
            if let Some(w) = m.worktrees.iter_mut().find(|w| w.agent_id == agent_id) {
                w.state = "kept".into();
            }
        }
        self.save();
        Ok(Some(cs))
    }

    pub fn changeset(&self, id: &str) -> Option<ChangeSet> {
        self.manifest
            .lock()
            .changesets
            .iter()
            .find(|c| c.id == id)
            .cloned()
    }

    pub fn changeset_of(&self, agent_id: &str) -> Option<ChangeSet> {
        self.manifest
            .lock()
            .changesets
            .iter()
            .find(|c| c.agent_id == agent_id)
            .cloned()
    }

    pub fn changesets(&self) -> Vec<ChangeSet> {
        self.manifest.lock().changesets.clone()
    }

    fn set_cs_state(&self, id: &str, state: ChangeSetState, note: Option<String>) {
        {
            let mut m = self.manifest.lock();
            if let Some(c) = m.changesets.iter_mut().find(|c| c.id == id) {
                c.state = state;
                if note.is_some() {
                    c.note = note;
                }
            }
        }
        self.save();
    }

    /// The patch text (bounded), for inspection.
    pub fn patch_text(&self, id: &str, max: usize) -> Result<(String, bool), WsError> {
        let cs = self
            .changeset(id)
            .ok_or_else(|| WsError::NotFound(id.into()))?;
        let body =
            std::fs::read(&cs.patch).map_err(|e| WsError::Io(format!("{}: {e}", cs.patch)))?;
        let truncated = body.len() > max;
        let mut cut = body.len().min(max);
        while cut > 0 && std::str::from_utf8(&body[..cut]).is_err() {
            cut -= 1;
        }
        Ok((String::from_utf8_lossy(&body[..cut]).to_string(), truncated))
    }

    /// Apply a ChangeSet to the working tree of `dest_dir`'s repository:
    /// all or nothing; a conflict writes nothing and keeps both versions.
    pub async fn apply(&self, id: &str, dest_dir: &Path) -> Result<ChangeSet, WsError> {
        let cs = self
            .changeset(id)
            .ok_or_else(|| WsError::NotFound(id.into()))?;
        if cs.state != ChangeSetState::Ready && cs.state != ChangeSetState::Conflict {
            return Err(WsError::Unsupported(
                format!("ChangeSet {id} is {:?}", cs.state).to_lowercase(),
            ));
        }
        if !Path::new(&cs.patch).exists() {
            return Err(WsError::NotFound(format!("patch of {id} ({})", cs.patch)));
        }
        let top = PathBuf::from(text(
            git(dest_dir, &["rev-parse", "--show-toplevel"], &[])
                .await
                .map_err(|e| WsError::NotARepo(e.to_string()))?,
        ));
        if let Err(WsError::Git(detail)) = git(
            &top,
            &[
                "apply",
                "--check",
                "--binary",
                "--whitespace=nowarn",
                &cs.patch,
            ],
            &[],
        )
        .await
        {
            let files = conflicting_files(&detail, &cs);
            self.set_cs_state(id, ChangeSetState::Conflict, Some(detail.clone()));
            return Err(WsError::Conflict { files, detail });
        }
        git(
            &top,
            &["apply", "--binary", "--whitespace=nowarn", &cs.patch],
            &[],
        )
        .await?;
        self.set_cs_state(id, ChangeSetState::Applied, None);
        // Integrated: the worktree has done its job.
        let _ = self.remove_worktree(&cs.agent_id, true).await;
        Ok(self.changeset(id).unwrap_or(cs))
    }

    /// Discard a ChangeSet: its worktree (owned) is removed; the patch stays
    /// with the session as a record.
    pub async fn discard(&self, id: &str) -> Result<ChangeSet, WsError> {
        let cs = self
            .changeset(id)
            .ok_or_else(|| WsError::NotFound(id.into()))?;
        if cs.state == ChangeSetState::Applied {
            return Err(WsError::Unsupported(format!(
                "ChangeSet {id} is already applied"
            )));
        }
        self.remove_worktree(&cs.agent_id, false).await?;
        self.set_cs_state(id, ChangeSetState::Discarded, None);
        Ok(self.changeset(id).unwrap_or(cs))
    }

    /// Remove an owned worktree. `unchanged`: it has no change of its own
    /// (only the inherited snapshot), or its patch is archived.
    async fn remove_worktree(&self, agent_id: &str, unchanged: bool) -> Result<(), WsError> {
        let wt = self
            .worktree(agent_id)
            .ok_or_else(|| WsError::NotOwned(agent_id.into()))?;
        if wt.state == "removed" {
            return Ok(());
        }
        if !unchanged {
            // Only with its work archived.
            let archived = self
                .changeset_of(agent_id)
                .is_some_and(|c| Path::new(&c.patch).exists());
            if !archived {
                return Err(WsError::Unsupported(format!(
                    "the worktree of {agent_id} has changes that are not archived; not removed"
                )));
            }
        }
        if wt.root.exists() {
            let root = wt.root.display().to_string();
            let _admin = self.admin.lock().await;
            // Targeted at a worktree this runtime created and recorded (its
            // changes, if any, are archived): never a general cleanup.
            git(
                &wt.repo_root,
                &["worktree", "remove", "--force", &root],
                &[],
            )
            .await?;
        }
        let refname = format!("refs/heads/{}", wt.branch);
        // Deleted only if it still points at its base (no commit on it).
        let _ = git(
            &wt.repo_root,
            &["update-ref", "-d", &refname, &wt.base_sha],
            &[],
        )
        .await;
        {
            let mut m = self.manifest.lock();
            if let Some(w) = m.worktrees.iter_mut().find(|w| w.agent_id == agent_id) {
                w.state = "removed".into();
            }
        }
        self.save();
        Ok(())
    }
}

fn parse_files(status: &str, numstat: &str) -> Vec<ChangedFile> {
    // numstat -z: "added\tremoved\tpath\0" or for renames "a\tr\t\0from\0to\0".
    let mut counts: Vec<(String, Option<u64>, Option<u64>)> = Vec::new();
    let mut parts = numstat.split('\0').peekable();
    while let Some(p) = parts.next() {
        if p.is_empty() {
            continue;
        }
        let mut f = p.splitn(3, '\t');
        let (a, r, path) = (
            f.next().unwrap_or(""),
            f.next().unwrap_or(""),
            f.next().unwrap_or(""),
        );
        let path = if path.is_empty() {
            let _from = parts.next();
            parts.next().unwrap_or("").to_string()
        } else {
            path.to_string()
        };
        counts.push((path, a.parse().ok(), r.parse().ok()));
    }
    status
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let cols: Vec<&str> = l.split('\t').collect();
            let st = cols.first().copied().unwrap_or("").to_string();
            let (from, path) = if st.starts_with('R') || st.starts_with('C') {
                (
                    cols.get(1).map(|s| s.to_string()),
                    cols.get(2).copied().unwrap_or("").to_string(),
                )
            } else {
                (None, cols.get(1).copied().unwrap_or("").to_string())
            };
            let c = counts.iter().find(|(p, _, _)| *p == path);
            let (added, removed) = c.map(|(_, a, r)| (*a, *r)).unwrap_or((None, None));
            ChangedFile {
                binary: c.is_some() && added.is_none() && removed.is_none(),
                path,
                status: st,
                from,
                added,
                removed,
            }
        })
        .collect()
}

fn conflicting_files(detail: &str, cs: &ChangeSet) -> Vec<String> {
    let mut v: Vec<String> = cs
        .files
        .iter()
        .filter(|f| detail.contains(&f.path))
        .map(|f| f.path.clone())
        .collect();
    v.dedup();
    v
}
