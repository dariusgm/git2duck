import os
from pathlib import Path

import duckdb
import pytest

from helper import find_extension, ROOT



def test_lines_touched_between_commits():
    """
    Test computing total lines touched from commit A to commit B along first-parent path.
    Uses DuckDB recursive CTE to traverse and sum line metrics.
    """
    # Build extension
    os.system(f"cargo build --release --features ext --manifest-path {ROOT / 'Cargo.toml'} > /dev/null 2>&1")

    ext_path = find_extension(debug=False)
    assert Path(ext_path).exists(), f"Extension not found at {ext_path}"

    con = duckdb.connect(":memory:", config={"allow_unsigned_extensions": True})
    con.execute("LOAD '{}';".format(ext_path.replace("'", "''")))


    # Get two commits from the repo to test traversal
    commits = con.execute(
        "SELECT commit_hash, parents FROM git(?) ORDER BY commit_time DESC LIMIT 10",
        [str(ROOT)]
    ).fetchall()

    if len(commits) < 2:
        pytest.skip("Not enough commits in repository for path testing")

    # Find a commit with a parent (not root)
    from_commit = None
    for commit_hash, parents_json in commits:
        import json
        parents = json.loads(parents_json)
        if len(parents) > 0:
            from_commit = commit_hash
            to_commit = parents[0]  # first parent
            break

    if not from_commit:
        pytest.skip("No commit with parent found for testing")

    # Query: total lines touched from from_commit to to_commit (first-parent path)
    query = """
    WITH RECURSIVE first_parent_path AS (
        SELECT commit_hash, parents, lines_added, lines_modified, lines_deleted, 0 AS depth
        FROM git(?)
        WHERE commit_hash = ?

        UNION ALL

        SELECT g.commit_hash, g.parents, g.lines_added, g.lines_modified, g.lines_deleted, p.depth + 1
        FROM first_parent_path p
        JOIN git(?) g 
          ON g.commit_hash = json_extract_string(p.parents, '$[0]')
        WHERE p.depth < 100 AND json_array_length(p.parents::JSON) > 0
    )
    SELECT 
        SUM(lines_added + lines_modified + lines_deleted) AS total_lines_touched,
        SUM(lines_added) AS total_added,
        SUM(lines_modified) AS total_modified,
        SUM(lines_deleted) AS total_deleted,
        COUNT(*) AS commits_in_path
    FROM first_parent_path
    WHERE commit_hash = ?;
    """

    result = con.execute(query, [str(ROOT), from_commit, str(ROOT), to_commit]).fetchone()

    if result:
        total_touched, total_added, total_modified, total_deleted, path_len = result
        # Basic validation
        assert total_touched is not None
        assert total_added is not None
        assert total_modified is not None
        assert total_deleted is not None
        assert path_len is not None
        assert total_touched >= 0
        assert total_added >= 0
        assert total_modified >= 0
        assert total_deleted >= 0
        assert path_len >= 1  # at least the from_commit
        # Consistency check
        assert total_touched == total_added + total_modified + total_deleted
