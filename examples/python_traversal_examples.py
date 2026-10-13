#!/usr/bin/env python3
"""
Comprehensive examples of commit graph traversal using git2duck DuckDB extension.
Shows multiple approaches to navigate from commit A to commit B.
"""

import argparse
import json
from pathlib import Path
from typing import Dict, List, Set, Optional
import duckdb


def find_extension(debug: bool) -> str:
    """Locate the compiled extension library."""
    import os
    root = Path(__file__).resolve().parents[1]
    if os.name == "nt":
        name = "git2duck.dll"
    elif hasattr(os, "uname") and os.uname().sysname == "Darwin":
        name = "libgit2duck.dylib"
    else:
        name = "libgit2duck.so"
    build = "debug" if debug else "release"
    return str(root / "target" / build / name)


def example_1_recursive_cte(con: duckdb.DuckDBPyConnection, repo: str, from_hash: str, to_hash: str):
    """
    Example 1: Recursive CTE (SQL-based traversal)
    
    Uses DuckDB's WITH RECURSIVE to traverse the commit graph from one commit to another.
    This is the most SQL-idiomatic approach and leverages DuckDB's query optimizer.
    """
    print("\n" + "="*80)
    print("EXAMPLE 1: Recursive CTE (Pure SQL)")
    print("="*80)
    
    query = """
    WITH RECURSIVE commit_path AS (
        -- Base case: start from the 'from' commit
        SELECT 
            commit_hash,
            message,
            author_name,
            commit_time,
            parents,
            0 AS depth,
            CAST(commit_hash AS VARCHAR) AS path
        FROM git(?)
        WHERE commit_hash LIKE ?
        
        UNION ALL
        
        -- Recursive case: find parent commits
        SELECT 
            g.commit_hash,
            g.message,
            g.author_name,
            g.commit_time,
            g.parents,
            cp.depth + 1 AS depth,
            cp.path || ' -> ' || g.commit_hash AS path
        FROM commit_path cp
        CROSS JOIN LATERAL (
            SELECT unnest(json_extract_string(cp.parents, '$[*]')) AS parent_hash
        ) AS parent_list
        JOIN git(?) g ON g.commit_hash = parent_list.parent_hash
        WHERE cp.depth < 100  -- Prevent infinite loops
    )
    SELECT 
        commit_hash,
        message,
        author_name,
        depth,
        path
    FROM commit_path
    WHERE commit_hash LIKE ?
    ORDER BY depth
    LIMIT 1;
    """
    
    try:
        result = con.execute(query, [repo, from_hash + '%', repo, to_hash + '%']).fetchall()
        if result:
            commit_hash, message, author, depth, path = result[0]
            print(f"Found path in {depth} steps:")
            print(f"  From: {from_hash[:8]}")
            print(f"  To:   {commit_hash[:8]}")
            print(f"  Author: {author}")
            print(f"  Message: {message[:50]}...")
            print(f"\nFull path:")
            for step in path.split(' -> '):
                print(f"  {step[:8]}")
        else:
            print(f"No path found from {from_hash[:8]} to {to_hash[:8]}")
    except Exception as e:
        print(f"Error: {e}")


def example_2_iterative_python(con: duckdb.DuckDBPyConnection, repo: str, from_hash: str, to_hash: str):
    """
    Example 2: Iterative Python BFS (Breadth-First Search)
    
    Loads all commits into Python and performs BFS to find the shortest path.
    Good for complex logic that's hard to express in SQL.
    """
    print("\n" + "="*80)
    print("EXAMPLE 2: Iterative Python BFS")
    print("="*80)
    
    # Load all commits
    commits = con.execute("""
        SELECT commit_hash, message, author_name, parents 
        FROM git(?)
    """, [repo]).fetchall()
    
    # Build adjacency list
    graph: Dict[str, List[str]] = {}
    commit_info: Dict[str, tuple] = {}
    
    for commit_hash, message, author, parents_json in commits:
        parents = json.loads(parents_json)
        graph[commit_hash] = parents
        commit_info[commit_hash] = (message, author)
    
    # Find matching hashes
    from_commit = next((h for h in graph.keys() if h.startswith(from_hash)), None)
    to_commit = next((h for h in graph.keys() if h.startswith(to_hash)), None)
    
    if not from_commit or not to_commit:
        print(f"Could not find commits: from={from_commit}, to={to_commit}")
        return
    
    # BFS
    from collections import deque
    queue = deque([(from_commit, [from_commit])])
    visited: Set[str] = {from_commit}
    
    while queue:
        current, path = queue.popleft()
        
        if current == to_commit:
            print(f"Found path in {len(path)-1} steps:")
            for i, commit_hash in enumerate(path):
                message, author = commit_info[commit_hash]
                print(f"  {i}. {commit_hash[:8]} - {author} - {message[:40]}...")
            return
        
        for parent in graph.get(current, []):
            if parent not in visited:
                visited.add(parent)
                queue.append((parent, path + [parent]))
    
    print(f"No path found from {from_commit[:8]} to {to_commit[:8]}")


def example_3_dfs_all_paths(con: duckdb.DuckDBPyConnection, repo: str, from_hash: str, to_hash: str, max_depth: int = 10):
    """
    Example 3: DFS finding all paths (Python)
    
    Uses Depth-First Search to find ALL paths between two commits.
    Useful for understanding merge scenarios.
    """
    print("\n" + "="*80)
    print("EXAMPLE 3: DFS - Find All Paths")
    print("="*80)
    
    # Load commits
    commits = con.execute("""
        SELECT commit_hash, message, parents 
        FROM git(?)
    """, [repo]).fetchall()
    
    graph: Dict[str, List[str]] = {}
    for commit_hash, message, parents_json in commits:
        parents = json.loads(parents_json)
        graph[commit_hash] = parents
    
    from_commit = next((h for h in graph.keys() if h.startswith(from_hash)), None)
    to_commit = next((h for h in graph.keys() if h.startswith(to_hash)), None)
    
    if not from_commit or not to_commit:
        print("Could not find specified commits")
        return
    
    all_paths = []
    
    def dfs(current: str, target: str, path: List[str], visited: Set[str]):
        if len(path) > max_depth:
            return
        
        if current == target:
            all_paths.append(path.copy())
            return
        
        for parent in graph.get(current, []):
            if parent not in visited:
                visited.add(parent)
                path.append(parent)
                dfs(parent, target, path, visited)
                path.pop()
                visited.remove(parent)
    
    dfs(from_commit, to_commit, [from_commit], {from_commit})
    
    if all_paths:
        print(f"Found {len(all_paths)} path(s):")
        for i, path in enumerate(all_paths[:5], 1):  # Show first 5
            print(f"\n  Path {i} ({len(path)-1} steps):")
            for commit_hash in path:
                print(f"    {commit_hash[:8]}")
        if len(all_paths) > 5:
            print(f"\n  ... and {len(all_paths)-5} more paths")
    else:
        print("No paths found")


def example_4_sql_window_functions(con: duckdb.DuckDBPyConnection, repo: str):
    """
    Example 4: SQL Window Functions for commit analysis
    
    Demonstrates using SQL analytics to understand commit structure.
    """
    print("\n" + "="*80)
    print("EXAMPLE 4: SQL Window Functions & Analytics")
    print("="*80)
    
    query = """
    WITH commit_data AS (
        SELECT 
            commit_hash,
            message,
            author_name,
            commit_time,
            len(json_extract_string(parents, '$[*]')) AS parent_count,
            parents
        FROM git(?)
    )
    SELECT 
        commit_hash,
        author_name,
        parent_count,
        CASE 
            WHEN parent_count = 0 THEN 'Root'
            WHEN parent_count = 1 THEN 'Normal'
            WHEN parent_count > 1 THEN 'Merge'
        END AS commit_type,
        COUNT(*) OVER (PARTITION BY author_name) AS author_commit_count,
        ROW_NUMBER() OVER (PARTITION BY author_name ORDER BY commit_time DESC) AS author_commit_rank
    FROM commit_data
    ORDER BY commit_time DESC
    LIMIT 20;
    """
    
    results = con.execute(query, [repo]).fetchall()
    print(f"\nRecent commits with analysis:")
    print(f"{'Hash':<10} {'Author':<20} {'Type':<8} {'Auth Commits':<12} {'Rank':<6}")
    print("-" * 70)
    for row in results:
        commit_hash, author, parent_count, commit_type, auth_commits, rank = row
        print(f"{commit_hash[:8]:<10} {author[:20]:<20} {commit_type:<8} {auth_commits:<12} {rank:<6}")


def example_5_parent_child_relationships(con: duckdb.DuckDBPyConnection, repo: str):
    """
    Example 5: Parent-Child relationship table
    
    Creates a normalized parent-child edge table for graph analysis.
    """
    print("\n" + "="*80)
    print("EXAMPLE 5: Parent-Child Edge Table")
    print("="*80)
    
    query = """
    WITH edges AS (
        SELECT 
            commit_hash AS child,
            unnest(json_extract_string(parents, '$[*]')) AS parent
        FROM git(?)
    )
    SELECT 
        child,
        parent,
        (SELECT message FROM git(?) g WHERE g.commit_hash = edges.parent) AS parent_message
    FROM edges
    LIMIT 20;
    """
    
    results = con.execute(query, [repo, repo]).fetchall()
    print(f"\nParent-Child relationships:")
    print(f"{'Child':<10} {'Parent':<10} {'Parent Message':<40}")
    print("-" * 65)
    for child, parent, message in results:
        msg_short = (message[:37] + '...') if message and len(message) > 40 else (message or '')
        print(f"{child[:8]:<10} {parent[:8]:<10} {msg_short:<40}")


def example_6_materialized_graph(con: duckdb.DuckDBPyConnection, repo: str):
    """
    Example 6: Materialized graph table for fast queries
    
    Creates a temp table with edges for efficient repeated queries.
    """
    print("\n" + "="*80)
    print("EXAMPLE 6: Materialized Graph for Performance")
    print("="*80)
    
    # Create temp edge table
    con.execute("""
        CREATE TEMP TABLE commit_edges AS
        SELECT 
            commit_hash AS child,
            unnest(json_extract_string(parents, '$[*]')) AS parent
        FROM git(?);
    """, [repo])
    
    # Create temp commit details table
    con.execute("""
        CREATE TEMP TABLE commit_details AS
        SELECT commit_hash, message, author_name, commit_time
        FROM git(?);
    """, [repo])
    
    # Now we can do fast queries
    print("\nCommits with their immediate children:")
    results = con.execute("""
        SELECT 
            c.commit_hash,
            c.message,
            COUNT(e.child) AS child_count,
            list(e.child) AS children
        FROM commit_details c
        LEFT JOIN commit_edges e ON c.commit_hash = e.parent
        GROUP BY c.commit_hash, c.message
        HAVING child_count > 0
        ORDER BY child_count DESC
        LIMIT 10;
    """).fetchall()
    
    for commit_hash, message, child_count, children in results:
        msg_short = (message[:37] + '...') if len(message) > 40 else message
        print(f"\n  {commit_hash[:8]} ({child_count} children) - {msg_short}")
        if children:
            for child in children[:3]:
                print(f"    ↳ {child[:8]}")


def main():
    parser = argparse.ArgumentParser(
        description="Git commit traversal examples using git2duck DuckDB extension"
    )
    parser.add_argument("repo", help="Path or URL to a git repository")
    parser.add_argument("--from", dest="from_commit", help="Starting commit hash (prefix)")
    parser.add_argument("--to", dest="to_commit", help="Target commit hash (prefix)")
    parser.add_argument("--debug", action="store_true", help="Load debug extension")
    parser.add_argument("--example", type=int, help="Run specific example (1-6), default: all")
    
    args = parser.parse_args()
    
    ext_path = find_extension(args.debug)
    if not Path(ext_path).exists():
        raise SystemExit(
            f"Extension not found at {ext_path}. "
            f"Build with: cargo build {'--debug' if args.debug else '--release'}"
        )
    
    con = duckdb.connect(database=":memory:")
    con.execute(f"LOAD '{ext_path.replace(chr(39), chr(39)+chr(39))}';")
    
    print(f"Analyzing repository: {args.repo}")
    
    # Get total commits
    total = con.execute("SELECT COUNT(*) FROM git(?)", [args.repo]).fetchone()[0]
    print(f"Total commits: {total}")
    
    # Run examples
    if args.example in (4, None):
        example_4_sql_window_functions(con, args.repo)
    
    if args.example in (5, None):
        example_5_parent_child_relationships(con, args.repo)
    
    if args.example in (6, None):
        example_6_materialized_graph(con, args.repo)
    
    # Path-finding examples require from/to commits
    if args.from_commit and args.to_commit:
        if args.example in (1, None):
            example_1_recursive_cte(con, args.repo, args.from_commit, args.to_commit)
        
        if args.example in (2, None):
            example_2_iterative_python(con, args.repo, args.from_commit, args.to_commit)
        
        if args.example in (3, None):
            example_3_dfs_all_paths(con, args.repo, args.from_commit, args.to_commit)
    else:
        print("\nNote: Provide --from and --to arguments to see path-finding examples (1-3)")


if __name__ == "__main__":
    main()
