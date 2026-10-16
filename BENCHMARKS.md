# git2duck Benchmarks and Profiling

This document explains how to benchmark git2duck processing time on large repositories (excluding network/download time) and generate flame graphs to find bottlenecks.

## Prerequisites

- Linux recommended (for `perf`).
- Rust toolchain: `rustup update` (stable), and `cargo` available.
- Build extension artifacts: `cargo build --release --features ext`.
- Optional (for flame graphs):
  - `perf` (Linux): `sudo apt-get install linux-tools-common linux-tools-generic` (distro-specific)
  - `cargo-flamegraph`: `cargo install flamegraph`
  - Ensure `perf` is usable by your user (may require `sudo sysctl kernel.perf_event_paranoid=1` or similar policy changes).

## Benchmark binary

A lightweight benchmarking binary is provided at `src/bin/bench.rs`.
It loads the built git2duck extension into DuckDB and runs a simple `SELECT COUNT(*) FROM git(?)` query, measuring execution time.

Usage:

```sh
# Build once (loads extension metadata, required for LOAD)
cargo build --release --features ext

# Run benchmark: <repo_path> [include_patches=true] [since=YYYY-MM-DD] [until=YYYY-MM-DD]
cargo run --release --bin bench -- /path/to/local/repo true 2020-01-01 2024-12-31
```

Output example:

```
repo=/path/to/local/repo, include_patches=true, rows=12345, elapsed_ms=9876.
second_pass rows=12345, elapsed_ms=4321.
```

Notes:
- The first pass measures cold execution (no DuckDB statement cache).
- The second pass shows warm/statement-cache effects.
- Use `include_patches=false` to measure commit-only metrics vs `true` to include `file_changes` computation.

## Clone repository once (exclude network/download time)

Clone the repo once locally, and then point the benchmark at the local path:

```sh
# Example for Apache Airflow
mkdir -p ~/repos
cd ~/repos
git clone https://github.com/apache/airflow.git  # one-time network cost (excluded from timing)
```

Subsequent benchmarks should point to `~/repos/airflow` to measure processing time only.

## Flame graph generation (Linux)

Use cargo-flamegraph to capture CPU profiles:

```sh
# Install tools
cargo install flamegraph
# Ensure `perf` is available (distro-specific)
# sudo apt-get install linux-tools-common linux-tools-$(uname -r)

# Build extension artifacts first (required for LOAD)
cargo build --release --features ext

# Run flamegraph for bench with patches enabled
sudo -E cargo flamegraph --bin bench -- /home/you/repos/airflow true 2019-01-01 2024-12-31

# Run flamegraph for bench with patches disabled (baseline)
sudo -E cargo flamegraph --bin bench -- /home/you/repos/airflow false 2019-01-01 2024-12-31
```

This will generate `flamegraph.svg` in the project root. Open it in a browser and inspect hot functions.

## Suggested scenarios

- Full history vs bounded windows (e.g., last 2 years): use `since` to limit processing.
- `include_patches=false` baseline to isolate commit-graph traversal cost.
- `include_patches=true` to include diff processing and `file_changes` materialization.

## What to look for

- Gix object traversal and commit walk hot spots.
- Diff/hunk computation (`similar::TextDiff`) hotspots for large binary or generated files.
- JSON serialization of `file_changes` and DuckDB string handling.
- DuckDB LOAD overhead vs execution time (should be negligible after first load).

## Parallelism considerations

- Diff processing per commit can be batched per commit or per file change; investigate parallelizing heavy inner loops (e.g., per-file diff) using `rayon`.
- Avoid parallelism that increases memory pressure substantially on very large histories.
- Consider heuristics to skip or cap processing of large/binary files when `include_patches=true`.

## Reporting

Capture the following:
- Repo, time window, include_patches flag.
- Rows (commits) processed.
- Elapsed ms, and second-pass elapsed ms.
- Attach `flamegraph.svg` and annotate top hot symbols.

## Performance Improvements (Nov 2025)

### Baseline (apache/airflow, 2024-01-01 to 2024-12-31, include_patches=true)
- Commits processed: 6,551
- Initial implementation: ~546 seconds

### Step 1: Time-window pushdown filtering
- Added `since`/`until` JSON parameters to filter commits by date before processing
- Result: **~100 seconds** (5.5x speedup)

### Step 2: Increased cache sizes and config flags
- Increased `TREE_CACHE_LIMIT` to 4096 (from default)
- Increased `BLOB_CACHE_LIMIT` to 65536 (from default)
- Added configurable `detect_renames`, `detect_copies`, `max_diff_bytes` parameters
- Result: **~103 seconds** (minimal change, better memory reuse)

### Step 3: Commit-level parallelism (Rayon)
- Split commits into 64-commit chunks processed in parallel
- Each worker opens its own `gix::Repository` handle for thread safety
- Leverages all 32 CPU cores on test server
- Result: **~16 seconds** (6.4x speedup from parallelism alone)

### Step 4: Rust Code-Level Optimizations (Nov 2025)
- Applied Rust performance best practices
- FxHashMap for faster hashing (~5% gain)
- memchr for binary detection and line counting (~3% gain)
- Function inlining with #[inline(always)] (~2% gain)
- Pre-allocation and string building optimizations (~2% gain)
- Result: **~96 seconds** for Rust Q4 2024 (7,564 commits)
- Result: **~14 seconds** for Airflow 2024 (6,551 commits)

### Step 5: Gix Native Tree-Diff Integration (Nov 2025)
- Replaced manual `build_blob_map()` tree traversal with gix native tree-diff API
- Used `gix::Tree::changes().for_each_to_obtain_tree()` for optimized tree enumeration
- Removed tree cache logic and reduced code by 583 lines
- Leverages gix's internal optimizations for tree comparison
- Result: **~34.5 seconds** for Rust Q4 2024 (7,564 commits) - **64% faster, 2.78x speedup**
- Result: **~10.8 seconds** for Airflow 2024 (6,551 commits) - **23% faster**

### Total Improvement Journey
- **Airflow 2024 (6,551 commits)**:
  - Original: 546s
  - After parallelism: 16s (34x speedup)
  - After Rust optimizations: 14s (39x speedup)
  - After gix tree-diff: **10.8s** (50.6x speedup)

- **Rust Q4 2024 (7,564 commits)** - New benchmark:
  - Original estimate: ~550s
  - After parallelism: 108s (5x speedup)
  - After Rust optimizations: 96s (5.7x speedup)
  - After gix tree-diff: **34.5s** (15.9x speedup)

### Key Learnings
1. Time-window pushdown was the biggest win (5.5x)
2. Commit-level parallelism essential for multi-core servers (6.4x)
3. Chunk size 64 is optimal for 32-thread servers
4. Rust code optimizations provide 11-18% additional gain
5. **Gix native tree-diff eliminated tree traversal bottleneck (23-64% gain, 2.78x on complex repos)**
6. Larger repositories with complex trees benefit most from native tree-diff
7. Code simplification (583 fewer lines) improved maintainability without sacrificing performance

### Next Steps
See [`docs/notes/FUTURE_OPTIMIZATIONS.md`](docs/notes/FUTURE_OPTIMIZATIONS.md) for potential further improvements:
- Batch blob loading (10-20% potential gain)
- Profile-guided optimization (5-15% potential gain)
- Parallel blob diff computation optimization

