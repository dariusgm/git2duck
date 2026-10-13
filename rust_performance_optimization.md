# Rust Performance Optimization Plan

## Identified Hot Paths (from benchmarking)
1. `build_blob_map()` - Called 2x per commit (tree traversal)
2. `get_blob_bytes()` - Called multiple times per file
3. `is_text()` - Called for every file (binary detection)
4. Diff computation with `similar::TextDiff`
5. String allocations and cloning

## Rust Performance Best Practices to Apply

### 1. Function Inlining
- `#[inline(always)]` for small hot functions
- `#[inline]` for medium-sized functions called frequently

### 2. Memory Allocations
- Pre-allocate HashMap/Vec with capacity
- Reduce String clones (use &str where possible)
- Avoid unnecessary `.to_string()` calls

### 3. Hash Function Optimization
- Replace `std::HashMap` with `rustc_hash::FxHashMap` for small keys (ObjectId)
- Much faster for non-cryptographic hashing

### 4. Binary Detection Optimization
- Use `memchr` crate for fast byte scanning
- Replace `.contains(&0)` with `memchr::memchr(0, bytes)`

### 5. Iterator Optimizations
- Avoid intermediate collections
- Use `bytes.iter().position(|&b| b == b'\n')` for line counting

### 6. Cache Optimization
- Return Arc or references to avoid cloning large blobs
- Use `entry()` API to avoid double lookups

### 7. String Building
- Pre-allocate String capacity for path building
- Use `write!` macro instead of `format!` for hot paths

## Implementation Order
1. Add dependencies (rustc-hash, memchr)
2. Apply inlining attributes
3. Optimize hash maps
4. Optimize binary detection  
5. Reduce allocations
6. Benchmark after each major change

## Expected Impact
- 10-20% improvement from better inlining
- 5-15% from faster hash functions
- 5-10% from memchr optimization
- 5-10% from reduced allocations
- **Total: 25-55% potential improvement**

---

## Actual Results (COMPLETED)

### Performance Improvements
| Repository | Baseline | After Optimization | Improvement |
|------------|----------|-------------------|-------------|
| Rust Q4 2024 (7,564 commits) | 108s | **96s** | **11.1%** ✅ |
| Airflow 2024 (6,551 commits) | 17s | **14s** | **17.6%** ✅ |

### Applied Optimizations

#### 1. FxHashMap (~5% improvement)
- Replaced `std::HashMap` with `rustc_hash::FxHashMap`
- Much faster for non-cryptographic hashing with small keys (ObjectId)
- Applied to tree_cache, blob_cache, and prev_oid_to_paths

#### 2. memchr for Binary Detection (~3% improvement)
- Replaced `.contains(&0)` with `memchr::memchr(0, bytes)`
- Optimized byte scanning is 3-10x faster
- Critical for `is_text()` function called on every file

#### 3. Function Inlining (~2% improvement)
- Added `#[inline(always)]` to `is_text()` and `get_blob_bytes()`
- Added `#[inline]` to `build_blob_map()`
- Eliminates function call overhead in hot paths

#### 4. Pre-allocation (~1% improvement)
- HashMap/Vec capacity hints reduce reallocations
- `FxHashMap::with_capacity_and_hasher(256, ...)`
- `Vec::with_capacity(both_paths.len())`

#### 5. String Building Optimization (~1% improvement)
- Replaced `format!("{}/{}", prefix, name)` with manual building
- Pre-allocated String capacity: `String::with_capacity(prefix.len() + 1 + name_str.len())`
- Avoided `.to_string()` where possible

#### 6. Code Quality Improvements (0% perf impact, better maintainability)
- Fast line counting with `memchr::memchr_iter(b'\\n', bytes).count()`
- Cleaner code structure in `get_blob_bytes()`

### Key Learnings

1. **Hash Function Choice Matters**: FxHashMap provided ~5% gain alone
2. **Byte Operations are Critical**: memchr provided ~3% gain on hot path
3. **Inlining Hot Functions**: Small functions called millions of times benefit greatly
4. **Diminishing Returns**: First optimizations (FxHashMap, memchr, inlining) gave 90% of the gain

### Remaining Bottleneck

Even with these optimizations, the main bottleneck remains:
- **Tree traversal** in `build_blob_map()` - called 2x per commit
- **Blob loading** from Git objects

To achieve further gains (beyond 11-18%), would need:
- gix native tree-diff API (avoid manual traversal)
- Batch blob loading (reduce individual object lookups)
- Profile-guided optimization (PGO) with actual workload
