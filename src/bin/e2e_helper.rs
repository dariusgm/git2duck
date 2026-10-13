use std::env;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let repo = env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("missing <repo> argument"))?;
    let con = duckdb::Connection::open_in_memory()?;
    git2duck::register_git(&con)?;
    let mut stmt = con.prepare("SELECT COUNT(*) FROM git(?);")?;
    let mut rows = stmt.query([repo.as_str()])?;
    if let Some(row) = rows.next()? {
        let count: i64 = row.get(0)?;
        println!("git: {} commits", count);
    } else {
        anyhow::bail!("no rows returned");
    }
    Ok(())
}
