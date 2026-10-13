# Future Performance Optimization Opportunities

## Current Performance (Nov 2025)
- **Rust Q4 2024** (7,564 commits): 96s
- **Airflow 2024** (6,551 commits): 14s
- **Total speedup from baseline**: 5.7x (Rust), 39x (Airflow)

## Identified Bottleneck
The main performance bottleneck is **tree traversal and blob loading** in `build_blob_map()`, which is called 2x per commit (current tree + parent tree).

---

## High-Impact Optimizations (20-40% potential gain each)

### 1. Gix Native Tree-Diff API
**Estimated Gain**: 20-30%

**Current Problem**:
- Manual tree walking with `build_blob_map()` visits every file
- Called twice per commit (current + parent)
- No early termination for identical subtrees

**Solution**:
- Use `gix::diff::tree()` which compares trees efficiently
- Skips identical subtrees automatically
- Only reports actual changes

**Implementation**:
```rust
use gix::diff::tree::Recorder;
let mut recorder = Recorder::default();
gix::diff::tree(&parent_tree, &current_tree, &mut recorder)?;
// recorder.records contains only changed files
```

**Complexity**: Medium (requires API migration)

---

### 2. Batch Blob Loading
**Estimated Gain**: 10-20%

**Current Problem**:
- Individual `repo.find_object(oid)` calls per blob
- No prefetching or batching
- Sequential I/O pattern

**Solution**:
- Collect all blob OIDs needed for a commit
- Use gix's object database batch loading
- Potential parallel blob loading

**Implementation**:
```rust
let blob_oids: Vec<ObjectId> = changes.iter()
    .flat_map(|(_, old, new)| vec![old, new])
    .flatten()
    .collect();
// Batch load all blobs
```

**Complexity**: Medium-High

---

### 3. Profile-Guided Optimization (PGO)
**Estimated Gain**: 5-15%

**Current Problem**:
- Compiler optimizations are generic
- Hot paths not known at compile time

**Solution**:
- Build with instrumentation: `RUSTFLAGS="-Cprofile-generate=/tmp/pgo-data"`
- Run on representative workload (Rust Q4 2024)
- Rebuild with profile data: `RUSTFLAGS="-Cprofile-use=/tmp/pgo-data"`

**Implementation**:
```bash
# 1. Instrumented build
RUSTFLAGS="-Cprofile-generate=/tmp/pgo-data" cargo build --release

# 2. Run representative workload
./target/release/bench /path/to/large/repo

# 3. Merge profile data
llvm-profdata merge -o /tmp/pgo-data/merged.profdata /tmp/pgo-data

# 4. Optimized build
RUSTFLAGS="-Cprofile-use=/tmp/pgo-data/merged.profdata" cargo build --release
```

**Complexity**: Low (just build process changes)

---

## Medium-Impact Optimizations (5-15% potential gain each)

### 4. Smart Binary Detection
**Estimated Gain**: 3-8%

**Current Problem**:
- UTF-8 validation on every file
- Even when file extension indicates binary

**Solution**:
- Fast-path: Check file extension first (`.png`, `.jpg`, `.zip`, etc.)
- Only validate UTF-8 for text-like extensions

**Implementation**:
```rust
#[inline(always)]
fn is_likely_binary_by_extension(path: &str) -> bool {
    matches!(
        path.rsplit('.').next(),
        Some("png" | "jpg" | "jpeg" | "gif" | "zip" | "pdf" | "exe" | "so" | "o" | "a")
    )
}

fn is_text(bytes: &[u8], path: &str) -> bool {
    if is_likely_binary_by_extension(path) {
        return false;
    }
    // ... existing checks
}
```

**Complexity**: Low

---

### 5. Streaming Diff Computation
**Estimated Gain**: 5-10%

**Current Problem**:
- Loading full blobs into memory
- Converting to String for diff
- High memory usage for large files

**Solution**:
- Stream-based diff for large files
- Line-by-line processing
- Avoid full String allocation

**Complexity**: High

---

### 6. Zero-Copy String Handling
**Estimated Gain**: 2-5%

**Current Problem**:
- Path strings are cloned multiple times
- `.to_string()` allocations in hot paths

**Solution**:
- Use `Cow<str>` or `&str` where possible
- Arena allocator for temporary strings
- String interning for repeated paths

**Implementation**:
```rust
use bumpalo::Bump;
let arena = Bump::new();
let path = arena.alloc_str(&format!("{}/{}", prefix, name));
```

**Complexity**: Medium

---

### 7. Parallel Tree Traversal
**Estimated Gain**: 3-8% (on repos with deep directory structures)

**Current Problem**:
- `build_blob_map()` is sequential
- Large trees processed one-by-one

**Solution**:
- Parallel subtree processing with Rayon
- Each subdirectory processed by different thread

**Complexity**: Medium-High (needs careful coordination)

---

## Low-Impact Optimizations (< 5% gain, but good practice)

### 8. SIMD for Binary Detection
**Estimated Gain**: 1-3%

Use explicit SIMD for NUL byte scanning:
```rust
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;
```

### 9. Custom Allocator
**Estimated Gain**: 1-2%

Use `jemalloc` or `mimalloc` instead of system allocator:
```toml
[dependencies]
jemallocator = "0.5"
```

### 10. Reduce JSON Serialization Overhead
**Estimated Gain**: 1-2%

- Use `serde_json::to_writer()` instead of `to_string()`
- Consider binary format for internal representation

---

## Infrastructure Improvements

### 11. Incremental Processing
Store processed commit results and only recompute changed commits on subsequent runs.

### 12. Distributed Processing
Split large repos across multiple machines for parallel processing.

### 13. Caching Layer
Persistent cache of blob contents and tree structures across runs.

---

## Recommended Priority Order

**Quick Wins** (implement first):
1. Smart binary detection by extension (easy, 3-8% gain)
2. Profile-Guided Optimization (easy, 5-15% gain)

**Medium-term** (significant effort, high reward):
3. Gix native tree-diff API (medium effort, 20-30% gain)
4. Batch blob loading (medium-high effort, 10-20% gain)

**Long-term** (research/experimental):
5. Streaming diff computation
6. Zero-copy string handling
7. Parallel tree traversal

---

## Performance Target

**Current**: 96s (Rust Q4 2024)

**With all high-impact optimizations**: ~45-60s (40-50% additional improvement)

**Theoretical limit** (I/O bound): ~30-40s (git itself takes time to read objects)

---

## Notes

- Each optimization should be benchmarked independently
- Not all optimizations may stack (some address overlapping issues)
- Always profile before and after to confirm gains
- Keep test suite passing throughout

**Last Updated**: November 5, 2025
