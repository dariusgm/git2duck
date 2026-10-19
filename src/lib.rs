use anyhow::Context;
use duckdb::{
    core::{DataChunkHandle, Inserter, LogicalTypeHandle, LogicalTypeId},
    vtab::{BindInfo, InitInfo, TableFunctionInfo, VTab},
    Connection, Result,
};
#[cfg(feature = "ext")]
use duckdb_loadable_macros::duckdb_entrypoint_c_api;
use gix::bstr::ByteSlice;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{atomic::AtomicBool, Mutex};

#[repr(C)]
struct GitBindData {
    repo: String,
}

#[cfg(feature = "ext")]
#[duckdb_entrypoint_c_api]
pub fn git2duck_init(con: Connection) -> Result<()> {
    register_git(&con)
        .and_then(|_| register_git_branches(&con))
        .and_then(|_| register_git_feature_metrics(&con))
        .and_then(|_| register_git_tags(&con))
}
#[cfg(feature = "ext")]
#[no_mangle]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn git2duck_init_c_api(
    db: *mut duckdb::ffi::duckdb_database,
) -> *const std::os::raw::c_char {
    extern "C" {
        fn git2duck_duckdb_init_c_api(
            db: *mut duckdb::ffi::duckdb_database,
        ) -> *const std::os::raw::c_char;
    }
    unsafe { git2duck_duckdb_init_c_api(db) }
}

struct CommitInfo {
    hash: String,
    message: String,
    author_name: String,
    author_email: String,
    commit_time: i64,
    parents: Vec<String>,
    lines_added: i64,
    lines_modified: i64,
    lines_deleted: i64,
    files_changed: i64,
    is_merge: bool,
    file_changes_json: String,
}

struct GitInitData {
    commits: Mutex<Vec<CommitInfo>>,
    index: Mutex<usize>,
}

struct GitVTab;

impl GitVTab {
    fn collect_commits(repo: &gix::Repository) -> anyhow::Result<Vec<CommitInfo>> {
        let mut seen: HashMap<gix::ObjectId, CommitInfo> = HashMap::new();
        let mut queue: VecDeque<gix::ObjectId> = VecDeque::new();

        // Start from all refs
        let refs = repo.references().context("list references")?;
        let mut iter = refs.all().context("iterate references")?;
        while let Some(Ok(mut r)) = iter.next() {
            if let Some(target) = r.try_id() {
                queue.push_back(target.into());
            } else if let Ok(id) = r.peel_to_id() {
                queue.push_back(id.detach());
            }
        }

        // BFS to collect all commits
        while let Some(oid) = queue.pop_front() {
            if seen.contains_key(&oid) {
                continue;
            }

            if let Ok(commit) = repo.find_commit(oid) {
                let hash = oid.to_string();
                let message = commit
                    .message_raw()
                    .ok()
                    .and_then(|m| m.to_str().ok())
                    .unwrap_or("<invalid utf-8>")
                    .to_string();

                let author = commit.author().ok();
                let author_name = author
                    .as_ref()
                    .and_then(|a| a.name.to_str().ok())
                    .unwrap_or("<unknown>")
                    .to_string();
                let author_email = author
                    .as_ref()
                    .and_then(|a| a.email.to_str().ok())
                    .unwrap_or("<unknown>")
                    .to_string();
                let commit_time = commit.time().ok().map(|t| t.seconds).unwrap_or(0);

                let mut parents = Vec::new();
                for parent_id in commit.parent_ids() {
                    let parent_oid = parent_id.detach();
                    parents.push(parent_oid.to_string());
                    queue.push_back(parent_oid);
                }

                let is_merge = parents.len() > 1;

                seen.insert(
                    oid,
                    CommitInfo {
                        hash,
                        message,
                        author_name,
                        author_email,
                        commit_time,
                        parents,
                        lines_added: 0,
                        lines_modified: 0,
                        lines_deleted: 0,
                        files_changed: 0,
                        is_merge,
                        file_changes_json: "[]".to_string(),
                    },
                );
            }
        }

        Ok(seen.into_values().collect())
    }
}

impl VTab for GitVTab {
    type InitData = GitInitData;
    type BindData = GitBindData;

    fn bind(bind: &BindInfo) -> Result<Self::BindData, Box<dyn std::error::Error>> {
        // Define output schema
        bind.add_result_column(
            "commit_hash",
            LogicalTypeHandle::from(LogicalTypeId::Varchar),
        );
        bind.add_result_column("message", LogicalTypeHandle::from(LogicalTypeId::Varchar));
        bind.add_result_column(
            "author_name",
            LogicalTypeHandle::from(LogicalTypeId::Varchar),
        );
        bind.add_result_column(
            "author_email",
            LogicalTypeHandle::from(LogicalTypeId::Varchar),
        );
        bind.add_result_column(
            "commit_time",
            LogicalTypeHandle::from(LogicalTypeId::Bigint),
        );
        bind.add_result_column("parents", LogicalTypeHandle::from(LogicalTypeId::Varchar));
        bind.add_result_column(
            "lines_added",
            LogicalTypeHandle::from(LogicalTypeId::Bigint),
        );
        bind.add_result_column(
            "lines_modified",
            LogicalTypeHandle::from(LogicalTypeId::Bigint),
        );
        bind.add_result_column(
            "lines_deleted",
            LogicalTypeHandle::from(LogicalTypeId::Bigint),
        );
        bind.add_result_column(
            "files_changed",
            LogicalTypeHandle::from(LogicalTypeId::Bigint),
        );
        bind.add_result_column("is_merge", LogicalTypeHandle::from(LogicalTypeId::Boolean));
        bind.add_result_column(
            "file_changes",
            LogicalTypeHandle::from(LogicalTypeId::Varchar),
        );
        let repo = bind.get_parameter(0).to_string();
        Ok(GitBindData { repo })
    }

    fn init(info: &InitInfo) -> Result<Self::InitData, Box<dyn std::error::Error>> {
        let bind_data: &GitBindData = unsafe { &*info.get_bind_data() };

        // Parse parameter: either a plain repo string or a JSON with {"repo": ..., "include_patches": bool}
        let mut repo_param = bind_data.repo.clone();
        let mut include_patches = true;
        let mut since_bound: Option<i64> = None;
        let mut until_bound: Option<i64> = None;
        let mut detect_renames = true;
        let mut detect_copies = true;
        let mut max_diff_bytes: usize = 1_000_000;
        if repo_param.trim_start().starts_with('{') {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&repo_param) {
                // Repo and feature flags
                if let Some(repo_s) = v.get("repo").and_then(|x| x.as_str()) {
                    repo_param = repo_s.to_string();
                }
                if let Some(ip) = v.get("include_patches").and_then(|x| x.as_bool()) {
                    include_patches = ip;
                }
                if let Some(dr) = v.get("detect_renames").and_then(|x| x.as_bool()) {
                    detect_renames = dr;
                }
                if let Some(dc) = v.get("detect_copies").and_then(|x| x.as_bool()) {
                    detect_copies = dc;
                }
                if let Some(mb) = v.get("max_diff_bytes").and_then(|x| x.as_u64()) {
                    max_diff_bytes = (mb as usize).max(1_000);
                }

                // Time bounds: epoch seconds
                if let Some(s) = v.get("since_ts").and_then(|x| x.as_i64()) {
                    since_bound = Some(s);
                }
                if let Some(u) = v.get("until_ts").and_then(|x| x.as_i64()) {
                    until_bound = Some(u);
                }

                // Time bounds: YYYY-MM-DD strings
                if since_bound.is_none() {
                    if let Some(s) = v.get("since").and_then(|x| x.as_str()) {
                        if let Ok(date) = time::Date::parse(
                            s,
                            time::macros::format_description!("[year]-[month]-[day]"),
                        ) {
                            let t = date
                                .with_hms(0, 0, 0)
                                .unwrap()
                                .assume_utc()
                                .unix_timestamp();
                            since_bound = Some(t);
                        }
                    }
                }
                if until_bound.is_none() {
                    if let Some(u) = v.get("until").and_then(|x| x.as_str()) {
                        if let Ok(date) = time::Date::parse(
                            u,
                            time::macros::format_description!("[year]-[month]-[day]"),
                        ) {
                            let t = date
                                .with_hms(23, 59, 59)
                                .unwrap()
                                .assume_utc()
                                .unix_timestamp();
                            until_bound = Some(t);
                        }
                    }
                }
            }
        }

        // Support local paths and basic remote URLs
        let is_url = repo_param.starts_with("http://")
            || repo_param.starts_with("https://")
            || repo_param.starts_with("git@");

        let commits = if is_url {
            // Clone remote into a temp dir
            tempfile::tempdir()
                .context("create temp dir")
                .and_then(|tmp| {
                    let prep = gix::prepare_clone(repo_param.as_str(), tmp.path().join("repo"));
                    let (_repo, _outcome) = prep
                        .map_err(anyhow::Error::from)?
                        .fetch_only(gix::progress::Discard, &AtomicBool::new(false))
                        .context("gix fetch_only")?;
                    let repo_path = tmp.path().join("repo");
                    let repo = gix::open(&repo_path).context("open cloned repo")?;
                    let mut commits = GitVTab::collect_commits(&repo)?;
                    if since_bound.is_some() || until_bound.is_some() {
                        commits.retain(|c| {
                            let t = c.commit_time;
                            if let Some(s) = since_bound {
                                if t < s {
                                    return false;
                                }
                            }
                            if let Some(u) = until_bound {
                                if t > u {
                                    return false;
                                }
                            }
                            true
                        });
                    }
                    // compute line metrics while tempdir is alive
                    compute_line_metrics(
                        &repo_path,
                        &mut commits,
                        include_patches,
                        detect_renames,
                        detect_copies,
                        max_diff_bytes,
                    );
                    Ok(commits)
                })
                .unwrap_or_else(|_| Vec::new())
        } else {
            gix::open(Path::new(&repo_param))
                .context("open repo")
                .and_then(|repo| {
                    let mut commits = GitVTab::collect_commits(&repo)?;
                    if since_bound.is_some() || until_bound.is_some() {
                        commits.retain(|c| {
                            let t = c.commit_time;
                            if let Some(s) = since_bound {
                                if t < s {
                                    return false;
                                }
                            }
                            if let Some(u) = until_bound {
                                if t > u {
                                    return false;
                                }
                            }
                            true
                        });
                    }
                    let repo_path = Path::new(&repo_param).to_path_buf();
                    compute_line_metrics(
                        &repo_path,
                        &mut commits,
                        include_patches,
                        detect_renames,
                        detect_copies,
                        max_diff_bytes,
                    );
                    Ok(commits)
                })
                .unwrap_or_else(|_| Vec::new())
        };

        Ok(GitInitData {
            commits: Mutex::new(commits),
            index: Mutex::new(0),
        })
    }

    fn func(
        func: &TableFunctionInfo<Self>,
        output: &mut DataChunkHandle,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let init_data = func.get_init_data();

        let commits = init_data.commits.lock().unwrap();
        let mut index = init_data.index.lock().unwrap();

        let remaining = commits.len() - *index;
        if remaining == 0 {
            output.set_len(0);
            return Ok(());
        }

        // Return up to VECTOR_SIZE rows per call (DuckDB typical is 2048)
        let chunk_size = remaining.min(2048);

        let hash_vec = output.flat_vector(0);
        let msg_vec = output.flat_vector(1);
        let author_name_vec = output.flat_vector(2);
        let author_email_vec = output.flat_vector(3);
        let mut time_vec = output.flat_vector(4);
        let parents_vec = output.flat_vector(5);
        let mut added_vec = output.flat_vector(6);
        let mut modified_vec = output.flat_vector(7);
        let mut deleted_vec = output.flat_vector(8);
        let mut files_vec = output.flat_vector(9);
        let mut is_merge_vec = output.flat_vector(10);
        let file_changes_vec = output.flat_vector(11);

        // Get direct access to i64 buffers
        let time_slice = unsafe { time_vec.as_mut_slice::<i64>() };
        let added_slice = unsafe { added_vec.as_mut_slice::<i64>() };
        let modified_slice = unsafe { modified_vec.as_mut_slice::<i64>() };
        let deleted_slice = unsafe { deleted_vec.as_mut_slice::<i64>() };
        let files_slice = unsafe { files_vec.as_mut_slice::<i64>() };
        let is_merge_slice = unsafe { is_merge_vec.as_mut_slice::<bool>() };

        for i in 0..chunk_size {
            let commit = &commits[*index + i];

            hash_vec.insert(i, std::ffi::CString::new(commit.hash.as_str())?);
            msg_vec.insert(i, std::ffi::CString::new(commit.message.as_str())?);
            author_name_vec.insert(i, std::ffi::CString::new(commit.author_name.as_str())?);
            author_email_vec.insert(i, std::ffi::CString::new(commit.author_email.as_str())?);

            // Write timestamp directly to i64 buffer
            time_slice[i] = commit.commit_time;

            // Encode parents as JSON array
            let parents_json =
                serde_json::to_string(&commit.parents).unwrap_or_else(|_| "[]".to_string());
            parents_vec.insert(i, std::ffi::CString::new(parents_json)?);
            added_slice[i] = commit.lines_added;
            modified_slice[i] = commit.lines_modified;
            deleted_slice[i] = commit.lines_deleted;
            files_slice[i] = commit.files_changed;
            is_merge_slice[i] = commit.is_merge;
            file_changes_vec.insert(
                i,
                std::ffi::CString::new(commit.file_changes_json.as_str())?,
            );
        }

        *index += chunk_size;
        output.set_len(chunk_size);
        Ok(())
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        Some(vec![LogicalTypeHandle::from(LogicalTypeId::Varchar)])
    }
}

pub fn register_git(con: &Connection) -> Result<()> {
    con.register_table_function::<GitVTab>("git")?;
    Ok(())
}

// BranchesVTab: emits one row per (branch, commit_time) for reachable commits, value=1
struct BranchesRow {
    commit_time: i64,
    branch: String,
    value: i64,
}

#[repr(C)]
struct BranchesBindData {
    repo: String,
}

struct BranchesInitData {
    rows: Mutex<Vec<BranchesRow>>,
    index: Mutex<usize>,
}

struct BranchesVTab;

impl BranchesVTab {
    // no aggregation helpers needed
}

impl VTab for BranchesVTab {
    type InitData = BranchesInitData;
    type BindData = BranchesBindData;

    fn bind(bind: &BindInfo) -> Result<Self::BindData, Box<dyn std::error::Error>> {
        // Output schema: commit_time (BIGINT epoch seconds), branch (VARCHAR), value (BIGINT)
        bind.add_result_column(
            "commit_time",
            LogicalTypeHandle::from(LogicalTypeId::Bigint),
        );
        bind.add_result_column("branch", LogicalTypeHandle::from(LogicalTypeId::Varchar));
        bind.add_result_column("value", LogicalTypeHandle::from(LogicalTypeId::Bigint));

        // Accept one VARCHAR parameter: either repo path/URL or JSON {repo, since, until}
        let repo = bind.get_parameter(0).to_string();
        Ok(BranchesBindData { repo })
    }

    fn init(info: &InitInfo) -> Result<Self::InitData, Box<dyn std::error::Error>> {
        let bind_data: &BranchesBindData = unsafe { &*info.get_bind_data() };

        // Parse parameter JSON if provided
        let mut repo_param = bind_data.repo.clone();
        let mut since_bound: Option<i64> = None;
        let mut until_bound: Option<i64> = None;
        if repo_param.trim_start().starts_with('{') {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&repo_param) {
                if let Some(repo_s) = v.get("repo").and_then(|x| x.as_str()) {
                    repo_param = repo_s.to_string();
                }
                // since/until as YYYY-MM-DD
                if let Some(s) = v.get("since").and_then(|x| x.as_str()) {
                    if let Ok(date) = time::Date::parse(
                        s,
                        time::macros::format_description!("[year]-[month]-[day]"),
                    ) {
                        since_bound = Some(
                            date.with_hms(0, 0, 0)
                                .unwrap()
                                .assume_utc()
                                .unix_timestamp(),
                        );
                    }
                }
                if let Some(u) = v.get("until").and_then(|x| x.as_str()) {
                    if let Ok(date) = time::Date::parse(
                        u,
                        time::macros::format_description!("[year]-[month]-[day]"),
                    ) {
                        until_bound = Some(
                            date.with_hms(23, 59, 59)
                                .unwrap()
                                .assume_utc()
                                .unix_timestamp(),
                        );
                    }
                }
                // since_ts/until_ts epoch seconds override
                if let Some(s) = v.get("since_ts").and_then(|x| x.as_i64()) {
                    since_bound = Some(s);
                }
                if let Some(u) = v.get("until_ts").and_then(|x| x.as_i64()) {
                    until_bound = Some(u);
                }
            }
        }

        let is_url = repo_param.starts_with("http://")
            || repo_param.starts_with("https://")
            || repo_param.starts_with("git@");

        // Helper to emit one row per reachable commit on each branch within optional bounds
        fn compute_rows(
            repo: &gix::Repository,
            since_bound: Option<i64>,
            until_bound: Option<i64>,
        ) -> Vec<BranchesRow> {
            let mut rows: Vec<BranchesRow> = Vec::new();
            let refs = match repo.references() {
                Ok(r) => r,
                Err(_) => return rows,
            };
            let mut iter = match refs.all() {
                Ok(i) => i,
                Err(_) => return rows,
            };
            while let Some(Ok(mut r)) = iter.next() {
                let name = r.name().as_bstr().to_str().unwrap_or("").to_string();
                // Skip non-branch refs like tags unless they are heads/remotes
                if !name.starts_with("refs/heads/") && !name.starts_with("refs/remotes/") {
                    continue;
                }
                // resolve tip
                let head_id = if let Some(id) = r.try_id() {
                    id.into()
                } else if let Ok(id) = r.peel_to_id() {
                    id.detach()
                } else {
                    continue;
                };
                // walk reachable commits and emit per-commit rows
                let mut queue: VecDeque<gix::ObjectId> = VecDeque::new();
                let mut seen: HashMap<gix::ObjectId, ()> = HashMap::new();
                queue.push_back(head_id);
                while let Some(oid) = queue.pop_front() {
                    if seen.contains_key(&oid) {
                        continue;
                    }
                    seen.insert(oid, ());
                    if let Ok(commit) = repo.find_commit(oid) {
                        if let Ok(time) = commit.time() {
                            let ts = time.seconds;
                            let in_bounds = since_bound.map(|s| ts >= s).unwrap_or(true)
                                && until_bound.map(|u| ts <= u).unwrap_or(true);
                            if in_bounds {
                                rows.push(BranchesRow {
                                    commit_time: ts,
                                    branch: name.clone(),
                                    value: 1,
                                });
                            }
                        }
                        for p in commit.parent_ids() {
                            queue.push_back(p.detach());
                        }
                    }
                }
            }
            // stable sort by (branch, commit_time)
            rows.sort_by(|a, b| {
                a.branch
                    .cmp(&b.branch)
                    .then(a.commit_time.cmp(&b.commit_time))
            });
            rows
        }

        let rows = if is_url {
            tempfile::tempdir()
                .context("create temp dir")
                .and_then(|tmp| {
                    let prep = gix::prepare_clone(repo_param.as_str(), tmp.path().join("repo"));
                    let (_repo, _outcome) = prep
                        .map_err(anyhow::Error::from)?
                        .fetch_only(gix::progress::Discard, &AtomicBool::new(false))
                        .context("gix fetch_only")?;
                    let repo = gix::open(tmp.path().join("repo")).context("open cloned repo")?;
                    Ok(compute_rows(&repo, since_bound, until_bound))
                })
                .unwrap_or_else(|_| Vec::new())
        } else {
            gix::open(Path::new(&repo_param))
                .context("open repo")
                .map(|repo| compute_rows(&repo, since_bound, until_bound))
                .unwrap_or_else(|_| Vec::new())
        };

        Ok(BranchesInitData {
            rows: Mutex::new(rows),
            index: Mutex::new(0),
        })
    }

    fn func(
        func: &TableFunctionInfo<Self>,
        output: &mut DataChunkHandle,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let init_data = func.get_init_data();
        let rows = init_data.rows.lock().unwrap();
        let mut index = init_data.index.lock().unwrap();

        let remaining = rows.len().saturating_sub(*index);
        if remaining == 0 {
            output.set_len(0);
            return Ok(());
        }
        let chunk_size = remaining.min(2048);

        let mut ts_vec = output.flat_vector(0);
        let branch_vec = output.flat_vector(1);
        let mut value_vec = output.flat_vector(2);

        let ts_slice = unsafe { ts_vec.as_mut_slice::<i64>() };
        let value_slice = unsafe { value_vec.as_mut_slice::<i64>() };

        for i in 0..chunk_size {
            let row = &rows[*index + i];
            ts_slice[i] = row.commit_time;
            branch_vec.insert(i, std::ffi::CString::new(row.branch.as_str())?);
            value_slice[i] = row.value;
        }

        *index += chunk_size;
        output.set_len(chunk_size);
        Ok(())
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        // Single VARCHAR parameter (repo or JSON)
        Some(vec![LogicalTypeHandle::from(LogicalTypeId::Varchar)])
    }
}

pub fn register_git_branches(con: &Connection) -> Result<()> {
    con.register_table_function::<BranchesVTab>("git_branches")?;
    Ok(())
}

// FeatureMetricsVTab: emits one row per commit with a detected feature key (no aggregation)
struct FMRow {
    feature: String,
    commit_time: i64,
    lines_added: i64,
    lines_deleted: i64,
    lines_changed: i64,
    net_lines_changed: i64,
    merges: i64,
}

#[repr(C)]
struct FMBindData {
    repo: String,
}

struct FMInitData {
    rows: Mutex<Vec<FMRow>>,
    index: Mutex<usize>,
}

struct FeatureMetricsVTab;

impl FeatureMetricsVTab {
    // Extract simple feature key like ABC-123 from subject/message without regex crate
    fn extract_feature_key(msg: &str) -> Option<String> {
        let bytes = msg.as_bytes();
        let n = bytes.len();
        let mut i = 0;
        while i < n {
            // Scan for uppercase letter
            let b = bytes[i];
            if b.is_ascii_uppercase() {
                // consume [A-Z][A-Z0-9]+
                let mut j = i + 1;
                while j < n {
                    let c = bytes[j];
                    if c.is_ascii_uppercase() || c.is_ascii_digit() {
                        j += 1;
                    } else {
                        break;
                    }
                }
                // need at least 2 total chars in the prefix
                if j - i >= 2 && j < n && bytes[j] == b'-' {
                    // consume -[0-9]+
                    let mut k = j + 1;
                    let mut have_digit = false;
                    while k < n {
                        let c2 = bytes[k];
                        if c2.is_ascii_digit() {
                            have_digit = true;
                            k += 1;
                        } else {
                            break;
                        }
                    }
                    if have_digit {
                        return Some(msg[i..k].to_string());
                    }
                }
                i = j;
            }
            i += 1;
        }
        None
    }
}

impl VTab for FeatureMetricsVTab {
    type InitData = FMInitData;
    type BindData = FMBindData;

    fn bind(bind: &BindInfo) -> Result<Self::BindData, Box<dyn std::error::Error>> {
        // Output schema
        bind.add_result_column("feature", LogicalTypeHandle::from(LogicalTypeId::Varchar));
        bind.add_result_column(
            "commit_time",
            LogicalTypeHandle::from(LogicalTypeId::Bigint),
        );
        bind.add_result_column(
            "lines_added",
            LogicalTypeHandle::from(LogicalTypeId::Bigint),
        );
        bind.add_result_column(
            "lines_deleted",
            LogicalTypeHandle::from(LogicalTypeId::Bigint),
        );
        bind.add_result_column(
            "lines_changed",
            LogicalTypeHandle::from(LogicalTypeId::Bigint),
        );
        bind.add_result_column(
            "net_lines_changed",
            LogicalTypeHandle::from(LogicalTypeId::Bigint),
        );
        bind.add_result_column("merges", LogicalTypeHandle::from(LogicalTypeId::Bigint));

        let repo = bind.get_parameter(0).to_string();
        Ok(FMBindData { repo })
    }

    fn init(info: &InitInfo) -> Result<Self::InitData, Box<dyn std::error::Error>> {
        let bind_data: &FMBindData = unsafe { &*info.get_bind_data() };
        let mut repo_param = bind_data.repo.clone();
        let mut since_bound: Option<i64> = None;
        let mut until_bound: Option<i64> = None;
        if repo_param.trim_start().starts_with('{') {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&repo_param) {
                if let Some(repo_s) = v.get("repo").and_then(|x| x.as_str()) {
                    repo_param = repo_s.to_string();
                }
                if since_bound.is_none() {
                    if let Some(s) = v.get("since").and_then(|x| x.as_str()) {
                        if let Ok(date) = time::Date::parse(
                            s,
                            time::macros::format_description!("[year]-[month]-[day]"),
                        ) {
                            since_bound = Some(
                                date.with_hms(0, 0, 0)
                                    .unwrap()
                                    .assume_utc()
                                    .unix_timestamp(),
                            );
                        }
                    }
                }
                if until_bound.is_none() {
                    if let Some(u) = v.get("until").and_then(|x| x.as_str()) {
                        if let Ok(date) = time::Date::parse(
                            u,
                            time::macros::format_description!("[year]-[month]-[day]"),
                        ) {
                            until_bound = Some(
                                date.with_hms(23, 59, 59)
                                    .unwrap()
                                    .assume_utc()
                                    .unix_timestamp(),
                            );
                        }
                    }
                }
                if let Some(s) = v.get("since_ts").and_then(|x| x.as_i64()) {
                    since_bound = Some(s);
                }
                if let Some(u) = v.get("until_ts").and_then(|x| x.as_i64()) {
                    until_bound = Some(u);
                }
            }
        }

        let is_url = repo_param.starts_with("http://")
            || repo_param.starts_with("https://")
            || repo_param.starts_with("git@");

        fn compute_rows(
            repo: &gix::Repository,
            repo_path: &std::path::Path,
            since_bound: Option<i64>,
            until_bound: Option<i64>,
        ) -> Vec<FMRow> {
            // Collect commits across refs
            let mut commits = GitVTab::collect_commits(repo).unwrap_or_default();
            if since_bound.is_some() || until_bound.is_some() {
                commits.retain(|c| {
                    let t = c.commit_time;
                    if let Some(s) = since_bound {
                        if t < s {
                            return false;
                        }
                    }
                    if let Some(u) = until_bound {
                        if t > u {
                            return false;
                        }
                    }
                    true
                });
            }
            // Compute line metrics without patches to keep memory low
            compute_line_metrics(
                repo_path,
                &mut commits,
                /*include_patches*/ false,
                /*detect_renames*/ true,
                /*detect_copies*/ true,
                /*max_diff_bytes*/ 1_000_000,
            );

            let mut rows: Vec<FMRow> = Vec::new();
            for c in &commits {
                let Some(feature) = FeatureMetricsVTab::extract_feature_key(&c.message) else {
                    continue;
                };
                let added = c.lines_added;
                let deleted = c.lines_deleted;
                let modified = c.lines_modified;
                let merges = if c.parents.len() > 1 { 1 } else { 0 };
                let lines_changed = added + deleted + modified;
                let net = added - deleted;
                rows.push(FMRow {
                    feature,
                    commit_time: c.commit_time,
                    lines_added: added,
                    lines_deleted: deleted,
                    lines_changed,
                    net_lines_changed: net,
                    merges,
                });
            }
            rows.sort_by(|a, b| {
                a.feature
                    .cmp(&b.feature)
                    .then(a.commit_time.cmp(&b.commit_time))
            });
            rows
        }

        let rows = if is_url {
            tempfile::tempdir()
                .context("create temp dir")
                .and_then(|tmp| {
                    let prep = gix::prepare_clone(repo_param.as_str(), tmp.path().join("repo"));
                    let (_repo, _outcome) = prep
                        .map_err(anyhow::Error::from)?
                        .fetch_only(gix::progress::Discard, &AtomicBool::new(false))
                        .context("gix fetch_only")?;
                    let repo_path = tmp.path().join("repo");
                    let repo = gix::open(&repo_path).context("open cloned repo")?;
                    Ok(compute_rows(&repo, &repo_path, since_bound, until_bound))
                })
                .unwrap_or_else(|_| Vec::new())
        } else {
            let repo_path = std::path::Path::new(&repo_param).to_path_buf();
            gix::open(&repo_path)
                .context("open repo")
                .map(|repo| compute_rows(&repo, &repo_path, since_bound, until_bound))
                .unwrap_or_else(|_| Vec::new())
        };

        Ok(FMInitData {
            rows: Mutex::new(rows),
            index: Mutex::new(0),
        })
    }

    fn func(
        func: &TableFunctionInfo<Self>,
        output: &mut DataChunkHandle,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let init_data = func.get_init_data();
        let rows = init_data.rows.lock().unwrap();
        let mut index = init_data.index.lock().unwrap();
        let remaining = rows.len().saturating_sub(*index);
        if remaining == 0 {
            output.set_len(0);
            return Ok(());
        }
        let chunk_size = remaining.min(2048);

        let feature_vec = output.flat_vector(0);
        let mut ts_vec = output.flat_vector(1);
        let mut added_vec = output.flat_vector(2);
        let mut deleted_vec = output.flat_vector(3);
        let mut changed_vec = output.flat_vector(4);
        let mut net_vec = output.flat_vector(5);
        let mut merges_vec = output.flat_vector(6);

        let ts_slice = unsafe { ts_vec.as_mut_slice::<i64>() };
        let added_slice = unsafe { added_vec.as_mut_slice::<i64>() };
        let deleted_slice = unsafe { deleted_vec.as_mut_slice::<i64>() };
        let changed_slice = unsafe { changed_vec.as_mut_slice::<i64>() };
        let net_slice = unsafe { net_vec.as_mut_slice::<i64>() };
        let merges_slice = unsafe { merges_vec.as_mut_slice::<i64>() };

        for i in 0..chunk_size {
            let row = &rows[*index + i];
            feature_vec.insert(i, std::ffi::CString::new(row.feature.as_str())?);
            ts_slice[i] = row.commit_time;
            added_slice[i] = row.lines_added;
            deleted_slice[i] = row.lines_deleted;
            changed_slice[i] = row.lines_changed;
            net_slice[i] = row.net_lines_changed;
            merges_slice[i] = row.merges;
        }

        *index += chunk_size;
        output.set_len(chunk_size);
        Ok(())
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        Some(vec![LogicalTypeHandle::from(LogicalTypeId::Varchar)])
    }
}

pub fn register_git_feature_metrics(con: &Connection) -> Result<()> {
    con.register_table_function::<FeatureMetricsVTab>("git_feature_metrics")?;
    Ok(())
}

// TagsVTab: exposes git tags with resolved targets and optional annotated metadata
struct TagsRow {
    tag: String,
    target: String,
    is_annotated: bool,
    tagger_name: String,
    tagger_email: String,
    tag_time: i64,
    message: String,
}

#[repr(C)]
struct TagsBindData {
    repo: String,
}

struct TagsInitData {
    rows: Mutex<Vec<TagsRow>>,
    index: Mutex<usize>,
}

struct TagsVTab;

impl VTab for TagsVTab {
    type InitData = TagsInitData;
    type BindData = TagsBindData;

    fn bind(bind: &BindInfo) -> Result<Self::BindData, Box<dyn std::error::Error>> {
        // Output schema
        bind.add_result_column("tag", LogicalTypeHandle::from(LogicalTypeId::Varchar));
        bind.add_result_column("target", LogicalTypeHandle::from(LogicalTypeId::Varchar));
        bind.add_result_column(
            "is_annotated",
            LogicalTypeHandle::from(LogicalTypeId::Boolean),
        );
        bind.add_result_column(
            "tagger_name",
            LogicalTypeHandle::from(LogicalTypeId::Varchar),
        );
        bind.add_result_column(
            "tagger_email",
            LogicalTypeHandle::from(LogicalTypeId::Varchar),
        );
        bind.add_result_column("tag_time", LogicalTypeHandle::from(LogicalTypeId::Bigint));
        bind.add_result_column("message", LogicalTypeHandle::from(LogicalTypeId::Varchar));

        let repo = bind.get_parameter(0).to_string();
        Ok(TagsBindData { repo })
    }

    fn init(info: &InitInfo) -> Result<Self::InitData, Box<dyn std::error::Error>> {
        let bind_data: &TagsBindData = unsafe { &*info.get_bind_data() };

        let mut repo_param = bind_data.repo.clone();
        if repo_param.trim_start().starts_with('{') {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&repo_param) {
                if let Some(repo_s) = v.get("repo").and_then(|x| x.as_str()) {
                    repo_param = repo_s.to_string();
                }
            }
        }

        let is_url = repo_param.starts_with("http://")
            || repo_param.starts_with("https://")
            || repo_param.starts_with("git@");

        fn collect_tags(repo: &gix::Repository) -> Vec<TagsRow> {
            let mut out: Vec<TagsRow> = Vec::new();
            let refs = match repo.references() {
                Ok(r) => r,
                Err(_) => return out,
            };
            let mut it = match refs.all() {
                Ok(i) => i,
                Err(_) => return out,
            };
            while let Some(Ok(mut r)) = it.next() {
                let name = r.name().as_bstr().to_str().unwrap_or("").to_string();
                if !name.starts_with("refs/tags/") {
                    continue;
                }
                // try get target id
                let target_oid = if let Some(id) = r.try_id() {
                    id.into()
                } else if let Ok(id) = r.peel_to_id() {
                    id.detach()
                } else {
                    continue;
                };

                // Attempt to read annotated tag object
                let mut is_annotated = false;
                let tagger_name = String::new();
                let tagger_email = String::new();
                let tag_time: i64 = 0;
                let message = String::new();
                let mut resolved_target = target_oid;

                if let Ok(obj) = repo.find_object(target_oid) {
                    if let Ok(tag) = obj.try_into_tag() {
                        is_annotated = true;
                        // Peel to the tagged object (often commit); fallback to original target on error
                        if let Ok(tgt) = tag.target_id() {
                            resolved_target = tgt.detach();
                        }
                        // tagger_name/email/time and message intentionally left as defaults to keep API stable
                    }
                }

                out.push(TagsRow {
                    tag: name,
                    target: resolved_target.to_string(),
                    is_annotated,
                    tagger_name,
                    tagger_email,
                    tag_time,
                    message,
                });
            }
            // Sort stable by tag name
            out.sort_by(|a, b| a.tag.cmp(&b.tag));
            out
        }

        let rows = if is_url {
            tempfile::tempdir()
                .context("create temp dir")
                .and_then(|tmp| {
                    let prep = gix::prepare_clone(repo_param.as_str(), tmp.path().join("repo"));
                    let (_repo, _outcome) = prep
                        .map_err(anyhow::Error::from)?
                        .fetch_only(gix::progress::Discard, &AtomicBool::new(false))
                        .context("gix fetch_only")?;
                    let repo = gix::open(tmp.path().join("repo")).context("open cloned repo")?;
                    Ok(collect_tags(&repo))
                })
                .unwrap_or_else(|_| Vec::new())
        } else {
            gix::open(Path::new(&repo_param))
                .context("open repo")
                .map(|repo| collect_tags(&repo))
                .unwrap_or_else(|_| Vec::new())
        };

        Ok(TagsInitData {
            rows: Mutex::new(rows),
            index: Mutex::new(0),
        })
    }

    fn func(
        func: &TableFunctionInfo<Self>,
        output: &mut DataChunkHandle,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let init_data = func.get_init_data();
        let rows = init_data.rows.lock().unwrap();
        let mut index = init_data.index.lock().unwrap();

        let remaining = rows.len().saturating_sub(*index);
        if remaining == 0 {
            output.set_len(0);
            return Ok(());
        }
        let chunk_size = remaining.min(2048);

        let tag_vec = output.flat_vector(0);
        let target_vec = output.flat_vector(1);
        let mut ann_vec = output.flat_vector(2);
        let tagger_name_vec = output.flat_vector(3);
        let tagger_email_vec = output.flat_vector(4);
        let mut time_vec = output.flat_vector(5);
        let message_vec = output.flat_vector(6);

        let ann_slice = unsafe { ann_vec.as_mut_slice::<bool>() };
        let time_slice = unsafe { time_vec.as_mut_slice::<i64>() };

        for i in 0..chunk_size {
            let row = &rows[*index + i];
            tag_vec.insert(i, std::ffi::CString::new(row.tag.as_str())?);
            target_vec.insert(i, std::ffi::CString::new(row.target.as_str())?);
            ann_slice[i] = row.is_annotated;
            tagger_name_vec.insert(i, std::ffi::CString::new(row.tagger_name.as_str())?);
            tagger_email_vec.insert(i, std::ffi::CString::new(row.tagger_email.as_str())?);
            time_slice[i] = row.tag_time;
            message_vec.insert(i, std::ffi::CString::new(row.message.as_str())?);
        }

        *index += chunk_size;
        output.set_len(chunk_size);
        Ok(())
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        Some(vec![LogicalTypeHandle::from(LogicalTypeId::Varchar)])
    }
}

pub fn register_git_tags(con: &Connection) -> Result<()> {
    con.register_table_function::<TagsVTab>("git_tags")?;
    Ok(())
}

// Compute per-commit line metrics using gix trees/blobs.
// Processes commits in parallel chunks, with each worker opening its own Repository handle.
fn compute_line_metrics(
    repo_path: &std::path::Path,
    commits: &mut [CommitInfo],
    include_patches: bool,
    detect_renames: bool,
    detect_copies: bool,
    max_diff_bytes: usize,
) {
    use rayon::prelude::*;
    use rustc_hash::FxHashMap;
    use similar::TextDiff;

    // Helper: check binary (NUL byte) and UTF-8 validity
    // Optimized with memchr for faster byte scanning
    #[inline(always)]
    fn is_text(bytes: &[u8]) -> bool {
        // Fast path: check for NUL byte with memchr (much faster than contains)
        if memchr::memchr(0, bytes).is_some() {
            return false;
        }
        std::str::from_utf8(bytes).is_ok()
    }

    // Type alias for tree changes result to avoid clippy complexity warning
    type TreeChanges = (
        Vec<(String, gix::ObjectId, gix::ObjectId)>, // modifications
        Vec<(String, gix::ObjectId)>,                // additions
        Vec<(String, gix::ObjectId)>,                // deletions
    );

    // Helper: enumerate tree changes using gix native tree-diff.
    // Returns (modified files with old/new ids, added files, deleted files).
    #[inline]
    fn tree_changes(
        repo: &gix::Repository,
        old_tree: gix::ObjectId,
        new_tree: gix::ObjectId,
    ) -> anyhow::Result<TreeChanges> {
        use gix::object::tree::diff::Change;
        let old_tree_obj = repo.find_object(old_tree)?.peel_to_tree()?;
        let new_tree_obj = repo.find_object(new_tree)?.peel_to_tree()?;

        let mut mods = Vec::new();
        let mut adds = Vec::new();
        let mut dels = Vec::new();

        old_tree_obj
            .changes()?
            .for_each_to_obtain_tree(&new_tree_obj, |change| {
                match change {
                    Change::Modification {
                        location,
                        previous_entry_mode: _,
                        entry_mode: _,
                        previous_id,
                        id,
                        ..
                    } => {
                        mods.push((location.to_string(), previous_id.detach(), id.detach()));
                    }
                    Change::Addition { location, id, .. } => {
                        adds.push((location.to_string(), id.detach()));
                    }
                    Change::Deletion { location, id, .. } => {
                        dels.push((location.to_string(), id.detach()));
                    }
                    Change::Rewrite {
                        source_location,
                        location,
                        source_id,
                        id,
                        ..
                    } => {
                        // Treat rewrite as delete + add (rename with content change)
                        dels.push((source_location.to_string(), source_id.detach()));
                        adds.push((location.to_string(), id.detach()));
                    }
                }
                Ok::<_, gix::Error>(std::ops::ControlFlow::Continue(()))
            })?;

        Ok((mods, adds, dels))
    }

    // Cap very large files to avoid pathological diffs
    let max_bytes_cfg: usize = max_diff_bytes;

    const BLOB_CACHE_LIMIT: usize = 65536;

    #[inline(always)]
    fn get_blob_bytes(
        repo: &gix::Repository,
        oid: gix::ObjectId,
        cache: &mut FxHashMap<gix::ObjectId, Vec<u8>>,
    ) -> Vec<u8> {
        // Check cache first
        if let Some(v) = cache.get(&oid) {
            return v.clone();
        }

        // Load blob from repo
        let bytes = repo
            .find_object(oid)
            .ok()
            .and_then(|o| o.try_into_blob().ok())
            .map(|b| b.data.to_vec())
            .unwrap_or_default();

        // Cache eviction if needed (simple FIFO)
        if cache.len() >= BLOB_CACHE_LIMIT {
            if let Some(k) = cache.keys().next().cloned() {
                cache.remove(&k);
            }
        }

        cache.insert(oid, bytes.clone());
        bytes
    }

    // Fast line counting using memchr (much faster than .lines().count())
    #[inline(always)]
    fn count_lines(bytes: &[u8]) -> i64 {
        memchr::memchr_iter(b'\n', bytes).count() as i64
    }

    // Process commits in parallel chunks for better CPU utilization
    // Grid search results (Rust Q4 2024, 7564 commits): 8=111s, 16=109s, 32=108s, 64=108s, 128=120s, 256=110s
    // Optimal: 64 (balanced parallelism vs overhead for 32-thread server)
    const CHUNK_SIZE: usize = 64;
    commits.par_chunks_mut(CHUNK_SIZE).for_each(|chunk| {
        // Each worker opens its own Repository handle for thread safety
        let repo = match gix::open(repo_path) {
            Ok(r) => r,
            Err(_) => return,
        };

        for commit_info in chunk.iter_mut() {
            let hash = commit_info.hash.clone();
            // parse oid
            let oid = match gix::ObjectId::from_hex(hash.as_bytes()) {
                Ok(o) => o,
                Err(_) => continue,
            };
            let Ok(commit) = repo.find_commit(oid) else {
                continue;
            };
            let mut parents = commit.parent_ids();
            let Some(first_parent_id) = parents.next().map(|p| p.detach()) else {
                continue;
            };
            // load trees
            let Ok(tree_id) = commit.tree_id() else {
                continue;
            };
            let Ok(parent_commit) = repo.find_commit(first_parent_id) else {
                continue;
            };
            let Ok(parent_tree_id) = parent_commit.tree_id() else {
                continue;
            };

            // If the trees are identical, there are no file changes. Skip heavy work.
            if tree_id == parent_tree_id {
                commit_info.lines_added = 0;
                commit_info.lines_modified = 0;
                commit_info.lines_deleted = 0;
                if include_patches {
                    commit_info.files_changed = 0;
                    commit_info.file_changes_json = "[]".to_string();
                }
                continue;
            }

            let tree_oid: gix::ObjectId = tree_id.into();
            let parent_tree_oid: gix::ObjectId = parent_tree_id.into();

            // Per-commit cache for blobs
            let mut blob_cache: FxHashMap<gix::ObjectId, Vec<u8>> =
                FxHashMap::with_capacity_and_hasher(128, Default::default());

            // Enumerate changes using gix tree-diff
            let (mods, adds, dels) = match tree_changes(&repo, parent_tree_oid, tree_oid) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let mut added_lines: i64 = 0;
            let mut deleted_lines: i64 = 0;
            let mut modified_lines: i64 = 0;

            let mut file_changes: Vec<serde_json::Value> = Vec::new();

            // Prepare reverse maps for rename/copy detection
            use std::collections::HashSet;
            let mut prev_oid_to_paths: FxHashMap<gix::ObjectId, Vec<String>> = FxHashMap::default();
            let mut prev_only: HashSet<String> = HashSet::new();
            for (p, oid) in &dels {
                prev_only.insert(p.clone());
                prev_oid_to_paths.entry(*oid).or_default().push(p.clone());
            }

            // Preload bytes for modifications, then parallelize diff computation
            let mut pre_mod: Vec<(String, Vec<u8>, Vec<u8>)> = Vec::with_capacity(mods.len());
            for (path, prev_id, curr_id) in &mods {
                if prev_id == curr_id {
                    continue;
                }
                let curr_bytes: Vec<u8> = get_blob_bytes(&repo, *curr_id, &mut blob_cache);
                let prev_bytes: Vec<u8> = get_blob_bytes(&repo, *prev_id, &mut blob_cache);
                pre_mod.push((path.clone(), curr_bytes, prev_bytes));
            }
            // When include_patches is false, avoid allocating JSON objects per file
            let mod_results: Vec<(i64, i64, i64, Option<serde_json::Value>)> = pre_mod
                .par_iter()
                .map(|(path, curr_bytes, prev_bytes)| {
                    if curr_bytes.len() > max_bytes_cfg || prev_bytes.len() > max_bytes_cfg {
                        if include_patches {
                            let mut obj = serde_json::Map::new();
                            obj.insert(
                                "new_path".to_string(),
                                serde_json::Value::String(path.clone()),
                            );
                            obj.insert("old_path".to_string(), serde_json::Value::Null);
                            obj.insert(
                                "kind".to_string(),
                                serde_json::Value::String("modified".to_string()),
                            );
                            obj.insert(
                                "additions".to_string(),
                                serde_json::Value::Number(0.into()),
                            );
                            obj.insert(
                                "deletions".to_string(),
                                serde_json::Value::Number(0.into()),
                            );
                            obj.insert("hunks".to_string(), serde_json::Value::Number(0.into()));
                            obj.insert("methods".to_string(), serde_json::Value::Number(0.into()));
                            obj.insert("is_binary".to_string(), serde_json::Value::Bool(true));
                            return (0, 0, 0, Some(serde_json::Value::Object(obj)));
                        } else {
                            return (0, 0, 0, None);
                        }
                    }
                    let text_both = is_text(curr_bytes) && is_text(prev_bytes);
                    let (ins, del, hunks) = if text_both {
                        let curr_str = std::str::from_utf8(curr_bytes).unwrap_or("");
                        let prev_str = std::str::from_utf8(prev_bytes).unwrap_or("");
                        let diff = TextDiff::from_lines(prev_str, curr_str);
                        let mut ins: i64 = 0;
                        let mut del: i64 = 0;
                        let mut hunks: i64 = 0;
                        let mut in_change = false;
                        for change in diff.iter_all_changes() {
                            match change.tag() {
                                similar::ChangeTag::Insert => {
                                    ins += 1;
                                    if !in_change {
                                        hunks += 1;
                                        in_change = true;
                                    }
                                }
                                similar::ChangeTag::Delete => {
                                    del += 1;
                                    if !in_change {
                                        hunks += 1;
                                        in_change = true;
                                    }
                                }
                                similar::ChangeTag::Equal => {
                                    in_change = false;
                                }
                            }
                        }
                        (ins, del, hunks)
                    } else {
                        (0, 0, 0)
                    };
                    let mod_here = ins.min(del);
                    let add_eff = ins - mod_here;
                    let del_eff = del - mod_here;
                    if include_patches {
                        let mut obj = serde_json::Map::new();
                        obj.insert(
                            "new_path".to_string(),
                            serde_json::Value::String(path.clone()),
                        );
                        obj.insert("old_path".to_string(), serde_json::Value::Null);
                        obj.insert(
                            "kind".to_string(),
                            serde_json::Value::String("modified".to_string()),
                        );
                        obj.insert(
                            "additions".to_string(),
                            serde_json::Value::Number(add_eff.into()),
                        );
                        obj.insert(
                            "deletions".to_string(),
                            serde_json::Value::Number(del_eff.into()),
                        );
                        obj.insert("hunks".to_string(), serde_json::Value::Number(hunks.into()));
                        obj.insert("methods".to_string(), serde_json::Value::Number(0.into()));
                        obj.insert("is_binary".to_string(), serde_json::Value::Bool(!text_both));
                        (
                            add_eff,
                            del_eff,
                            mod_here,
                            Some(serde_json::Value::Object(obj)),
                        )
                    } else {
                        (add_eff, del_eff, mod_here, None)
                    }
                })
                .collect();
            for (add_eff, del_eff, mod_here, obj) in mod_results {
                modified_lines += mod_here;
                added_lines += add_eff;
                deleted_lines += del_eff;
                if let Some(obj) = obj {
                    file_changes.push(obj);
                }
            }

            // 2) Detect renames and copies among additions by matching blob IDs present in deletions
            let mut consumed_prev: HashSet<String> = HashSet::new();
            let mut consumed_curr: HashSet<String> = HashSet::new();
            if detect_renames || detect_copies {
                for (path, curr_id) in &adds {
                    if let Some(prev_paths_for_id) = prev_oid_to_paths.get(curr_id) {
                        // Prefer a prev path that no longer exists (rename), otherwise treat as copy
                        let mut chosen_old: Option<String> = None;
                        for p_old in prev_paths_for_id {
                            if prev_only.contains(p_old) && !consumed_prev.contains(p_old) {
                                chosen_old = Some(p_old.clone());
                                break;
                            }
                        }
                        if chosen_old.is_none() && detect_copies {
                            // copy: pick a stable old path (lexicographically first) that wasn't used
                            for p_old in prev_paths_for_id {
                                if !consumed_prev.contains(p_old) {
                                    chosen_old = Some(p_old.clone());
                                    break;
                                }
                            }
                        }
                        if let Some(old_path) = chosen_old {
                            // class: renamed if old removed, else copied
                            let is_renamed = prev_only.contains(&old_path);
                            consumed_prev.insert(old_path.clone());
                            consumed_curr.insert(path.clone());

                            // compute line metrics between identical content -> zero changes
                            let curr_bytes: Vec<u8> =
                                get_blob_bytes(&repo, *curr_id, &mut blob_cache);
                            let text_curr = is_text(&curr_bytes);
                            if include_patches {
                                let mut obj = serde_json::Map::new();
                                obj.insert(
                                    "new_path".to_string(),
                                    serde_json::Value::String(path.clone()),
                                );
                                obj.insert(
                                    "old_path".to_string(),
                                    serde_json::Value::String(old_path),
                                );
                                obj.insert(
                                    "kind".to_string(),
                                    serde_json::Value::String(if is_renamed {
                                        "renamed".to_string()
                                    } else {
                                        "copied".to_string()
                                    }),
                                );
                                obj.insert(
                                    "additions".to_string(),
                                    serde_json::Value::Number(0.into()),
                                );
                                obj.insert(
                                    "deletions".to_string(),
                                    serde_json::Value::Number(0.into()),
                                );
                                obj.insert(
                                    "hunks".to_string(),
                                    serde_json::Value::Number(0.into()),
                                );
                                obj.insert(
                                    "methods".to_string(),
                                    serde_json::Value::Number(0.into()),
                                );
                                obj.insert(
                                    "is_binary".to_string(),
                                    serde_json::Value::Bool(!text_curr),
                                );
                                file_changes.push(serde_json::Value::Object(obj));
                            }
                        }
                    }
                }
            }
            // 3) Added files (excluding those consumed by rename/copy)
            let mut pre_add: Vec<(String, Vec<u8>)> = Vec::with_capacity(adds.len());
            for (path, curr_id) in &adds {
                if consumed_curr.contains(path) {
                    continue;
                }
                let curr_bytes: Vec<u8> = get_blob_bytes(&repo, *curr_id, &mut blob_cache);
                pre_add.push((path.clone(), curr_bytes));
            }
            let add_results: Vec<(i64, Option<serde_json::Value>)> = pre_add
                .par_iter()
                .map(|(path, curr_bytes)| {
                    if curr_bytes.len() > max_bytes_cfg {
                        if include_patches {
                            let mut obj = serde_json::Map::new();
                            obj.insert(
                                "new_path".to_string(),
                                serde_json::Value::String(path.clone()),
                            );
                            obj.insert("old_path".to_string(), serde_json::Value::Null);
                            obj.insert(
                                "kind".to_string(),
                                serde_json::Value::String("added".to_string()),
                            );
                            obj.insert(
                                "additions".to_string(),
                                serde_json::Value::Number(0.into()),
                            );
                            obj.insert(
                                "deletions".to_string(),
                                serde_json::Value::Number(0.into()),
                            );
                            obj.insert("hunks".to_string(), serde_json::Value::Number(0.into()));
                            obj.insert("methods".to_string(), serde_json::Value::Number(0.into()));
                            obj.insert("is_binary".to_string(), serde_json::Value::Bool(true));
                            return (0, Some(serde_json::Value::Object(obj)));
                        } else {
                            return (0, None);
                        }
                    }
                    let text_curr = is_text(curr_bytes);
                    // Fast-path: for added files, additions = number of lines; hunks = 1 if non-zero
                    let (ins, hunks) = if text_curr {
                        // Fast line counting with memchr
                        let ins = count_lines(curr_bytes);
                        let hunks = if ins > 0 { 1 } else { 0 };
                        (ins, hunks)
                    } else {
                        (0, 0)
                    };
                    if include_patches {
                        let mut obj = serde_json::Map::new();
                        obj.insert(
                            "new_path".to_string(),
                            serde_json::Value::String(path.clone()),
                        );
                        obj.insert("old_path".to_string(), serde_json::Value::Null);
                        obj.insert(
                            "kind".to_string(),
                            serde_json::Value::String("added".to_string()),
                        );
                        obj.insert(
                            "additions".to_string(),
                            serde_json::Value::Number(ins.into()),
                        );
                        obj.insert("deletions".to_string(), serde_json::Value::Number(0.into()));
                        obj.insert("hunks".to_string(), serde_json::Value::Number(hunks.into()));
                        obj.insert("methods".to_string(), serde_json::Value::Number(0.into()));
                        obj.insert("is_binary".to_string(), serde_json::Value::Bool(!text_curr));
                        (ins, Some(serde_json::Value::Object(obj)))
                    } else {
                        (ins, None)
                    }
                })
                .collect();
            for (ins, obj) in add_results {
                added_lines += ins;
                if let Some(obj) = obj {
                    file_changes.push(obj);
                }
            }

            // 4) Deleted files (excluding those consumed by rename/copy)
            let mut pre_del: Vec<(String, Vec<u8>)> = Vec::with_capacity(dels.len());
            for (path, prev_id) in &dels {
                if consumed_prev.contains(path) {
                    continue;
                }
                let prev_bytes: Vec<u8> = get_blob_bytes(&repo, *prev_id, &mut blob_cache);
                pre_del.push((path.clone(), prev_bytes));
            }
            let del_results: Vec<(i64, Option<serde_json::Value>)> = pre_del
                .par_iter()
                .map(|(path, prev_bytes)| {
                    if prev_bytes.len() > max_bytes_cfg {
                        if include_patches {
                            let mut obj = serde_json::Map::new();
                            obj.insert(
                                "new_path".to_string(),
                                serde_json::Value::String(path.clone()),
                            );
                            obj.insert("old_path".to_string(), serde_json::Value::Null);
                            obj.insert(
                                "kind".to_string(),
                                serde_json::Value::String("deleted".to_string()),
                            );
                            obj.insert(
                                "additions".to_string(),
                                serde_json::Value::Number(0.into()),
                            );
                            obj.insert(
                                "deletions".to_string(),
                                serde_json::Value::Number(0.into()),
                            );
                            obj.insert("hunks".to_string(), serde_json::Value::Number(0.into()));
                            obj.insert("methods".to_string(), serde_json::Value::Number(0.into()));
                            obj.insert("is_binary".to_string(), serde_json::Value::Bool(true));
                            return (0, Some(serde_json::Value::Object(obj)));
                        } else {
                            return (0, None);
                        }
                    }
                    let text_prev = is_text(prev_bytes);
                    // Fast-path: for deleted files, deletions = number of lines; hunks = 1 if non-zero
                    let (del, hunks) = if text_prev {
                        // Fast line counting with memchr
                        let del = count_lines(prev_bytes);
                        let hunks = if del > 0 { 1 } else { 0 };
                        (del, hunks)
                    } else {
                        (0, 0)
                    };
                    if include_patches {
                        let mut obj = serde_json::Map::new();
                        obj.insert(
                            "new_path".to_string(),
                            serde_json::Value::String(path.clone()),
                        );
                        obj.insert("old_path".to_string(), serde_json::Value::Null);
                        obj.insert(
                            "kind".to_string(),
                            serde_json::Value::String("deleted".to_string()),
                        );
                        obj.insert("additions".to_string(), serde_json::Value::Number(0.into()));
                        obj.insert(
                            "deletions".to_string(),
                            serde_json::Value::Number(del.into()),
                        );
                        obj.insert("hunks".to_string(), serde_json::Value::Number(hunks.into()));
                        obj.insert("methods".to_string(), serde_json::Value::Number(0.into()));
                        obj.insert("is_binary".to_string(), serde_json::Value::Bool(!text_prev));
                        (del, Some(serde_json::Value::Object(obj)))
                    } else {
                        (del, None)
                    }
                })
                .collect();
            for (del, obj) in del_results {
                deleted_lines += del;
                if let Some(obj) = obj {
                    file_changes.push(obj);
                }
            }

            commit_info.lines_added = added_lines;
            commit_info.lines_modified = modified_lines;
            commit_info.lines_deleted = deleted_lines;
            if include_patches {
                commit_info.files_changed = file_changes.len() as i64;
                commit_info.file_changes_json =
                    serde_json::to_string(&file_changes).unwrap_or_else(|_| "[]".to_string());
            } else {
                commit_info.files_changed = 0;
                commit_info.file_changes_json = "[]".to_string();
            }
        }
    });
}

#[cfg(feature = "ext")]
#[duckdb_entrypoint_c_api(ext_name = "git", min_duckdb_version = "v0.0.1")]
pub fn extension_entrypoint(con: Connection) -> Result<(), Box<dyn std::error::Error>> {
    register_git(&con)
        .and_then(|_| register_git_branches(&con))
        .and_then(|_| register_git_feature_metrics(&con))
        .and_then(|_| register_git_tags(&con))
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })
}
