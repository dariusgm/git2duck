#!/usr/bin/env python3
"""
Example of using the git2duck DuckDB loadable extension from Python.
Loads the compiled extension and queries a repository to retrieve detailed
commit information including parent relationships.
"""

import argparse
import json
import os
from pathlib import Path
from datetime import datetime

import duckdb  # pip install duckdb


def find_extension(debug: bool) -> str:
    root = Path(__file__).resolve().parents[1]
    if os.name == "nt":
        name = "git2duck.dll"
    elif hasattr(os, "uname") and os.uname().sysname == "Darwin":  # type: ignore[attr-defined]
        name = "libgit2duck.dylib"
    else:
        name = "libgit2duck.so"
    build = "debug" if debug else "release"
    return str(root / "target" / build / name)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", help="Path or URL to a git repository")
    parser.add_argument("--debug", action="store_true", help="Load extension from target/debug")
    parser.add_argument("--limit", type=int, default=10, help="Number of commits to show")
    args = parser.parse_args()

    ext_path = find_extension(args.debug)
    if not Path(ext_path).exists():
        raise SystemExit(
            f"Extension not found at {ext_path}. Build it first with `cargo build{' --debug' if args.debug else ' --release'}`"
        )

    con = duckdb.connect(database=":memory:")
    con.execute(f"LOAD '{ext_path.replace(chr(39), chr(39)+chr(39))}';")

    print(f"Querying: {args.repo}\n")
    
    # Get total count
    count_result = con.execute("SELECT COUNT(*) FROM git(?);", [args.repo]).fetchone()
    print(f"Total commits: {count_result[0]}\n")
    
    # Show recent commits with details
    query = """
        SELECT 
            commit_hash,
            message,
            author_name,
            author_email,
            commit_time,
            parents
        FROM git(?)
        ORDER BY commit_time DESC
        LIMIT ?;
    """
    
    results = con.execute(query, [args.repo, args.limit]).fetchall()
    
    print(f"Recent {len(results)} commits:")
    print("=" * 80)
    
    for commit_hash, message, author_name, author_email, commit_time, parents_json in results:
        parents = json.loads(parents_json)
        commit_date = datetime.fromtimestamp(commit_time).strftime('%Y-%m-%d %H:%M:%S')
        
        # Truncate message to first line
        first_line = message.split('\n')[0]
        if len(first_line) > 60:
            first_line = first_line[:57] + '...'
        
        print(f"\nCommit: {commit_hash}")
        print(f"Author: {author_name} <{author_email}>")
        print(f"Date:   {commit_date}")
        print(f"Parents: {len(parents)} - {', '.join(p[:8] for p in parents) if parents else '(root commit)'}")
        print(f"\n    {first_line}")


if __name__ == "__main__":
    main()
