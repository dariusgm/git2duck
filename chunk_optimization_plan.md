# Chunk Size Optimization Plan

## Server Specs
- 16 cores + 16 threads (32 total threads)
- Large memory available
- Target: Optimize for maximum parallelism

## Grid Search Strategy

Test chunk sizes in order of likely performance:

### Phase 1: Smaller chunks (more parallelism)
1. **CHUNK_SIZE = 32** - Baseline (2x more chunks than current)
2. **CHUNK_SIZE = 16** - High parallelism (4x more chunks)
3. **CHUNK_SIZE = 8** - Very high parallelism (8x more chunks)

### Phase 2: Larger chunks (less overhead)
4. **CHUNK_SIZE = 128** - Lower parallelism (0.5x chunks)
5. **CHUNK_SIZE = 256** - Minimal parallelism (0.25x chunks)

### Current Baseline
- CHUNK_SIZE = 64 (current implementation)
- Rust Q4 2024: ~108s (7,564 commits)
- Airflow 2024: ~17s (6,551 commits)

## Benchmarks to Run
For each chunk size:
1. Rust repo Q4 2024 (7,564 commits) - primary metric
2. Airflow 2024 (6,551 commits) - validation metric

## Success Criteria
- Performance improvement > 5% on Rust benchmark
- All tests pass
- No regressions on Airflow benchmark

## Execution Protocol
For each test:
1. Update CHUNK_SIZE constant
2. Run: `cargo test -q`
3. Run: `cargo run --release --bin bench -- /path/to/repos/rust true 2024-10-01 2024-12-31`
4. Run: `cargo run --release --bin bench -- /path/to/repos/airflow true 2024-01-01 2024-12-31`
5. Record results
6. If best so far: commit and push
7. Continue to next size

## Expected Outcome
Optimal chunk size likely between 16-32 to maximize 32-thread utilization.

---

## Optimization Results

### 1. Chunk Size Grid Search ✅ COMPLETED
- **Result**: CHUNK_SIZE=64 is optimal
- **Tested**: 8, 16, 32, 64, 128, 256
- **Performance**: All sizes within 108-120s range on Rust Q4 2024
- **Conclusion**: Parallelization is NOT the bottleneck

### 2. Shared Blob Cache (Arc<DashMap>) ❌ NO IMPROVEMENT
- **Implementation**: Replaced per-chunk HashMap with shared DashMap across all workers
- **Result**: 
  - Rust Q4 2024: 108s (0% change)
  - Airflow 2024: 17s (0% change)
- **Analysis**: 
  - Minimal blob reuse across commits
  - Per-chunk cache was already sufficient
  - Bottleneck is in tree traversal, not cache misses
- **Decision**: REVERTED (no performance benefit)

### Next Steps
The real bottleneck is **tree traversal and blob loading**, NOT:
- Parallelization overhead (grid search confirmed)
- Cache hit rates (shared cache confirmed)

**Potential high-impact optimizations**:
1. Use gix native tree-diff instead of manual tree walking
2. Optimize tree traversal with iterative vs recursive approach
3. Batch blob loading (reduce individual object lookups)
4. Profile with perf to identify specific hot paths
