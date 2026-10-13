-- ============================================================================
-- Git2Duck DuckDB Extension Example Queries
-- Demonstrates commit graph traversal and analysis with parent/child relationships
-- ============================================================================

-- Load the extension (adjust path for debug vs release builds)
-- LOAD './target/release/libgit2duck.so';
-- LOAD './target/debug/libgit2duck.so';

-- ============================================================================
-- BASIC QUERIES
-- ============================================================================

-- Get all commits with details
SELECT 
    commit_hash,
    message,
    author_name,
    author_email,
    commit_time,
    parents
FROM git('.')
LIMIT 10;

-- Count total commits
SELECT COUNT(*) AS total_commits FROM git('.');

-- Show commits by author
SELECT 
    author_name,
    COUNT(*) AS commit_count
FROM git('.')
GROUP BY author_name
ORDER BY commit_count DESC;

-- ============================================================================
-- PARENT/CHILD RELATIONSHIP QUERIES
-- ============================================================================

-- Create a parent-child edge table
WITH edges AS (
    SELECT 
        commit_hash AS child,
        unnest(json_extract_string(parents, '$[*]')) AS parent
    FROM git('.')
)
SELECT * FROM edges LIMIT 20;

-- Find root commits (commits with no parents)
SELECT 
    commit_hash,
    message,
    author_name
FROM git('.')
WHERE json_array_length(parents::JSON) = 0;

-- Find merge commits (commits with multiple parents)
SELECT 
    commit_hash,
    message,
    author_name,
    json_array_length(parents::JSON) AS parent_count
FROM git('.')
WHERE json_array_length(parents::JSON) > 1
ORDER BY parent_count DESC;

-- ============================================================================
-- RECURSIVE TRAVERSAL - Find path from commit A to commit B
-- ============================================================================

-- Example: Find path between two commits (replace hash prefixes)
WITH RECURSIVE commit_path AS (
    -- Base case: start from a specific commit
    SELECT 
        commit_hash,
        message,
        author_name,
        parents,
        0 AS depth,
        CAST(commit_hash AS VARCHAR) AS path
    FROM git('.')
    WHERE commit_hash LIKE '770b2d3%'  -- Replace with your starting commit
    
    UNION ALL
    
    -- Recursive case: follow parent commits
    SELECT 
        g.commit_hash,
        g.message,
        g.author_name,
        g.parents,
        cp.depth + 1 AS depth,
        cp.path || ' -> ' || g.commit_hash AS path
    FROM commit_path cp
    CROSS JOIN LATERAL (
        SELECT unnest(json_extract_string(cp.parents, '$[*]')) AS parent_hash
    ) AS parent_list
    JOIN git('.') g ON g.commit_hash = parent_list.parent_hash
    WHERE cp.depth < 50  -- Limit depth to prevent infinite loops
)
SELECT 
    depth,
    commit_hash,
    message,
    path
FROM commit_path
ORDER BY depth;

-- ============================================================================
-- FIND ALL ANCESTORS of a specific commit
-- ============================================================================

WITH RECURSIVE ancestors AS (
    SELECT 
        commit_hash,
        message,
        parents,
        0 AS generation
    FROM git('.')
    WHERE commit_hash LIKE '770b2d3%'  -- Replace with your commit
    
    UNION ALL
    
    SELECT 
        g.commit_hash,
        g.message,
        g.parents,
        a.generation + 1
    FROM ancestors a
    CROSS JOIN LATERAL (
        SELECT unnest(json_extract_string(a.parents, '$[*]')) AS parent_hash
    ) AS parent_list
    JOIN (SELECT commit_hash, message, parents FROM git('.')) g 
        ON g.commit_hash = parent_list.parent_hash
    WHERE a.generation < 100
)
SELECT 
    generation,
    commit_hash,
    left(message, 50) AS message_preview
FROM ancestors
ORDER BY generation;

-- ============================================================================
-- COMMIT STATISTICS & ANALYTICS
-- ============================================================================

-- Commits per author with merge commit count
SELECT 
    author_name,
    COUNT(*) AS total_commits,
    SUM(CASE WHEN json_array_length(parents::JSON) > 1 THEN 1 ELSE 0 END) AS merge_commits,
    MIN(commit_time) AS first_commit,
    MAX(commit_time) AS last_commit
FROM git('.')
GROUP BY author_name
ORDER BY total_commits DESC;

-- Commit activity over time (by month)
SELECT 
    strftime(CAST(to_timestamp(commit_time) AS TIMESTAMP), '%Y-%m') AS month,
    COUNT(*) AS commits,
    COUNT(DISTINCT author_name) AS unique_authors
FROM git('.')
GROUP BY month
ORDER BY month DESC
LIMIT 12;

-- ============================================================================
-- GRAPH ANALYSIS
-- ============================================================================

-- Find commits that are heavily branched (many children)
WITH edges AS (
    SELECT 
        commit_hash AS child,
        unnest(json_extract_string(parents, '$[*]')) AS parent
    FROM git('.')
)
SELECT 
    g.commit_hash,
    g.message,
    COUNT(e.child) AS children_count
FROM git('.') g
LEFT JOIN edges e ON g.commit_hash = e.parent
GROUP BY g.commit_hash, g.message
HAVING children_count > 0
ORDER BY children_count DESC
LIMIT 10;

-- Find isolated commit chains (commits with single parent and single child)
WITH edges AS (
    SELECT 
        commit_hash AS child,
        unnest(json_extract_string(parents, '$[*]')) AS parent
    FROM git('.')
),
child_counts AS (
    SELECT parent, COUNT(*) AS child_count
    FROM edges
    GROUP BY parent
)
SELECT 
    g.commit_hash,
    g.message,
    json_array_length(g.parents::JSON) AS parent_count,
    COALESCE(cc.child_count, 0) AS child_count
FROM git('.') g
LEFT JOIN child_counts cc ON g.commit_hash = cc.parent
WHERE json_array_length(g.parents::JSON) = 1
  AND COALESCE(cc.child_count, 0) = 1
LIMIT 20;

-- ============================================================================
-- WORKING WITH REMOTE REPOSITORIES
-- ============================================================================

-- Query a remote repository (will clone temporarily)
SELECT 
    commit_hash,
    author_name,
    left(message, 50) AS message_preview
FROM git('https://github.com/dariusgm/quick-deck-mcp.git')
LIMIT 10;

-- ============================================================================
-- ADVANCED: Shortest path between any two commits
-- =========================================================================

-- This query finds the shortest path between two specific commits
-- Replace the hash prefixes with your actual commit hashes
WITH RECURSIVE paths AS (
    -- Start from commit A
    SELECT 
        commit_hash,
        parents,
        0 AS distance,
        CAST(commit_hash AS VARCHAR) AS path,
        ARRAY[commit_hash] AS visited
    FROM git('.')
    WHERE commit_hash LIKE '770b2d3%'  -- Commit A
    
    UNION ALL
    
    -- Expand to parent commits
    SELECT 
        g.commit_hash,
        g.parents,
        p.distance + 1,
        p.path || ' -> ' || g.commit_hash,
        array_append(p.visited, g.commit_hash)
    FROM paths p
    CROSS JOIN LATERAL (
        SELECT unnest(json_extract_string(p.parents, '$[*]')) AS parent_hash
    ) AS parent_list
    JOIN git('.') g ON g.commit_hash = parent_list.parent_hash
    WHERE p.distance < 100
      AND NOT list_contains(p.visited, g.commit_hash)  -- Avoid cycles
)
SELECT 
    distance,
    commit_hash,
    path
FROM paths
WHERE commit_hash LIKE '4bc2348%'  -- Commit B
ORDER BY distance
LIMIT 1;

-- =========================================================================
-- LINES TOUCHED FROM COMMIT A TO COMMIT B (first-parent path)
-- =========================================================================

-- Replace the hash prefixes with your actual commits
WITH RECURSIVE first_parent_path AS (
    SELECT commit_hash, parents, lines_added, lines_modified, lines_deleted, 0 AS depth
    FROM git('.')
    WHERE commit_hash LIKE '770b2d3%'

    UNION ALL

    SELECT g.commit_hash, g.parents, g.lines_added, g.lines_modified, g.lines_deleted, p.depth + 1
    FROM first_parent_path p
    JOIN git('.') g 
      ON g.commit_hash = json_extract_string(p.parents, '$[0]') -- first parent only
    WHERE p.depth < 500
)
SELECT 
    SUM(lines_added + lines_modified + lines_deleted) AS total_lines_touched,
    SUM(lines_added) AS total_added,
    SUM(lines_modified) AS total_modified,
    SUM(lines_deleted) AS total_deleted
FROM first_parent_path
WHERE commit_hash LIKE '4bc2348%';
