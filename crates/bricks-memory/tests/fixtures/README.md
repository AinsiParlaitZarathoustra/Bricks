# Grafeo 0.5.44 migration fixture

`grafeo-0.5.44.grafeo` was generated with the published Grafeo **0.5.44**
engine, not with 0.6.0. It contains Bricks schema version 2, two legacy
memories (French Unicode text and floating-point confidence), a relation
with properties, and a native text index. It contains no user data.

To regenerate, create a temporary Rust project (outside the Bricks
workspace) with this dependency:

```toml
[dependencies]
grafeo = { version = "=0.5.44", default-features = false, features = ["lpg", "gql", "grafeo-file", "wal", "text-index"] }
```

Copy `generate_0_5_44.rs` to that project's `src/main.rs`, then run:

```sh
cargo run -- /absolute/path/to/a/new/grafeo-0.5.44.grafeo
```

Use a new output path. The generator closes the database and checkpoints
its WAL. Copy the resulting `.grafeo` file here. The integration test
copies these bytes into a temporary directory before opening with 0.6.0;
it checks the `.pre-0.6` backup, graph data, Bricks schema migration,
embedding, recall, and reopening without duplicates.
