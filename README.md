  
## Table Functions

- `git(repo_or_json: VARCHAR)`
  - **Parameters**
    - `repo_or_json: VARCHAR`
      - Either a plain path/URL to a Git repository (e.g. `'.'`, `'/path/to/repo'`, `https://github.com/owner/repo.git`, `git@github.com:owner/repo.git`), or a JSON object string with advanced options:
        - `repo: string` – repository path or URL (required when using JSON)
        - `include_patches: bool` – default `true`. When `false`, `file_changes` is an empty array and `files_changed` is `0`, but per-commit line metrics are still computed.
        - `detect_renames: bool` – default `true`. Enable/disable rename detection when computing file changes.
        - `detect_copies: bool` – default `true`. Enable/disable copy detection when computing file changes.
        - `max_diff_bytes: u64` – maximum bytes per file blob considered for diffing (default `1_000_000`, minimum `1_000`). Larger files are treated as binary/no-diff for metrics.
        - `since: "YYYY-MM-DD"` – lower bound on commit time (inclusive).
        - `until: "YYYY-MM-DD"` – upper bound on commit time (inclusive).
        - `since_ts: i64` – lower bound Unix timestamp (seconds); overrides `since` if present.
        - `until_ts: i64` – upper bound Unix timestamp (seconds); overrides `until` if present.
  - **Result columns**
    - `commit_hash: VARCHAR` – commit hash (40-char hex).
    - `message: VARCHAR` – commit message (subject + body as a single string).
    - `author_name: VARCHAR` – author display name, or `<unknown>`.
    - `author_email: VARCHAR` – author email, or `<unknown>`.
    - `commit_time: BIGINT` – commit time as Unix epoch seconds.
    - `parents: VARCHAR` – JSON array of parent commit hashes (e.g. `["<hash>", ...]`).
    - `lines_added: BIGINT` – effective added lines (after matching modifications).
    - `lines_modified: BIGINT` – effective modified lines (in-place line changes).
    - `lines_deleted: BIGINT` – effective deleted lines.
    - `files_changed: BIGINT` – number of files touched in the commit.
    - `is_merge: BOOLEAN` – `true` if the commit has more than one parent.
    - `file_changes: VARCHAR` – JSON array of per-file change objects, empty when `include_patches=false`. Each object has keys like:
      - `new_path: string`
      - `old_path: string | null`
      - `kind: "added" | "deleted" | "modified" | "renamed" | "copied"`
      - `additions: number`
      - `deletions: number`
      - `hunks: number`
      - `methods: number`
      - `is_binary: bool`

- `git_branches(json: VARCHAR)`
  - **Parameters**
    - `json: VARCHAR`
      - Either a plain repo path/URL, or a JSON object string:
        - `repo: string` – repository path or URL.
        - `since: "YYYY-MM-DD"` – optional lower bound on commit time.
        - `until: "YYYY-MM-DD"` – optional upper bound on commit time.
        - `since_ts: i64` – optional lower bound Unix timestamp.
        - `until_ts: i64` – optional upper bound Unix timestamp.
  - **Result columns**
    - `commit_time: BIGINT` – commit time as Unix epoch seconds.
    - `branch: VARCHAR` – fully qualified ref name (e.g. `refs/heads/main`, `refs/remotes/origin/main`).
    - `value: BIGINT` – always `1`, useful for aggregations (e.g. activity histograms per branch).

- `git_feature_metrics(json: VARCHAR)`
  - **Parameters**
    - `json: VARCHAR`
      - Either a plain repo path/URL, or a JSON object string:
        - `repo: string` – repository path or URL.
        - `since: "YYYY-MM-DD"` / `until: "YYYY-MM-DD"` – optional date bounds.
        - `since_ts: i64` / `until_ts: i64` – optional Unix timestamp bounds.
  - **Result columns**
    - `feature: VARCHAR` – detected feature key from commit message (e.g. `ABC-123`).
    - `commit_time: BIGINT` – commit time as Unix epoch seconds.
    - `lines_added: BIGINT` – lines added in the commit.
    - `lines_deleted: BIGINT` – lines deleted in the commit.
    - `lines_changed: BIGINT` – total changed lines (`added + deleted + modified`).
    - `net_lines_changed: BIGINT` – net additions (`added - deleted`).
    - `merges: BIGINT` – `1` if the commit is a merge, else `0`.

- `git_tags(json: VARCHAR)`
  - **Parameters**
    - `json: VARCHAR`
      - Either a plain repo path/URL, or a JSON object string:
        - `repo: string` – repository path or URL.
  - **Result columns**
    - `tag: VARCHAR` – full tag ref name (e.g. `refs/tags/v1.2.3`).
    - `target: VARCHAR` – resolved target object ID (usually the tagged commit hash).
    - `is_annotated: BOOLEAN` – `true` if the tag object is annotated.
    - `tagger_name: VARCHAR` – currently left as an empty string for all tags.
    - `tagger_email: VARCHAR` – currently left as an empty string for all tags.
    - `tag_time: BIGINT` – currently `0` for all tags.
    - `message: VARCHAR` – currently an empty string for all tags.

# Git2Duck DuckDB Extension (Rust)

A DuckDB extension written in Rust that provides Git repository analysis using `gix`.

## Features

- **Native Rust**: Uses `gix` for Git operations (pure Rust, no libgit2 dependency)
- **FFI Integration**: Exports C-compatible functions for DuckDB
- **Memory Safe**: Leverages Rust's safety guarantees

## Building

### Prerequisites

This project uses the Rust duckdb crate with the `bundled` and `vtab-loadable` features, so you do not need system DuckDB libraries for normal builds.

Required tools:

```bash
# Rust toolchain
rustup update

# Optional: profiling tools for flame graphs (Linux)
sudo apt-get update && sudo apt-get install -y \
  linux-tools-common linux-tools-generic linux-tools-$(uname -r)

# cargo-flamegraph (user install)
cargo install flamegraph
```

### Build the Extension

```bash
# Build the Rust library and extension (with DuckDB loadable entrypoint)
cargo build --release --features ext
```

This produces `target/release/libgit2duck.so` (or `.dylib` on macOS, `.dll` on Windows).

### Supported Platforms

CI builds and tests the extension on **Linux** (`linux_amd64` and `linux_arm64`).
macOS and Windows are not built in CI at the moment; the code is portable and can be built locally.
If you need prebuilt binaries for another platform, please open an issue.

### Building on macOS (and Windows)

The steps are the same as on Linux; only the shared-library name and the DuckDB platform string differ.

```bash
# 1. Build the shared library with the loadable entrypoint
cargo build --release --features ext
#    -> target/release/libgit2duck.dylib   (macOS)
#    -> target/release/git2duck.dll        (Windows)

# 2. Clone the packaging tools (one-time)
git clone --depth 1 https://github.com/duckdb/extension-ci-tools.git

# 3. Append the DuckDB extension metadata
python3 extension-ci-tools/scripts/append_extension_metadata.py \
  --library-file target/release/libgit2duck.dylib \
  --extension-name git2duck \
  --out-file target/release/git2duck.duckdb_extension \
  --duckdb-platform osx_arm64 \
  --duckdb-version v1.2.0 \
  --extension-version v0.1.0 \
  --abi-type C_STRUCT
```

Pick the platform string that matches your machine: `osx_arm64` (Apple Silicon), `osx_amd64` (Intel Mac),
`windows_amd64` or `windows_arm64`. `--duckdb-version` is the DuckDB C API version the extension targets
(`v1.2.0`, see the packaging section below). Windows builds additionally need the MSVC toolchain; at the time of
writing the bundled DuckDB did not compile with the newest Visual Studio, so use Visual Studio 2022.

Load it in DuckDB with `allow_unsigned_extensions` enabled, as described in the Usage section.

### Package the DuckDB Extension Artifact (optional)

If you want DuckDB to load the packaged `*.duckdb_extension` file by default (instead of the shared library),
package the artifact using DuckDB's extension-ci-tools script. Re-run this step whenever you change the set of
registered table functions or the entrypoint, so the packaged metadata reflects the current implementation.

Steps:

1. Build the shared library with the loadable entrypoint:

   ```bash
   cargo build --release --features ext
   ```

2. Clone the packaging tools (one-time):

   ```bash
   git clone https://github.com/duckdb/extension-ci-tools.git /tmp/extension-ci-tools
   ```

3. Package using the helper script (recommended):

```bash
DUCKDB_VERSION=v1.2.0 DUCKDB_PLATFORM=linux_amd64 bash package.sh
```

4. Verify loading in DuckDB CLI:

   ```sql
   -- One of these should work depending on your platform and paths
   LOAD 'target/release/git2duck.duckdb_extension';
   -- or
   LOAD 'target/release/libgit2duck.so';

   -- Should expose all table functions
   SELECT * FROM git('{"repo": "."}') LIMIT 1;
   SELECT 1 FROM git_branches('{"repo": "."}') LIMIT 0;
   SELECT 1 FROM git_feature_metrics('{"repo": "."}') LIMIT 0;
   SELECT 1 FROM git_tags('{"repo": "."}') LIMIT 0;
   ```

Troubleshooting:
- If DuckDB still loads an older packaged artifact, delete stale `*.duckdb_extension` files from `target/release[/deps]`
  or set `GIT2DUCK_EXT_PATH` to point directly to the rebuilt shared library (`libgit2duck.so`).
- Our app logs which path was attempted and successfully loaded.

## Usage

### Option 1: Load as Extension (with loadable entrypoint)

```sql
-- Load the extension (Release)
LOAD './target/release/libgit2duck.so';

-- Load the extension (Debug)
LOAD './target/debug/libgit2duck.so';

-- Query commits
SELECT * FROM git('"/path/to/repo"');
```

### Option 2: Use via duckdb-rs in-process (tests/bench)

Benchmarks and tests register the table function in-process without loading the shared library:

```rust
let con: duckdb::Connection = duckdb::Connection::open_in_memory()?;

// Register individual table functions in-process
git2duck::register_git(&con)?;                // exposes git(...)
git2duck::register_git_branches(&con)?;       // exposes git_branches(...)
git2duck::register_git_feature_metrics(&con)?; // exposes git_feature_metrics(...)
git2duck::register_git_tags(&con)?;           // exposes git_tags(...)

// Now you can run:
// SELECT * FROM git(?);
// SELECT * FROM git_branches(?);
// SELECT * FROM git_feature_metrics(?);
// SELECT * FROM git_tags(?);
```

### Benchmarks & Profiling

See BENCHMARKS.md for detailed instructions on running performance benchmarks and generating flame graphs. Required tools are listed above (`linux-tools-*` for perf and `cargo-flamegraph`).

## Architecture

```
┌─────────────────────────────────────┐
│  DuckDB SQL Query                   │
│  SELECT * FROM git('{"repo": "."}') │
└──────────────┬──────────────────────┘
               │
               ▼
┌─────────────────────────────────────┐
│  DuckDB Extension Loader            │
│  Loads libgit2duck.so               │
└──────────────┬──────────────────────┘
               │
               ▼
┌─────────────────────────────────────────────────────────────┐
│  Rust FFI Layer (this crate)                                │
│  - git2duck_init(con: duckdb::Connection) -> duckdb::Result<()> │
│  - git2duck_init_c_api(db: *mut duckdb::ffi::duckdb_database)   │
│      -> *const std::os::raw::c_char (C entrypoint)          │
│  - register_git(&Connection) -> duckdb::Result<()>          │
│  - register_git_branches(&Connection) -> duckdb::Result<()> │
│  - register_git_feature_metrics(&Connection) -> duckdb::Result<()> │
│  - register_git_tags(&Connection) -> duckdb::Result<()>     │
└──────────────┬──────────────────────────────────────────────┘
               │
               ▼
┌─────────────────────────────────────┐
│  Gix Library                        │
│  - Open repository                  │
│  - Traverse commits                 │
│  - Extract commit data              │
└─────────────────────────────────────┘
```

## Development

### Testing

```bash
cargo test
```

### Using with DuckDB CLI

```bash
# Build the extension
cargo build --release

# Test with DuckDB
duckdb << EOF
-- This will work once proper DuckDB FFI is implemented
LOAD './target/release/libgit2duck.so';
SELECT COUNT(*) FROM git_commits('.');
EOF
```

### Debugging

Enable logging:
```bash
RUST_LOG=debug cargo build --release
```

## Advantages over C++ Implementation

1. **Memory Safety**: Rust prevents memory leaks and undefined behavior
2. **Native Gix**: Pure Rust Git implementation, no libgit2 dependency
3. **Better Error Handling**: Result types and pattern matching
4. **Easier Development**: Cargo manages dependencies and builds
5. **Code Reuse**: Can share code with git2duck CLI tool

## Next Steps

To complete the DuckDB integration:

1. Add proper DuckDB C API bindings or use `duckdb-rs`
2. Implement the table function callbacks
3. Register function with `duckdb_register_table_function`
4. Add comprehensive tests
5. Create proper error handling for DuckDB errors

## Resources

- [DuckDB Extension API](https://duckdb.org/docs/extensions/overview)
- [Rust FFI Guide](https://doc.rust-lang.org/nomicon/ffi.html)
- [gix Documentation](https://docs.rs/gix/)
