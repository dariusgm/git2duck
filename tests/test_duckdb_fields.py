import os
import json
from pathlib import Path

import duckdb
import pytest

from helper import find_extension, ROOT




@pytest.mark.parametrize(
    "repo",
    [
        str(ROOT),
        "https://github.com/dariusgm/quick-deck-mcp.git",
    ],
)
def test_git_table_returns_expected_columns(repo):
    # Build the loadable extension with entrypoint metadata
    os.system(f"cargo build --release --features ext --manifest-path {ROOT / 'Cargo.toml'} > /dev/null 2>&1")

    ext_path = find_extension(debug=False)
    assert Path(ext_path).exists(), f"Extension not found at {ext_path}"

    con = duckdb.connect(":memory:", config={"allow_unsigned_extensions": True})

    con.execute("LOAD '{}';".format(ext_path.replace("'", "''")))

    rows = con.execute(
        """
        SELECT commit_hash, message, author_name, author_email, commit_time, parents
        FROM git(?)
        LIMIT 100
        """,
        [repo],
    ).fetchall()

    assert len(rows) > 0
    for r in rows:
        commit_hash, message, author_name, author_email, commit_time, parents_json = r
        assert isinstance(commit_hash, str) and len(commit_hash) >= 7
        assert isinstance(message, str) and len(message) >= 1
        assert isinstance(author_name, str) and len(author_name) >= 1
        assert isinstance(author_email, str)
        assert ("@" in author_email) or (author_email == "<unknown>")
        assert isinstance(commit_time, int) and commit_time > 1104537600
        parents = json.loads(parents_json)
        assert isinstance(parents, list)
