use std::env;
use std::time::Instant;

use git2duck::register_git;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Args: <repo_path> [include_patches=true] [since=YYYY-MM-DD] [until=YYYY-MM-DD]
    let mut args = env::args().skip(1);
    let repo = args
        .next()
        .expect("usage: bench <repo_path> [include_patches=true] [since] [until]");
    let include_patches = args
        .next()
        .unwrap_or_else(|| "true".into())
        .parse::<bool>()
        .unwrap_or(true);
    let since = args.next();
    let until = args.next();

    // Open DuckDB and register table function in-process
    let con = duckdb::Connection::open_in_memory()?;
    register_git(&con)?;

    // Build JSON arg
    let mut arg = serde_json::json!({
        "repo": repo,
        "include_patches": include_patches,
    });
    if let Some(s) = &since {
        arg["since"] = serde_json::Value::String(s.clone());
    }
    if let Some(u) = &until {
        arg["until"] = serde_json::Value::String(u.clone());
    }

    // Optional where clause by commit_time boundaries
    let mut where_sql = String::new();
    if since.is_some() || until.is_some() {
        where_sql.push_str(" WHERE 1=1");
        if let Some(s) = since {
            where_sql.push_str(&format!(
                " AND to_timestamp(commit_time) >= TIMESTAMP '{} 00:00:00'",
                s
            ));
        }
        if let Some(u) = until {
            where_sql.push_str(&format!(
                " AND to_timestamp(commit_time) <= TIMESTAMP '{} 23:59:59'",
                u
            ));
        }
    }

    // Warm-up prepare
    let sql = format!("SELECT COUNT(*) FROM git(?){}", where_sql);
    let mut stmt = con.prepare(&sql)?;

    // Measure execution (forces full scan of table function)
    let start = Instant::now();
    let arg_s = arg.to_string();
    let mut rows = stmt.query([arg_s.as_str()])?;
    let cnt: i64 = rows.next()?.unwrap().get(0)?;
    let elapsed = start.elapsed();

    println!(
        "repo={}, include_patches={}, rows={}, elapsed_ms={}.",
        repo,
        include_patches,
        cnt,
        elapsed.as_millis()
    );

    // Optional: measure second pass to see cache effects
    let start2 = Instant::now();
    let arg_s2 = arg.to_string();
    let mut stmt2 = con.prepare(&sql)?;
    let mut rows2 = stmt2.query([arg_s2.as_str()])?;
    let cnt2: i64 = rows2.next()?.unwrap().get(0)?;
    let elapsed2 = start2.elapsed();
    println!(
        "second_pass rows={}, elapsed_ms={}.",
        cnt2,
        elapsed2.as_millis()
    );

    Ok(())
}
