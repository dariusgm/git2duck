# Git2Duck Quick Start Guide

## Quick Build & Test

### 1. Build Everything

```bash
cd /path/to/git2duck
cargo build --release
```

This will build both:
- `git2duck-cli` (CLI tool)
- `git2duck` (DuckDB extension as shared library)

### 2. Test the CLI

```bash
# Count commits in current directory
./target/release/git2duck .

# Count commits in git2parquet
./target/release/git2duck ../git2parquet

# Count commits from a remote repo
./target/release/git2duck https://github.com/duckdb/duckdb
```

### 3. Test the DuckDB Extension (via Python FFI)

```bash
cd git2duck-duckdb

# Run the Python example
python3 examples/python_example.py ../git2parquet
```

Expected output:
```
Loading git2duck library...
Reading commits from: ../git2parquet
Found 123 commits

Top 10 commits:
--------------------------------------------------------------------------------
Hash:   abc123def456
Author: John Doe <john@example.com>
Time:   1698765432
Msg:    Add new feature for analyzing commits
--------------------------------------------------------------------------------
...
```

## Directory Structure After Build

```
git2duck/
├── target/
│   └── release/
│       ├── git2duck                       # CLI binary
│       └── libgit2duck.{so,dylib}         # DuckDB extension library
├── git2duck-cli/                          # CLI source
└── git2duck-duckdb/                       # Extension source
```

## Common Issues

### Issue: "Cannot find libgit2duck.so"

**Solution**: Make sure you've built with `cargo build --release` from the workspace root.

### Issue: Python can't load the library

**Solution**: Check the path in the Python script matches your OS:
- Linux: `.so`
- macOS: `.dylib`
- Windows: `.dll`

### Issue: Workspace build fails

**Solution**: Ensure you're running from the workspace root (`/path/to/git2duck`), not from a sub-crate directory.

## Using from DuckDB (Future)

Once the full DuckDB C API integration is complete, you'll be able to:

```sql
-- Start DuckDB
duckdb

-- Load the extension
LOAD './target/release/libgit2duck.so';

-- Query commits
SELECT COUNT(*) FROM git_commits('../git2parquet');

-- Analyze patterns (daily example; aggregate as needed in SQL)
SELECT 
    author_name,
    COUNT(*) as commits,
    DATE_TRUNC('day', commit_time) as day
FROM git_commits('../git2parquet')
GROUP BY author_name, day
ORDER BY day DESC, commits DESC;
```

## Development Workflow

```bash
# 1. Make changes to code
vim git2duck-cli/src/main.rs

# 2. Check for errors (fast)
cargo check

# 3. Run tests
cargo test

# 4. Format code
cargo fmt

# 5. Lint
cargo clippy

# 6. Build release
cargo build --release
```

## Next Steps

1. **Explore the code**: 
   - `git2duck-cli/src/main.rs` - CLI implementation
   - `git2duck-duckdb/src/lib.rs` - FFI and Git operations

2. **Complete DuckDB integration**:
   - Add proper DuckDB C API bindings
   - Implement table function callbacks
   - Add streaming support

3. **Add more features**:
   - File change tracking
   - Git blame
   - Branch/tag analysis

## Resources

- [Rust FFI Documentation](https://doc.rust-lang.org/nomicon/ffi.html)
- [DuckDB Extension Guide](https://duckdb.org/docs/extensions/overview)
- [gix Documentation](https://docs.rs/gix/)
