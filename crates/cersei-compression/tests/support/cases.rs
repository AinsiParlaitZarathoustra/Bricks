//! The fixture corpus shared by `tests/corpus.rs` and `examples/measure.rs`.
//!
//! `fixtures/captured/` holds real outputs recorded on a development machine
//! (paths anonymised); `fixtures/reconstructed/` reproduces the documented
//! formats of tools that were not installed there (see its README).

pub struct Case {
    pub fixture: &'static str,
    pub command: &'static str,
    pub rule: &'static str,
    /// Lines (substrings) that must survive.
    pub keep: &'static [&'static str],
    /// Noise that must be gone.
    pub drop: &'static [&'static str],
}

pub const CASES: &[Case] = &[
    Case {
        fixture: "captured/cargo_test_middle_failure.txt",
        command: "cargo test --no-fail-fast",
        rule: "cargo-test",
        keep: &[
            "test middle_failure ... FAILED",
            "panicked at tests/f.rs:27:82",
            "parse failed: Err(ParseIntError { kind: InvalidDigit })",
            "test result: FAILED. 25 passed; 1 failed",
        ],
        drop: &["test case_13 ... ok", "Running tests/a.rs"],
    },
    Case {
        fixture: "captured/cargo_test_fail.txt",
        command: "cargo test",
        rule: "cargo-test",
        keep: &[
            "assertion `left == right` failed: arithmétique cassée",
            "débogage: état intermédiaire ✓",
            "called `Option::unwrap()` on a `None` value",
            "test result: FAILED. 11 passed; 2 failed; 1 ignored",
            "error: test failed, to rerun pass `--lib`",
        ],
        drop: &["test tests::t05 ... ok"],
    },
    Case {
        fixture: "captured/cargo_test_fail_ansi.txt",
        command: "cargo test --color always",
        rule: "cargo-test",
        keep: &["arithmétique cassée", "test result: FAILED"],
        drop: &["\x1b["],
    },
    Case {
        fixture: "captured/cargo_test_pass_long.txt",
        command: "cargo test -p cersei-provider",
        rule: "cargo-test",
        keep: &["test result: ok. 128 passed"],
        drop: &["Doc-tests cersei_provider"],
    },
    Case {
        fixture: "captured/cargo_clippy.txt",
        command: "cargo clippy",
        rule: "cargo-build",
        keep: &[
            "warning: unneeded `return` statement",
            "help: remove `return`",
            "warning: calls to `push` immediately after creation",
            "generated 2 warnings",
        ],
        drop: &["Checking rs v0.1.0"],
    },
    Case {
        fixture: "captured/cargo_build_error.txt",
        command: "cargo build",
        rule: "cargo-build",
        keep: &[
            "error[E0308]: mismatched types",
            "expected `i32`, found `&str`",
            "error[E0425]: cannot find function `undefined_fn` in this scope",
            "could not compile `rs` (lib) due to 2 previous errors",
        ],
        drop: &["Compiling rs v0.1.0"],
    },
    Case {
        fixture: "captured/cargo_build_error.json.txt",
        command: "cargo build --message-format=json",
        rule: "cargo-build",
        keep: &[
            "error[E0308]: mismatched types",
            "error[E0425]: cannot find function `undefined_fn` in this scope",
            "build finished: FAILED",
        ],
        drop: &["\"reason\":\"compiler-artifact\""],
    },
    Case {
        fixture: "captured/cargo_audit.txt",
        command: "cargo audit --no-fetch",
        rule: "cargo-audit",
        keep: &[
            "RUSTSEC-2026-0119",
            "RUSTSEC-2026-0118",
            "Solution:  No fixed upgrade is available!",
            "RUSTSEC-2026-0002",
            "error: 2 vulnerabilities found!",
        ],
        drop: &["Loaded 1239 security advisories", "Scanning Cargo.lock"],
    },
    Case {
        fixture: "captured/go_test.txt",
        command: "go test ./...",
        rule: "go-test",
        keep: &[
            "--- FAIL: TestAddWrong",
            "Add(2, 2) = 4; want 5 — résultat inattendu",
            "panic: runtime error: integer divide by zero",
            "calc/calc_test.go:19",
            "FAIL\texample.com/gomod/calc",
        ],
        drop: &[],
    },
    Case {
        fixture: "captured/go_test_v.txt",
        command: "go test -v ./...",
        rule: "go-test",
        keep: &["--- FAIL: TestAddWrong", "want 5", "integer divide by zero"],
        drop: &["=== RUN   TestAdd/case#07", "--- PASS: TestAdd/case#03"],
    },
    Case {
        fixture: "captured/go_test_json.txt",
        command: "go test -json ./...",
        rule: "go-test",
        keep: &["--- FAIL: TestAddWrong", "want 5", "integer divide by zero"],
        drop: &["\"Action\":\"run\""],
    },
    Case {
        fixture: "captured/go_test_build_fail.txt",
        command: "go test ./...",
        rule: "go-test",
        keep: &[
            "util/util.go:4:28: undefined: undefinedThing",
            "[build failed]",
        ],
        drop: &[],
    },
    Case {
        fixture: "captured/pytest_fail.txt",
        command: "uv run pytest",
        rule: "pytest",
        keep: &[
            "KeyError: 'clé'",
            "AssertionError: somme inattendue",
            "FAILED test_calc.py::test_keyerror",
            "2 failed, 30 passed, 1 skipped",
        ],
        drop: &["platform darwin", "rootdir:"],
    },
    Case {
        fixture: "captured/pytest_v_fail.txt",
        command: "python3 -m pytest -v",
        rule: "pytest",
        keep: &["KeyError: 'clé'", "FAILED test_calc.py::test_assert_msg"],
        drop: &["test_calc.py::test_add[7] PASSED"],
    },
    Case {
        fixture: "captured/uv_run_traceback.txt",
        command: "uv run --no-project python -c 'import json'",
        rule: "generic",
        keep: &[
            "Traceback (most recent call last):",
            "json.decoder.JSONDecodeError",
        ],
        drop: &[],
    },
    Case {
        fixture: "captured/npm_test_fail.txt",
        command: "npm test",
        rule: "npm",
        keep: &[
            "AssertionError [ERR_ASSERTION]: valeur inattendue",
            "test.js:4:8",
        ],
        drop: &[],
    },
    Case {
        fixture: "captured/yarn_test_fail.txt",
        command: "yarn test",
        rule: "yarn",
        keep: &[
            "AssertionError [ERR_ASSERTION]: valeur inattendue",
            "error Command failed with exit code 1.",
        ],
        drop: &["info Visit https://yarnpkg.com"],
    },
    Case {
        fixture: "captured/pnpm_test_fail.txt",
        command: "pnpm test",
        rule: "pnpm",
        keep: &["AssertionError [ERR_ASSERTION]", "ELIFECYCLE"],
        drop: &[],
    },
    Case {
        fixture: "reconstructed/vitest_fail.txt",
        command: "npx vitest run",
        rule: "vitest",
        keep: &[
            "FAIL  src/math.test.ts > add > handles négatifs",
            "AssertionError: expected -2 to be 2",
            "src/math.test.ts:9:24",
            "Tests  1 failed | 242 passed (243)",
        ],
        drop: &["✓ src/feature17.test.ts"],
    },
    Case {
        fixture: "reconstructed/eslint.txt",
        command: "pnpm exec eslint src",
        rule: "eslint",
        keep: &[
            "/work/app/src/api/client.ts",
            "'unusedHelper' is defined but never used",
            "'props' is missing in props validation",
            "✖ 4 problems (2 errors, 2 warnings)",
        ],
        drop: &[],
    },
    Case {
        fixture: "reconstructed/vite_build.txt",
        command: "vite build",
        rule: "vite",
        keep: &["✓ built in 4.21s", "(!) Some chunks are larger than 500 kB"],
        drop: &["dist/assets/chunk-17A9f3.js", "rendering chunks..."],
    },
    Case {
        fixture: "reconstructed/vite_build_error.txt",
        command: "npx vite build",
        rule: "vite",
        keep: &[
            "error during build:",
            "Rollup failed to resolve import \"@/missing/module\"",
        ],
        drop: &[],
    },
    Case {
        fixture: "reconstructed/next_build.txt",
        command: "next build",
        rule: "next-build",
        keep: &["✓ Compiled successfully", "First Load JS shared by all"],
        drop: &["/page-12 ", "Generating static pages (6/24)"],
    },
    Case {
        fixture: "reconstructed/next_build_error.txt",
        command: "npx next build",
        rule: "next-build",
        keep: &[
            "Failed to compile.",
            "./app/page.tsx:3:7",
            "Type error: Type 'number' is not assignable",
        ],
        drop: &[],
    },
    Case {
        fixture: "reconstructed/turbo_fail.txt",
        command: "turbo run build",
        rule: "turbo",
        keep: &["error TS2322", "Failed:    web#build", "ERROR  run failed"],
        drop: &["Packages in scope"],
    },
    Case {
        fixture: "reconstructed/bun_test_fail.txt",
        command: "bun test",
        rule: "bun-test",
        keep: &[
            "✗ math > divides by zero",
            "Received function did not throw",
            "1 fail",
        ],
        drop: &["✓ math > case 12 "],
    },
    Case {
        fixture: "reconstructed/bun_install.txt",
        command: "bun install",
        rule: "bun-install",
        keep: &["312 packages installed"],
        drop: &["package-13@"],
    },
    Case {
        fixture: "reconstructed/ruff_check.txt",
        command: "uv run ruff check .",
        rule: "ruff",
        keep: &[
            "F401 [*] `os` imported but unused",
            "E722 Do not use bare `except`",
            "Found 2 errors.",
        ],
        drop: &[],
    },
    Case {
        fixture: "reconstructed/mypy.txt",
        command: "mypy src",
        rule: "mypy",
        keep: &[
            "models.py:12: error: Incompatible types",
            "models.py:30: note:",
            "Found 2 errors in 1 file",
        ],
        drop: &[],
    },
    Case {
        fixture: "reconstructed/uv_sync.txt",
        command: "uv sync",
        rule: "uv",
        keep: &["Installed 41 packages in 88ms"],
        drop: &["pkg-20==", "Resolved 41 packages"],
    },
    Case {
        fixture: "reconstructed/uv_resolve_error.txt",
        command: "uv add fastapi==99.0",
        rule: "uv",
        keep: &[
            "× No solution found when resolving dependencies",
            "requirements are unsatisfiable",
        ],
        drop: &[],
    },
    Case {
        fixture: "reconstructed/kubectl_apply.txt",
        command: "kubectl -n prod apply -f k8s/",
        rule: "kubectl-change",
        keep: &[
            "deployment.apps/web configured",
            "Error from server (Invalid)",
        ],
        drop: &["configmap/app-config-20 unchanged"],
    },
    Case {
        fixture: "reconstructed/terraform_plan.txt",
        command: "terraform plan -out=tf.plan",
        rule: "terraform-plan",
        keep: &[
            "# aws_instance.web will be updated in-place",
            "\"Name\" = \"ancien\" -> \"nouveau\"",
            "Plan: 0 to add, 1 to change, 0 to destroy.",
            "│ Error: Invalid reference",
            "on main.tf line 12",
            "│ Warning: Argument is deprecated",
        ],
        drop: &["Refreshing state..."],
    },
    Case {
        fixture: "reconstructed/gh_pr_checks.txt",
        command: "gh pr checks 42",
        rule: "gh-pr-checks",
        keep: &["lint\tfail\t42s", "deploy-preview\tpending"],
        drop: &["test (7)\tpass"],
    },
    Case {
        fixture: "reconstructed/docker_compose_up.txt",
        command: "docker compose -f compose.yml up",
        rule: "docker-compose",
        keep: &[
            "Error: connect ECONNREFUSED 172.18.0.3:6379",
            "web-1 exited with code 1",
            "repeated",
        ],
        drop: &["✔ Container app-db-1"],
    },
];

pub fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}
