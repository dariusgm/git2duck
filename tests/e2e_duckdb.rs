use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
extern crate serde_json;

// No external library needed when registering directly in-process.
fn ensure_lib_exists() {}

#[test]
fn e2e_include_patches_false_yields_empty_file_changes() -> anyhow::Result<()> {
    ensure_lib_exists();
    let con = duckdb::Connection::open_in_memory()?;
    git2duck::register_git(&con)?;
    git2duck::register_git_feature_metrics(&con)?;
    git2duck::register_git_tags(&con)?;

    // clone small public repo into a temp dir
    let tmp = tempfile::tempdir()?;
    let repo_path = clone_quick_deck_mcp(tmp.path())?;

    let arg = serde_json::json!({
        "repo": repo_path.to_string_lossy(),
        "include_patches": false
    })
    .to_string();

    let mut stmt = con.prepare("SELECT files_changed, file_changes FROM git(?);")?;
    let mut rows = stmt.query([arg.as_str()])?;
    let mut saw_any = false;
    while let Some(row) = rows.next()? {
        let files_changed: i64 = row.get(0)?;
        let file_changes_json: String = row.get(1)?;
        assert_eq!(
            files_changed, 0,
            "files_changed should be 0 when include_patches=false"
        );
        let v: serde_json::Value = serde_json::from_str(&file_changes_json)?;
        assert!(
            v.is_array() && v.as_array().unwrap().is_empty(),
            "file_changes should be an empty array"
        );
        saw_any = true;
    }
    assert!(saw_any, "expected at least one row");
    Ok(())
}

fn clone_quick_deck_mcp(into: &Path) -> anyhow::Result<PathBuf> {
    let url = "https://github.com/dariusgm/quick-deck-mcp.git";
    let repo_dir = into.join("quick-deck-mcp");
    let mut prep = gix::prepare_clone(url, &repo_dir)?;
    let _ = prep.fetch_only(gix::progress::Discard, &AtomicBool::new(false))?;
    Ok(repo_dir)
}

#[test]
fn e2e_git_function_quick_deck() -> anyhow::Result<()> {
    ensure_lib_exists();

    // open duckdb in-memory and register extension directly
    let con = duckdb::Connection::open_in_memory()?;
    git2duck::register_git(&con)?;

    // clone small public repo into a temp dir
    let tmp = tempfile::tempdir()?;
    let repo_path = clone_quick_deck_mcp(tmp.path())?;

    // run the table function with full field set including line metrics, files_changed, is_merge, and file_changes JSON
    let mut stmt = con.prepare("SELECT commit_hash, message, author_name, author_email, commit_time, parents, lines_added, lines_modified, lines_deleted, files_changed, is_merge, file_changes FROM git(?);")?;
    let mut rows = stmt.query([repo_path.to_string_lossy().as_ref()])?;

    let mut count = 0;
    while let Some(row) = rows.next()? {
        let hash: String = row.get(0)?;
        let message: String = row.get(1)?;
        let author: String = row.get(2)?;
        let email: String = row.get(3)?;
        let ts: i64 = row.get(4)?;
        let parents_json: String = row.get(5)?;
        let added: i64 = row.get(6)?;
        let modified: i64 = row.get(7)?;
        let deleted: i64 = row.get(8)?;
        let files_changed: i64 = row.get(9)?;
        let is_merge: bool = row.get(10)?;
        let file_changes_json: String = row.get(11)?;

        assert!(!hash.is_empty(), "commit hash should not be empty");
        assert!(!author.is_empty(), "author_name should not be empty");
        assert!(
            email.contains('@') || email == "<unknown>",
            "author_email should look like an email or <unknown>: {}",
            email
        );
        assert!(!message.is_empty(), "message should not be empty");
        // commit_time should be a plausible Unix timestamp (after 2005-01-01)
        assert!(ts > 1104537600, "commit_time seems invalid: {}", ts);
        // Parents should be valid JSON array
        let parents: Vec<String> =
            serde_json::from_str(&parents_json).expect("parents should be valid JSON array");
        assert!(
            added >= 0 && modified >= 0 && deleted >= 0,
            "line metrics must be non-negative"
        );
        assert!(files_changed >= 0, "files_changed must be non-negative");
        // is_merge should match parent count
        assert_eq!(
            is_merge,
            parents.len() > 1,
            "is_merge should be true iff parent count > 1"
        );
        // file_changes should be valid JSON array of objects
        let file_changes: serde_json::Value =
            serde_json::from_str(&file_changes_json).expect("file_changes should be valid JSON");
        assert!(file_changes.is_array(), "file_changes should be an array");
        if let Some(first) = file_changes.as_array().and_then(|a| a.first()) {
            if first.is_object() {
                let obj = first.as_object().unwrap();
                assert!(obj.contains_key("new_path"), "file_change missing new_path");
                assert!(
                    obj.contains_key("additions"),
                    "file_change missing additions"
                );
                assert!(
                    obj.contains_key("deletions"),
                    "file_change missing deletions"
                );
                assert!(obj.contains_key("old_path"), "file_change missing old_path");
                assert!(obj.contains_key("kind"), "file_change missing kind");
                assert!(
                    obj.contains_key("is_binary"),
                    "file_change missing is_binary"
                );
                assert!(obj.contains_key("hunks"), "file_change missing hunks");
                assert!(obj.contains_key("methods"), "file_change missing methods");
            }
        }
        count += 1;
    }

    assert!(count > 0, "expected at least one commit");

    // Minimal smoke test for git_feature_metrics: should execute without error
    git2duck::register_git_feature_metrics(&con)?;
    let mut stmt2 =
        con.prepare("SELECT feature, commit_time FROM git_feature_metrics(?) LIMIT 1;")?;
    let mut rows2 = stmt2.query([repo_path.to_string_lossy().as_ref()])?;
    let _ = rows2.next()?; // allow zero rows if no feature keys present

    // Minimal smoke test for git_tags: should execute without error
    git2duck::register_git_tags(&con)?;
    let mut stmt3 =
        con.prepare("SELECT tag, target, is_annotated, tag_time FROM git_tags(?) LIMIT 1;")?;
    let mut rows3 = stmt3.query([repo_path.to_string_lossy().as_ref()])?;
    let _ = rows3.next()?; // allow zero rows if no tags present
    Ok(())
}

#[test]
fn e2e_sql_example_runs() -> anyhow::Result<()> {
    ensure_lib_exists();
    let con = duckdb::Connection::open_in_memory()?;
    git2duck::register_git(&con)?;

    // Read example SQL and execute statements except any LOAD lines
    let example = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/duckdb_example.sql"),
    )?;
    for stmt in example.split(';') {
        let s = stmt.trim();
        if s.is_empty() {
            continue;
        }
        if s.to_ascii_uppercase().starts_with("LOAD ") {
            continue;
        }
        // Execute and ignore result; should not error
        con.execute_batch(&format!("{};", s))?;
    }
    Ok(())
}

#[test]
fn e2e_git_branches_basic() -> anyhow::Result<()> {
    ensure_lib_exists();

    let con = duckdb::Connection::open_in_memory()?;
    git2duck::register_git_branches(&con)?;

    // clone small public repo into a temp dir
    let tmp = tempfile::tempdir()?;
    let repo_path = clone_quick_deck_mcp(tmp.path())?;

    let mut stmt = con.prepare(
        "SELECT commit_time, branch, value FROM git_branches(?) ORDER BY branch, commit_time LIMIT 50;",
    )?;
    let mut rows = stmt.query([repo_path.to_string_lossy().as_ref()])?;

    let mut saw_any = false;
    while let Some(row) = rows.next()? {
        let ts: i64 = row.get(0)?;
        let branch: String = row.get(1)?;
        let value: i64 = row.get(2)?;

        // commit_time should be a plausible Unix timestamp (after 2005-01-01)
        assert!(ts > 1104537600, "commit_time seems invalid: {}", ts);
        assert!(!branch.is_empty(), "branch should not be empty");
        assert_eq!(value, 1, "value should always be 1 for git_branches");
        saw_any = true;
    }

    assert!(saw_any, "expected at least one git_branches row");
    Ok(())
}

#[test]
fn e2e_git_branches_json_params_smoke() -> anyhow::Result<()> {
    ensure_lib_exists();

    let con = duckdb::Connection::open_in_memory()?;
    git2duck::register_git_branches(&con)?;

    // clone small public repo into a temp dir
    let tmp = tempfile::tempdir()?;
    let repo_path = clone_quick_deck_mcp(tmp.path())?;

    // Use JSON with wide since_ts/until_ts bounds to exercise JSON parsing path.
    let arg = serde_json::json!({
        "repo": repo_path.to_string_lossy(),
        "since_ts": 0,
        "until_ts": 4_102_444_800_i64
    })
    .to_string();

    let mut stmt =
        con.prepare("SELECT commit_time, branch, value FROM git_branches(?) LIMIT 10;")?;
    let mut rows = stmt.query([arg.as_str()])?;

    // We do not assert on row count here; just ensure it executes without error
    // and yields rows with the expected column types if any are present.
    if let Some(row) = rows.next()? {
        let _ts: i64 = row.get(0)?;
        let _branch: String = row.get(1)?;
        let value: i64 = row.get(2)?;
        assert_eq!(value, 1, "value should be 1 when using JSON params as well");
    }

    Ok(())
}
