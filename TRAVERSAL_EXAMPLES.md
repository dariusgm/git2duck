# Git Commit Traversal Examples

This document explains the comprehensive commit graph traversal capabilities added to git2duck.

## What Changed

The `git(path)` table function now returns detailed commit information instead of just a count:

### New Schema

| Column | Type | Description |
|--------|------|-------------|
| `commit_hash` | VARCHAR | Full SHA-1 commit hash |
| `message` | VARCHAR | Commit message (full text) |
| `author_name` | VARCHAR | Author's name |
| `author_email` | VARCHAR | Author's email |
| `commit_time` | BIGINT | Unix timestamp |
| `parents` | VARCHAR | JSON array of parent commit hashes |
| `lines_added` | BIGINT | Lines added vs first parent (text files only) |
| `lines_modified` | BIGINT | Lines modified vs first parent (min of ins/del per file) |
| `lines_deleted` | BIGINT | Lines deleted vs first parent (text files only) |

### Line Metrics Computation

**Per-commit diff statistics** are computed against the **first parent only**:
- **lines_added**: Total inserted lines across all text files.
- **lines_modified**: Sum of min(inserted, deleted) per file—approximates replaced lines.
- **lines_deleted**: Total deleted lines across all text files.
- **Binary detection**: Files with NUL bytes or invalid UTF-8 are skipped entirely.
- **Root commits**: All metrics are zero (no parent to diff against).
- **Merge commits**: Only the first-parent diff is computed (consistent with typical dev tool behavior).

**Definition alignment**: These metrics match what GitHub/GitLab show in their commit views.

## Traversal Examples

### 1. SQL Recursive CTE (Pure DuckDB)

**File**: `examples/duckdb_example.sql`

Find path from commit A to commit B using SQL recursion:

```sql
WITH RECURSIVE commit_path AS (
    SELECT commit_hash, message, parents, 0 AS depth,
           CAST(commit_hash AS VARCHAR) AS path
    FROM git('.')
    WHERE commit_hash LIKE 'abc123%'
    
    UNION ALL
    
    SELECT g.commit_hash, g.message, g.parents, cp.depth + 1,
           cp.path || ' -> ' || g.commit_hash AS path
    FROM commit_path cp
    CROSS JOIN LATERAL (
        SELECT unnest(json_extract_string(cp.parents, '$[*]')) AS parent_hash
    ) AS parent_list
    JOIN git('.') g ON g.commit_hash = parent_list.parent_hash
    WHERE cp.depth < 50
)
SELECT * FROM commit_path WHERE commit_hash LIKE 'def456%';
```

**Advantages**:
- Leverages DuckDB's query optimizer
- No external dependencies
- Efficient for simple path queries

### 2. Python BFS (Breadth-First Search)

**File**: `examples/python_traversal_examples.py` - Example 2

Finds the **shortest path** between two commits:

```python
def example_2_iterative_python(con, repo, from_hash, to_hash):
    # Load all commits
    commits = con.execute("SELECT commit_hash, message, parents FROM git(?)", [repo])
    
    # Build graph
    graph = {hash: json.loads(parents) for hash, _, parents in commits}
    
    # BFS
    queue = deque([(from_commit, [from_commit])])
    visited = {from_commit}
    
    while queue:
        current, path = queue.popleft()
        if current == to_commit:
            print(f"Found path in {len(path)-1} steps")
            return path
        
        for parent in graph.get(current, []):
            if parent not in visited:
                visited.add(parent)
                queue.append((parent, path + [parent]))
```

**Advantages**:
- Guaranteed shortest path
- Easy to add custom logic
- Good for complex filtering

### 3. Python DFS (Depth-First Search)

**File**: `examples/python_traversal_examples.py` - Example 3

Finds **all paths** between two commits (useful for understanding merges):

```python
def example_3_dfs_all_paths(con, repo, from_hash, to_hash, max_depth=10):
    all_paths = []
    
    def dfs(current, target, path, visited):
        if current == target:
            all_paths.append(path.copy())
            return
        
        for parent in graph.get(current, []):
            if parent not in visited and len(path) <= max_depth:
                visited.add(parent)
                path.append(parent)
                dfs(parent, target, path, visited)
                path.pop()
                visited.remove(parent)
    
    dfs(from_commit, to_commit, [from_commit], {from_commit})
    return all_paths
```

**Advantages**:
- Finds all possible paths
- Useful for merge analysis
- Can identify bottlenecks

### 4. SQL Window Functions

**File**: `examples/python_traversal_examples.py` - Example 4

Analyze commit patterns and statistics:

```sql
SELECT 
    commit_hash,
    author_name,
    len(json_extract_string(parents, '$[*]')) AS parent_count,
    CASE 
        WHEN parent_count = 0 THEN 'Root'
        WHEN parent_count = 1 THEN 'Normal'
        WHEN parent_count > 1 THEN 'Merge'
    END AS commit_type,
    COUNT(*) OVER (PARTITION BY author_name) AS author_commit_count
FROM git('.')
ORDER BY commit_time DESC;
```

**Advantages**:
- Rich analytics
- Identify merge commits, root commits
- Author statistics

### 5. Parent-Child Edge Table

**File**: `examples/python_traversal_examples.py` - Example 5

Create a normalized graph edge table:

```sql
WITH edges AS (
    SELECT 
        commit_hash AS child,
        unnest(json_extract_string(parents, '$[*]')) AS parent
    FROM git('.')
)
SELECT child, parent FROM edges;
```

**Advantages**:
- Standard graph format
- Easy to join and analyze
- Works with graph libraries

### 6. Materialized Graph

**File**: `examples/python_traversal_examples.py` - Example 6

Create temp tables for fast repeated queries:

```sql
-- Create edge table once
CREATE TEMP TABLE commit_edges AS
SELECT commit_hash AS child,
       unnest(json_extract_string(parents, '$[*]')) AS parent
FROM git('.');

-- Fast queries
SELECT c.commit_hash, COUNT(e.child) AS child_count
FROM commit_details c
LEFT JOIN commit_edges e ON c.commit_hash = e.parent
GROUP BY c.commit_hash
ORDER BY child_count DESC;
```

**Advantages**:
- Fast repeated queries
- No re-parsing
- Good for complex analysis

## Running the Examples

### SQL Examples

```bash
# Load the extension
duckdb :memory: -c "LOAD './target/release/libgit2duck.so';"

# Run queries from file
duckdb :memory: < examples/duckdb_example.sql
```

### Python Examples

```bash
# Basic usage
python examples/python_example.py .

# Traversal examples
python examples/python_traversal_examples.py . --from abc123 --to def456

# Specific example
python examples/python_traversal_examples.py . --example 2 --from abc123 --to def456
```

## Use Cases

### Find commits between releases
```sql
-- Find all commits between two release tags
WITH RECURSIVE path AS (
    SELECT * FROM git('.') WHERE commit_hash LIKE 'v2.0%'
    UNION ALL
    SELECT g.* FROM path p, git('.') g, 
    LATERAL unnest(json_extract_string(p.parents, '$[*]')) AS parent
    WHERE g.commit_hash = parent AND g.commit_hash NOT LIKE 'v1.0%'
)
SELECT commit_hash, message FROM path;
```

### Identify merge commits causing issues
```sql
-- Find merge commits with many parents (octopus merges)
SELECT commit_hash, message, 
       json_array_length(parents::JSON) AS parent_count
FROM git('.')
WHERE parent_count > 2
ORDER BY parent_count DESC;
```

### Author contribution analysis
```sql
-- Commits per author over time
SELECT 
    author_name,
    strftime(CAST(to_timestamp(commit_time) AS TIMESTAMP), '%Y-%m') AS month,
    COUNT(*) AS commits
FROM git('.')
GROUP BY author_name, month
ORDER BY month DESC, commits DESC;
```

### Total lines touched from commit A to B
```sql
-- Calculate total lines modified along the first-parent path
-- from a recent commit back to an older commit
WITH RECURSIVE first_parent_path AS (
    SELECT commit_hash, parents, lines_added, lines_modified, lines_deleted, 0 AS depth
    FROM git('.')
    WHERE commit_hash LIKE 'abc123%'  -- Starting commit

    UNION ALL

    SELECT g.commit_hash, g.parents, g.lines_added, g.lines_modified, g.lines_deleted, p.depth + 1
    FROM first_parent_path p
    JOIN git('.') g 
      ON g.commit_hash = json_extract_string(p.parents, '$[0]')  -- First parent only
    WHERE p.depth < 500 AND json_array_length(p.parents::JSON) > 0
)
SELECT 
    SUM(lines_added + lines_modified + lines_deleted) AS total_lines_touched,
    SUM(lines_added) AS total_added,
    SUM(lines_modified) AS total_modified,
    SUM(lines_deleted) AS total_deleted,
    COUNT(*) AS commits_in_path
FROM first_parent_path
WHERE commit_hash LIKE 'def456%';  -- Ending commit (ancestor of starting commit)
```

This query traverses the **first-parent path** from the starting commit to the ending commit and sums up all line changes. Useful for:
- Estimating code churn between releases
- Understanding the scope of changes in a feature branch
- Tracking refactoring impact

## Performance Tips

1. **Use indexed temp tables** for repeated queries
2. **Limit recursion depth** to prevent infinite loops
3. **Filter early** in recursive CTEs
4. **Materialize graphs** for complex multi-query analysis
5. **Use appropriate algorithm**: BFS for shortest path, DFS for all paths

## API Reference

See the updated examples for full API usage:
- `examples/python_example.py` - Basic usage
- `examples/python_traversal_examples.py` - Advanced traversal
- `examples/duckdb_example.sql` - SQL queries
