//! The skill store: installs, creations and removals of copies in
//! `<bricks_home>/skills`, what the other folders of the machine are left
//! alone, and what a refused or interrupted installation leaves behind.
//! Everything happens in temporary folders.

use bricks_skills::store::{Field, INSTALL_FILE, InstallOrigin, SkillStore, StoreError};
use bricks_skills::{SkillAccess, SkillsService, parse_document};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Every entry under `root` (relative path to kind and content digest),
/// symbolic links recorded by target, nothing followed.
fn digest(root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if !root.exists() {
        return out;
    }
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        for entry in std::fs::read_dir(root.join(&rel)).unwrap() {
            let entry = entry.unwrap();
            let child = rel.join(entry.file_name());
            let key = child.to_string_lossy().replace('\\', "/");
            let meta = std::fs::symlink_metadata(entry.path()).unwrap();
            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(entry.path()).unwrap();
                out.insert(key, format!("link:{}", target.display()));
            } else if meta.is_dir() {
                out.insert(key, "dir".into());
                stack.push(child);
            } else if meta.is_file() {
                let bytes = std::fs::read(entry.path()).unwrap();
                out.insert(key, format!("file:{}", sha(&bytes)));
            } else {
                out.insert(key, "special".into());
            }
        }
    }
    out
}

/// The digest of an installed copy, without its hidden install record.
fn copy_digest(dir: &Path) -> BTreeMap<String, String> {
    let mut d = digest(dir);
    d.remove(INSTALL_FILE);
    d
}

/// Names of the entries directly in `dir` (hidden ones included).
fn entries_of(dir: &Path) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// True when the store has no work left over from an installation.
fn staging_is_empty(store_dir: &Path) -> bool {
    let staging = store_dir.join(".staging");
    !staging.exists() || std::fs::read_dir(&staging).unwrap().next().is_none()
}

fn names(access: &SkillAccess) -> Vec<String> {
    access
        .catalog()
        .entries
        .into_iter()
        .map(|e| e.name)
        .collect()
}

fn write_folder(dir: &Path, name: &str, body: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let text = format!("---\nname: {name}\ndescription: Does {name}.\n---\n\n{body}\n");
    std::fs::write(dir.join("SKILL.md"), text).unwrap();
}

fn mkfifo(path: &Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "mkfifo failed");
}

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

// 1. Other clients' folders are neither read nor changed.
#[test]
fn bricks_sources_ignore_other_clients_folders() {
    let tmp = tmp();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    let store_dir = tmp.path().join("store");
    for (base, client, name) in [
        (&home, ".claude", "x"),
        (&home, ".agents", "y"),
        (&project, ".claude", "z"),
        (&project, ".agents", "w"),
    ] {
        write_folder(
            &base.join(client).join("skills").join(name),
            name,
            "Valid body.",
        );
    }
    let before_home = digest(&home);
    let before_project = digest(&project);

    let access = SkillAccess::new(Arc::new(SkillsService::standard(
        &project,
        Some(&store_dir),
    )));
    assert_eq!(names(&access), vec!["create-skills"]);

    assert_eq!(digest(&home), before_home, "home folders changed");
    assert_eq!(digest(&project), before_project, "project folders changed");
}

// 2. A created skill is listed at once, and its document round-trips.
#[test]
fn created_skill_is_listed_without_reload_and_its_document_round_trips() {
    let tmp = tmp();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let store_dir = tmp.path().join("store");
    let access = SkillAccess::new(Arc::new(SkillsService::standard(
        &project,
        Some(&store_dir),
    )));
    assert_eq!(names(&access), vec!["create-skills"]);

    let store = SkillStore::new(&store_dir);
    let description = "Says \"hi\": first line\nsecond line --- still text";
    let installed = store
        .create(
            "hello-world",
            description,
            "Greet the user warmly.\n\nWhen asked, say hello.",
            InstallOrigin::Created,
        )
        .unwrap();
    assert_eq!(installed.name, "hello-world");

    // The same service, no reload call.
    let catalog = access.catalog();
    let entry = catalog
        .entries
        .iter()
        .find(|e| e.name == "hello-world")
        .expect("hello-world is listed at once");
    assert_eq!(entry.description, description);

    let text = std::fs::read_to_string(installed.dir.join("SKILL.md")).unwrap();
    let doc = parse_document(&text, Some("hello-world")).unwrap();
    assert_eq!(doc.name, "hello-world");
    assert_eq!(doc.description, description);
}

// 3. Each refusal names its field or reason.
#[test]
fn refusals_name_the_exact_field_or_reason() {
    let tmp = tmp();
    let store_dir = tmp.path().join("store");
    let store = SkillStore::new(&store_dir);

    for bad in [
        "Bad_Name",
        "bad--name",
        "under_score",
        "UPPER",
        &"a".repeat(65),
    ] {
        match store.create(bad, "Does things.", "Do it.", InstallOrigin::Created) {
            Err(StoreError::Invalid {
                field: Field::Name, ..
            }) => {}
            other => panic!("name `{bad}`: expected Invalid(Name), got {other:?}"),
        }
    }
    match store.create("fine-name", "   ", "Do it.", InstallOrigin::Created) {
        Err(StoreError::Invalid {
            field: Field::Description,
            ..
        }) => {}
        other => panic!("empty description: expected Invalid(Description), got {other:?}"),
    }
    match store.create("fine-name", "Does things.", " \n ", InstallOrigin::Created) {
        Err(StoreError::Invalid {
            field: Field::Instructions,
            ..
        }) => {}
        other => panic!("empty instructions: expected Invalid(Instructions), got {other:?}"),
    }
    match store.create(
        "create-skills",
        "Does things.",
        "Do it.",
        InstallOrigin::Created,
    ) {
        Err(StoreError::Reserved { name }) => assert_eq!(name, "create-skills"),
        other => panic!("create-skills: expected Reserved, got {other:?}"),
    }

    let forked = tmp.path().join("src").join("forked");
    std::fs::create_dir_all(&forked).unwrap();
    std::fs::write(
        forked.join("SKILL.md"),
        "---\nname: forked\ndescription: Runs apart.\ncontext: fork\n---\n\nBody.\n",
    )
    .unwrap();
    match store.install(&forked, InstallOrigin::Imported) {
        Err(StoreError::Incompatible { name, .. }) => assert_eq!(name, "forked"),
        other => panic!("context: fork: expected Incompatible, got {other:?}"),
    }

    let reserved = tmp.path().join("src").join("create-skills");
    write_folder(&reserved, "create-skills", "Imitation.");
    match store.install(&reserved, InstallOrigin::Imported) {
        Err(StoreError::Reserved { name }) => assert_eq!(name, "create-skills"),
        other => panic!("folder create-skills: expected Reserved, got {other:?}"),
    }

    // Second installation of the same name: refused, the first copy kept.
    let first = tmp.path().join("src").join("dup");
    write_folder(&first, "dup", "First version.");
    store.install(&first, InstallOrigin::Imported).unwrap();
    let kept = copy_digest(&store_dir.join("dup"));
    let second = tmp.path().join("other").join("dup");
    write_folder(&second, "dup", "Second version, different.");
    match store.install(&second, InstallOrigin::Imported) {
        Err(StoreError::Exists { name }) => assert_eq!(name, "dup"),
        other => panic!("second install: expected Exists, got {other:?}"),
    }
    assert_eq!(
        copy_digest(&store_dir.join("dup")),
        kept,
        "first copy changed"
    );
}

// 4. A refused or interrupted installation leaves nothing in the store.
#[test]
fn refused_or_interrupted_installations_leave_no_partial_folder() {
    let tmp = tmp();
    let store_dir = tmp.path().join("store");
    let store = SkillStore::new(&store_dir);
    let src_root = tmp.path().join("src");

    // Start with one real skill so the store is not empty.
    let base = src_root.join("base");
    write_folder(&base, "base", "Base body.");
    store.install(&base, InstallOrigin::Imported).unwrap();
    let before = entries_of(&store_dir);

    let check = |what: &str| {
        assert_eq!(entries_of(&store_dir), before, "{what}: store changed");
        assert!(staging_is_empty(&store_dir), "{what}: staging not empty");
    };

    let _ = store.create("Bad_Name", "Does things.", "Do it.", InstallOrigin::Created);
    check("invalid name");
    let _ = store.create("fine-name", "  ", "Do it.", InstallOrigin::Created);
    check("empty description");
    let _ = store.create("fine-name", "Does things.", "  ", InstallOrigin::Created);
    check("empty instructions");

    let forked = src_root.join("forked");
    std::fs::create_dir_all(&forked).unwrap();
    std::fs::write(
        forked.join("SKILL.md"),
        "---\nname: forked\ndescription: Runs apart.\ncontext: fork\n---\n\nBody.\n",
    )
    .unwrap();
    let _ = store.install(&forked, InstallOrigin::Imported);
    check("incompatible folder");

    let reserved = src_root.join("create-skills");
    write_folder(&reserved, "create-skills", "Imitation.");
    let _ = store.install(&reserved, InstallOrigin::Imported);
    check("reserved folder");

    // A file that cannot be read stops the copy half way.
    use std::os::unix::fs::PermissionsExt;
    let unreadable = src_root.join("unreadable");
    write_folder(&unreadable, "unreadable", "Partly copied.");
    let secret = unreadable.join("secret.md");
    std::fs::write(&secret, "hidden").unwrap();
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();
    if unsafe { libc::geteuid() } == 0 {
        // Root reads any file: the case cannot happen here.
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644)).unwrap();
        eprintln!("skipped the unreadable-file case as root");
    } else {
        let result = store.install(&unreadable, InstallOrigin::Imported);
        // Refused (the copy reports it as an unsafe entry); what matters is
        // that nothing of it is left behind, checked below.
        assert!(result.is_err(), "unreadable file: installed anyway");
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644)).unwrap();
        check("unreadable file");
    }
}

// 5. A large import: every file copied, identical bytes and layout, source untouched.
#[test]
fn large_import_copies_every_file_with_identical_bytes_and_layout() {
    let tmp = tmp();
    let store_dir = tmp.path().join("store");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let store = SkillStore::new(&store_dir);

    let src = tmp.path().join("src").join("big-skill");
    write_folder(&src, "big-skill", "Use the bundled assets.");
    for i in 0..100u32 {
        let bytes: Vec<u8> = (0..(50 + i))
            .map(|b| (b as u8).wrapping_mul((i as u8) ^ 0x5a))
            .collect();
        let dir = src.join("assets");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("file_{i:03}.bin")), bytes).unwrap();
    }
    let deep_dir = src.join("references/a/b/c/d");
    std::fs::create_dir_all(&deep_dir).unwrap();
    std::fs::write(deep_dir.join("deep.md"), "deep content\n").unwrap();
    std::fs::write(src.join("LICENSE"), "license text\n").unwrap();
    std::fs::write(src.join("LICENSE.txt"), "license txt\n").unwrap();
    // Not copied: the system and version-control leftovers.
    std::fs::write(src.join(".DS_Store"), "junk").unwrap();
    std::fs::create_dir_all(src.join(".git")).unwrap();
    std::fs::write(src.join(".git/config"), "[core]\n").unwrap();

    let before = digest(&src);
    let installed = store.install(&src, InstallOrigin::Imported).unwrap();
    assert_eq!(
        installed.files, 104,
        "SKILL.md, 100 assets, deep.md, LICENSE, LICENSE.txt"
    );

    let expected: BTreeMap<_, _> = before
        .iter()
        .filter(|(k, _)| !k.starts_with(".git") && k.as_str() != ".DS_Store")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    assert_eq!(
        copy_digest(&installed.dir),
        expected,
        "copy differs from source"
    );
    assert_eq!(digest(&src), before, "source folder changed");

    let service = SkillsService::standard(&project, Some(&store_dir));
    let access = SkillAccess::new(Arc::new(service));
    let deep = access
        .resource("big-skill", "references/a/b/c/d/deep.md")
        .expect("deep resource resolves");
    assert_eq!(std::fs::read(deep).unwrap(), b"deep content\n");
    let asset = access
        .resource("big-skill", "assets/file_099.bin")
        .expect("asset resolves");
    assert_eq!(
        std::fs::read(asset).unwrap(),
        std::fs::read(src.join("assets/file_099.bin")).unwrap()
    );
}

// 6. Links and special files are refused, and nothing is installed.
#[test]
fn symbolic_links_and_fifos_are_refused_and_nothing_is_installed() {
    let tmp = tmp();
    let store_dir = tmp.path().join("store");
    let store = SkillStore::new(&store_dir);
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "not yours").unwrap();

    // A link to a file outside the folder.
    let file_link = tmp.path().join("src").join("file-link");
    write_folder(&file_link, "file-link", "Body.");
    std::os::unix::fs::symlink(outside.join("secret.txt"), file_link.join("notes.md")).unwrap();
    match store.install(&file_link, InstallOrigin::Imported) {
        Err(StoreError::Unsafe { path, .. }) => assert!(path.contains("notes.md"), "path: {path}"),
        other => panic!("file link: expected Unsafe, got {other:?}"),
    }

    // A link to a folder outside.
    let dir_link = tmp.path().join("src").join("dir-link");
    write_folder(&dir_link, "dir-link", "Body.");
    std::os::unix::fs::symlink(&outside, dir_link.join("refs")).unwrap();
    match store.install(&dir_link, InstallOrigin::Imported) {
        Err(StoreError::Unsafe { path, .. }) => assert!(path.contains("refs"), "path: {path}"),
        other => panic!("dir link: expected Unsafe, got {other:?}"),
    }

    // A named pipe.
    let fifo = tmp.path().join("src").join("fifo-skill");
    write_folder(&fifo, "fifo-skill", "Body.");
    mkfifo(&fifo.join("pipe"));
    match store.install(&fifo, InstallOrigin::Imported) {
        Err(StoreError::Unsafe { path, .. }) => assert!(path.contains("pipe"), "path: {path}"),
        other => panic!("fifo: expected Unsafe, got {other:?}"),
    }

    assert!(
        entries_of(&store_dir).is_empty(),
        "store holds {:?}",
        entries_of(&store_dir)
    );
    assert!(staging_is_empty(&store_dir));
}

// 7. Removal only touches copies in the store.
#[test]
fn removal_only_touches_store_copies() {
    let tmp = tmp();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let store_dir = tmp.path().join("store");
    let store = SkillStore::new(&store_dir);

    let src = tmp.path().join("src").join("gone-soon");
    write_folder(&src, "gone-soon", "Body.");
    store.install(&src, InstallOrigin::Imported).unwrap();

    let access = SkillAccess::new(Arc::new(SkillsService::standard(
        &project,
        Some(&store_dir),
    )));
    assert!(names(&access).iter().any(|n| n == "gone-soon"));
    assert_eq!(store.remove("gone-soon").unwrap(), "gone-soon");
    assert!(
        !names(&access).iter().any(|n| n == "gone-soon"),
        "removed skill still listed without reload"
    );

    assert_eq!(
        store.remove("create-skills"),
        Err(StoreError::Reserved {
            name: "create-skills".into()
        })
    );

    write_folder(
        &home.join(".claude/skills/only-claude"),
        "only-claude",
        "Body.",
    );
    let before = digest(&home);
    assert_eq!(
        store.remove("only-claude"),
        Err(StoreError::NotInstalled {
            name: "only-claude".into()
        })
    );
    assert_eq!(digest(&home), before, "sibling folder changed");
}

// 8. A reinstalled skill is a new identity: the old activation no longer verifies.
#[test]
fn reinstalled_skill_is_a_new_identity() {
    let tmp = tmp();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let store_dir = tmp.path().join("store");
    let store = SkillStore::new(&store_dir);

    let src = tmp.path().join("src").join("s");
    write_folder(&src, "s", "Body of s.");
    let first = store.install(&src, InstallOrigin::Imported).unwrap();

    let access = SkillAccess::new(Arc::new(SkillsService::standard(
        &project,
        Some(&store_dir),
    )));
    let old = access.activate("s").unwrap();
    assert_eq!(old.source, format!("store:{}", first.id));

    store.remove("s").unwrap();
    let second = store.install(&src, InstallOrigin::Imported).unwrap();
    assert_ne!(first.id, second.id, "install ids must differ");

    let new = access.activate("s").unwrap();
    assert_ne!(
        new.source, old.source,
        "reinstalled copy keeps the old source"
    );
    assert_eq!(new.revision, old.revision, "same text, same revision");

    let err = access
        .verify("s", &old.source, &old.revision)
        .expect_err("old activation must not verify");
    assert!(err.contains("now comes from"), "message: {err}");
}

// 9. Two installers racing for one name: exactly one wins, the copy is whole.
#[test]
fn racing_installers_of_one_name_leave_one_whole_copy() {
    for round in 0..20 {
        let tmp = tmp();
        let store_dir = tmp.path().join("store");
        let a = tmp.path().join("a").join("same");
        let b = tmp.path().join("b").join("same");
        write_folder(&a, "same", "Version A.");
        write_folder(&b, "same", "Version B, a little longer than A.");
        let store = Arc::new(SkillStore::new(&store_dir));
        let barrier = Arc::new(Barrier::new(2));

        let handles: Vec<_> = [a.clone(), b.clone()]
            .into_iter()
            .map(|src| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    store.install(&src, InstallOrigin::Imported)
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        let oks: Vec<usize> = results
            .iter()
            .enumerate()
            .filter(|(_, r)| r.is_ok())
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            oks.len(),
            1,
            "round {round}: exactly one install must succeed"
        );
        match &results[1 - oks[0]] {
            Err(StoreError::Exists { name }) => assert_eq!(name, "same"),
            other => panic!("round {round}: loser expected Exists, got {other:?}"),
        }

        let winner = if oks[0] == 0 { &a } else { &b };
        assert_eq!(
            copy_digest(&store_dir.join("same")),
            digest(winner),
            "round {round}: installed copy is not the whole winning folder"
        );
        assert!(
            staging_is_empty(&store_dir),
            "round {round}: staging not empty"
        );
    }
}
