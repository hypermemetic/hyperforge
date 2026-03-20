//! Workspace-level commit activity over time with composable file-level filters.

use async_stream::stream;
use futures::Stream;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::commands::runner::discover_or_bail;
use crate::hub::{ActivityDay, ExtensionStats, HyperforgeEvent};
use crate::hubs::utils::{glob_match, RepoFilter};

// ── Raw records from git log ────────────────────────────────────────────────

struct CommitRecord {
    date: String,
    author: String,
}

struct FileRecord {
    date: String,
    file: String,
    insertions: usize,
    deletions: usize,
}

struct ParsedLog {
    commits: Vec<CommitRecord>,
    files: Vec<FileRecord>,
}

// ── Aggregated result ───────────────────────────────────────────────────────

struct RepoActivityResult {
    commits: usize,
    insertions: usize,
    deletions: usize,
    files_changed: usize,
    authors: Vec<String>,
    daily: BTreeMap<String, DayBucket>,
    by_extension: Vec<ExtensionStats>,
}

#[derive(Default)]
struct DayBucket {
    commits: usize,
    insertions: usize,
    deletions: usize,
}

// ── Filter config ───────────────────────────────────────────────────────────

struct ActivityFilter {
    exclude_ext: Vec<String>,
    exclude_path: Vec<String>,
    min_file_net: usize,
}

impl ActivityFilter {
    fn new(
        exclude_ext: Option<Vec<String>>,
        exclude_path: Option<Vec<String>>,
        min_file_net: Option<usize>,
    ) -> Self {
        // Flatten comma-separated values so `--exclude_ext json,lock` works
        let exclude_ext = exclude_ext
            .unwrap_or_default()
            .iter()
            .flat_map(|s| s.split(',').map(|p| p.trim().trim_start_matches('.').to_lowercase()))
            .filter(|s| !s.is_empty())
            .collect();
        let exclude_path = exclude_path
            .unwrap_or_default()
            .iter()
            .flat_map(|s| s.split(',').map(|p| p.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect();
        Self {
            exclude_ext,
            exclude_path,
            min_file_net: min_file_net.unwrap_or(0),
        }
    }

}

// ── Phase 1: Parse ──────────────────────────────────────────────────────────

/// Parse `git log --format="COMMIT %aI %aN" --numstat` output into raw records.
fn parse_git_log(output: &str) -> ParsedLog {
    let mut commits = Vec::new();
    let mut files = Vec::new();
    let mut current_date: Option<String> = None;

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix("COMMIT ") {
            let mut parts = rest.splitn(2, ' ');
            let iso_date = parts.next().unwrap_or("");
            let author = parts.next().unwrap_or("").to_string();
            let date = iso_date.get(..10).unwrap_or(iso_date).to_string();

            commits.push(CommitRecord {
                date: date.clone(),
                author,
            });
            current_date = Some(date);
        } else if let Some(date) = &current_date {
            // numstat line: "insertions\tdeletions\tfilename"
            let mut parts = line.split('\t');
            let ins_str = parts.next().unwrap_or("-");
            let del_str = parts.next().unwrap_or("-");
            let file = parts.next().unwrap_or("");

            // Binary files show "-" for both
            if ins_str == "-" || del_str == "-" {
                if !file.is_empty() {
                    files.push(FileRecord {
                        date: date.clone(),
                        file: file.to_string(),
                        insertions: 0,
                        deletions: 0,
                    });
                }
                continue;
            }

            let ins: usize = ins_str.parse().unwrap_or(0);
            let del: usize = del_str.parse().unwrap_or(0);

            if !file.is_empty() {
                files.push(FileRecord {
                    date: date.clone(),
                    file: file.to_string(),
                    insertions: ins,
                    deletions: del,
                });
            }
        }
    }

    ParsedLog { commits, files }
}

// ── Phase 2: Filter + Aggregate ─────────────────────────────────────────────

fn file_extension(path: &str) -> String {
    Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn apply_filters_and_aggregate(parsed: ParsedLog, filter: &ActivityFilter) -> RepoActivityResult {
    let mut filtered_files = parsed.files;

    // 1. exclude_ext
    if !filter.exclude_ext.is_empty() {
        filtered_files.retain(|f| {
            let ext = file_extension(&f.file);
            !filter.exclude_ext.contains(&ext)
        });
    }

    // 2. exclude_path — glob against file paths
    if !filter.exclude_path.is_empty() {
        filtered_files.retain(|f| {
            !filter.exclude_path.iter().any(|pat| glob_match(pat, &f.file))
        });
    }

    // 3. min_file_net — group by file, compute |total_ins - total_del|, drop if < threshold
    if filter.min_file_net > 0 {
        let mut file_totals: HashMap<String, (usize, usize)> = HashMap::new();
        for f in &filtered_files {
            let entry = file_totals.entry(f.file.clone()).or_default();
            entry.0 += f.insertions;
            entry.1 += f.deletions;
        }
        let excluded_files: HashSet<String> = file_totals
            .into_iter()
            .filter(|(_, (ins, del))| {
                let net = if *ins >= *del { ins - del } else { del - ins };
                net < filter.min_file_net
            })
            .map(|(file, _)| file)
            .collect();

        if !excluded_files.is_empty() {
            filtered_files.retain(|f| !excluded_files.contains(&f.file));
        }
    }

    // Aggregate commits
    let mut authors: HashSet<String> = HashSet::new();
    let mut daily: BTreeMap<String, DayBucket> = BTreeMap::new();

    for c in &parsed.commits {
        authors.insert(c.author.clone());
        daily.entry(c.date.clone()).or_default().commits += 1;
    }

    // Aggregate file stats
    let mut total_insertions: usize = 0;
    let mut total_deletions: usize = 0;
    let mut unique_files: HashSet<String> = HashSet::new();
    let mut ext_map: HashMap<String, (usize, usize, HashSet<String>)> = HashMap::new();

    for f in &filtered_files {
        total_insertions += f.insertions;
        total_deletions += f.deletions;
        unique_files.insert(f.file.clone());

        let bucket = daily.entry(f.date.clone()).or_default();
        bucket.insertions += f.insertions;
        bucket.deletions += f.deletions;

        let ext = file_extension(&f.file);
        let entry = ext_map.entry(ext).or_insert_with(|| (0, 0, HashSet::new()));
        entry.0 += f.insertions;
        entry.1 += f.deletions;
        entry.2.insert(f.file.clone());
    }

    // Build by_extension, sorted by total changes descending
    let mut by_extension: Vec<ExtensionStats> = ext_map
        .into_iter()
        .map(|(ext, (ins, del, files))| ExtensionStats {
            extension: if ext.is_empty() {
                "(none)".to_string()
            } else {
                ext
            },
            insertions: ins,
            deletions: del,
            files: files.len(),
        })
        .collect();
    by_extension.sort_by(|a, b| {
        let a_total = a.insertions + a.deletions;
        let b_total = b.insertions + b.deletions;
        b_total.cmp(&a_total)
    });

    let mut author_list: Vec<String> = authors.into_iter().collect();
    author_list.sort();

    RepoActivityResult {
        commits: parsed.commits.len(),
        insertions: total_insertions,
        deletions: total_deletions,
        files_changed: unique_files.len(),
        authors: author_list,
        daily,
        by_extension,
    }
}

// ── Git integration ─────────────────────────────────────────────────────────

fn measure_activity(
    repo_path: &Path,
    since: &str,
    until: &str,
    filter: &ActivityFilter,
) -> Result<RepoActivityResult, String> {
    let output = std::process::Command::new("git")
        .args([
            "log",
            &format!("--since={}", since),
            &format!("--until={}", until),
            "--format=COMMIT %aI %aN",
            "--numstat",
        ])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("git log failed: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed = parse_git_log(&stdout);
    Ok(apply_filters_and_aggregate(parsed, filter))
}

// ── Stream entry point ──────────────────────────────────────────────────────

pub fn activity(
    path: String,
    since: Option<String>,
    until: Option<String>,
    include: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
    exclude_ext: Option<Vec<String>>,
    exclude_path: Option<Vec<String>>,
    min_file_net: Option<usize>,
) -> impl Stream<Item = HyperforgeEvent> + Send + 'static {
    let repo_filter = RepoFilter::new(include, exclude);
    let since = since.unwrap_or_else(|| "7 days ago".to_string());
    let until = until.unwrap_or_else(|| "now".to_string());
    let activity_filter = ActivityFilter::new(exclude_ext, exclude_path, min_file_net);

    stream! {
        let workspace_path = PathBuf::from(&path);

        let ctx = match discover_or_bail(&workspace_path) {
            Ok(ctx) => ctx,
            Err(event) => { yield event; return; }
        };

        let repos: Vec<_> = ctx.repos.iter()
            .filter(|r| repo_filter.matches(&r.dir_name))
            .collect();

        if repos.is_empty() {
            yield HyperforgeEvent::Info {
                message: "No repos matched filter.".to_string(),
            };
            return;
        }

        // Serialize filter config so it can be shared across spawn_blocking tasks
        let filter_exclude_ext = activity_filter.exclude_ext.clone();
        let filter_exclude_path = activity_filter.exclude_path.clone();
        let filter_min_file_net = activity_filter.min_file_net;

        let inputs: Vec<(String, PathBuf, String, String, Vec<String>, Vec<String>, usize)> =
            repos.iter()
                .map(|r| (
                    r.dir_name.clone(),
                    r.path.clone(),
                    since.clone(),
                    until.clone(),
                    filter_exclude_ext.clone(),
                    filter_exclude_path.clone(),
                    filter_min_file_net,
                ))
                .collect();

        let results = crate::commands::runner::run_batch_blocking(
            inputs,
            8,
            |(repo_name, repo_path, since_val, until_val, exc_ext, exc_path, min_net)| {
                let filter = ActivityFilter {
                    exclude_ext: exc_ext,
                    exclude_path: exc_path,
                    min_file_net: min_net,
                };
                match measure_activity(&repo_path, &since_val, &until_val, &filter) {
                    Ok(result) => (repo_name, Some(result), None::<String>),
                    Err(e) => (repo_name, None, Some(e)),
                }
            },
        ).await;

        // Collect successful results for sorting and merging
        let mut entries: Vec<(String, RepoActivityResult)> = Vec::new();

        for result in results {
            match result {
                Ok((repo_name, Some(activity), _)) => {
                    if activity.commits == 0 {
                        continue;
                    }
                    entries.push((repo_name, activity));
                }
                Ok((repo_name, None, Some(e))) => {
                    yield HyperforgeEvent::Error {
                        message: format!("{}: {}", repo_name, e),
                    };
                }
                Ok((repo_name, None, None)) => {
                    yield HyperforgeEvent::Error {
                        message: format!("{}: unknown error", repo_name),
                    };
                }
                Err(e) => {
                    yield HyperforgeEvent::Error {
                        message: format!("Task error: {}", e),
                    };
                }
            }
        }

        // Sort by total changes descending
        entries.sort_by(|a, b| {
            let a_total = a.1.insertions + a.1.deletions;
            let b_total = b.1.insertions + b.1.deletions;
            b_total.cmp(&a_total)
        });

        // Merge workspace-level daily buckets, totals, and extension stats
        let mut ws_commits: usize = 0;
        let mut ws_insertions: usize = 0;
        let mut ws_deletions: usize = 0;
        let mut ws_authors: HashSet<String> = HashSet::new();
        let mut ws_daily: BTreeMap<String, DayBucket> = BTreeMap::new();
        let mut ws_ext_map: HashMap<String, (usize, usize, HashSet<String>)> = HashMap::new();

        for (repo_name, result) in &entries {
            ws_commits += result.commits;
            ws_insertions += result.insertions;
            ws_deletions += result.deletions;
            for author in &result.authors {
                ws_authors.insert(author.clone());
            }
            for (date, bucket) in &result.daily {
                let ws_bucket = ws_daily.entry(date.clone()).or_default();
                ws_bucket.commits += bucket.commits;
                ws_bucket.insertions += bucket.insertions;
                ws_bucket.deletions += bucket.deletions;
            }

            // Merge extension stats at workspace level
            for ext_stat in &result.by_extension {
                let entry = ws_ext_map
                    .entry(ext_stat.extension.clone())
                    .or_insert_with(|| (0, 0, HashSet::new()));
                entry.0 += ext_stat.insertions;
                entry.1 += ext_stat.deletions;
                // We can't recover per-file names here, so just accumulate file count
            }

            // Emit per-repo event
            let daily_vec: Vec<ActivityDay> = result.daily.iter()
                .map(|(date, b)| ActivityDay {
                    date: date.clone(),
                    commits: b.commits,
                    insertions: b.insertions,
                    deletions: b.deletions,
                })
                .collect();

            yield HyperforgeEvent::RepoActivity {
                repo_name: repo_name.clone(),
                commits: result.commits,
                insertions: result.insertions,
                deletions: result.deletions,
                files_changed: result.files_changed,
                authors: result.authors.clone(),
                daily: daily_vec,
                by_extension: result.by_extension.clone(),
            };
        }

        // Build workspace extension summary
        let mut ws_by_extension: Vec<ExtensionStats> = ws_ext_map
            .into_iter()
            .map(|(ext, (ins, del, _))| ExtensionStats {
                extension: ext,
                insertions: ins,
                deletions: del,
                files: 0, // workspace-level file count not meaningful (cross-repo dupes)
            })
            .collect();

        // Patch file counts from per-repo data (sum across repos)
        for ext_stat in &mut ws_by_extension {
            ext_stat.files = entries.iter()
                .flat_map(|(_, r)| r.by_extension.iter())
                .filter(|e| e.extension == ext_stat.extension)
                .map(|e| e.files)
                .sum();
        }

        ws_by_extension.sort_by(|a, b| {
            let a_total = a.insertions + a.deletions;
            let b_total = b.insertions + b.deletions;
            b_total.cmp(&a_total)
        });

        // Emit workspace summary
        let mut ws_author_list: Vec<String> = ws_authors.into_iter().collect();
        ws_author_list.sort();

        let ws_daily_vec: Vec<ActivityDay> = ws_daily.iter()
            .map(|(date, b)| ActivityDay {
                date: date.clone(),
                commits: b.commits,
                insertions: b.insertions,
                deletions: b.deletions,
            })
            .collect();

        yield HyperforgeEvent::WorkspaceActivity {
            since: since.clone(),
            until: until.clone(),
            total_repos: entries.len(),
            total_commits: ws_commits,
            total_insertions: ws_insertions,
            total_deletions: ws_deletions,
            total_authors: ws_author_list,
            daily: ws_daily_vec,
            by_extension: ws_by_extension,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_filter() -> ActivityFilter {
        ActivityFilter::new(None, None, None)
    }

    #[test]
    fn test_parse_git_log_basic() {
        let input = "\
COMMIT 2026-03-15T10:30:00-07:00 Alice
5\t2\tsrc/main.rs
3\t1\tsrc/lib.rs

COMMIT 2026-03-15T14:00:00-07:00 Bob
10\t0\tREADME.md

COMMIT 2026-03-14T09:00:00-07:00 Alice
1\t1\tsrc/main.rs
";

        let parsed = parse_git_log(input);
        let result = apply_filters_and_aggregate(parsed, &no_filter());
        assert_eq!(result.commits, 3);
        assert_eq!(result.insertions, 19);
        assert_eq!(result.deletions, 4);
        assert_eq!(result.files_changed, 3);
        assert_eq!(result.authors, vec!["Alice", "Bob"]);
        assert_eq!(result.daily.len(), 2);

        let day15 = &result.daily["2026-03-15"];
        assert_eq!(day15.commits, 2);
        assert_eq!(day15.insertions, 18);
        assert_eq!(day15.deletions, 3);

        let day14 = &result.daily["2026-03-14"];
        assert_eq!(day14.commits, 1);
        assert_eq!(day14.insertions, 1);
        assert_eq!(day14.deletions, 1);
    }

    #[test]
    fn test_parse_git_log_empty() {
        let parsed = parse_git_log("");
        let result = apply_filters_and_aggregate(parsed, &no_filter());
        assert_eq!(result.commits, 0);
        assert_eq!(result.insertions, 0);
        assert_eq!(result.deletions, 0);
        assert_eq!(result.files_changed, 0);
        assert!(result.authors.is_empty());
        assert!(result.daily.is_empty());
    }

    #[test]
    fn test_parse_git_log_binary_files() {
        let input = "\
COMMIT 2026-03-15T10:00:00Z Dev
-\t-\timage.png
3\t1\tsrc/lib.rs
";

        let parsed = parse_git_log(input);
        let result = apply_filters_and_aggregate(parsed, &no_filter());
        assert_eq!(result.commits, 1);
        assert_eq!(result.insertions, 3);
        assert_eq!(result.deletions, 1);
        assert_eq!(result.files_changed, 2); // binary file still counted
    }

    #[test]
    fn test_extension_breakdown() {
        let input = "\
COMMIT 2026-03-15T10:00:00Z Dev
5\t2\tsrc/main.rs
3\t1\tsrc/lib.rs
10\t0\tREADME.md
2\t1\tdata.json
";

        let parsed = parse_git_log(input);
        let result = apply_filters_and_aggregate(parsed, &no_filter());

        assert_eq!(result.by_extension.len(), 3);

        let rs = result.by_extension.iter().find(|e| e.extension == "rs").unwrap();
        assert_eq!(rs.insertions, 8);
        assert_eq!(rs.deletions, 3);
        assert_eq!(rs.files, 2);

        let md = result.by_extension.iter().find(|e| e.extension == "md").unwrap();
        assert_eq!(md.insertions, 10);
        assert_eq!(md.deletions, 0);
        assert_eq!(md.files, 1);

        let json = result.by_extension.iter().find(|e| e.extension == "json").unwrap();
        assert_eq!(json.insertions, 2);
        assert_eq!(json.deletions, 1);
        assert_eq!(json.files, 1);
    }

    #[test]
    fn test_exclude_ext_filter() {
        let input = "\
COMMIT 2026-03-15T10:00:00Z Dev
5\t2\tsrc/main.rs
10\t0\tpackage-lock.json
2\t1\tdata.json
";

        let parsed = parse_git_log(input);
        let filter = ActivityFilter::new(
            Some(vec!["json".to_string()]),
            None,
            None,
        );
        let result = apply_filters_and_aggregate(parsed, &filter);

        assert_eq!(result.insertions, 5);
        assert_eq!(result.deletions, 2);
        assert_eq!(result.files_changed, 1);
        assert!(result.by_extension.iter().all(|e| e.extension != "json"));
    }

    #[test]
    fn test_exclude_ext_comma_separated() {
        let input = "\
COMMIT 2026-03-15T10:00:00Z Dev
5\t2\tsrc/main.rs
10\t0\tpackage-lock.json
3\t0\tCargo.lock
";

        let parsed = parse_git_log(input);
        let filter = ActivityFilter::new(
            Some(vec!["json,lock".to_string()]),
            None,
            None,
        );
        let result = apply_filters_and_aggregate(parsed, &filter);

        assert_eq!(result.insertions, 5);
        assert_eq!(result.deletions, 2);
        assert_eq!(result.files_changed, 1);
    }

    #[test]
    fn test_exclude_path_filter() {
        let input = "\
COMMIT 2026-03-15T10:00:00Z Dev
5\t2\tsrc/main.rs
10\t0\tgenerated/api.rs
3\t1\tgenerated/types.rs
";

        let parsed = parse_git_log(input);
        let filter = ActivityFilter::new(
            None,
            Some(vec!["generated/*".to_string()]),
            None,
        );
        let result = apply_filters_and_aggregate(parsed, &filter);

        assert_eq!(result.insertions, 5);
        assert_eq!(result.deletions, 2);
        assert_eq!(result.files_changed, 1);
    }

    #[test]
    fn test_min_file_net_filter() {
        let input = "\
COMMIT 2026-03-15T10:00:00Z Dev
10\t0\tsrc/new_feature.rs
5\t5\tsrc/churn.rs
";

        let parsed = parse_git_log(input);
        let filter = ActivityFilter::new(None, None, Some(1));
        let result = apply_filters_and_aggregate(parsed, &filter);

        // churn.rs has net 0, so it's filtered out
        assert_eq!(result.files_changed, 1);
        assert_eq!(result.insertions, 10);
        assert_eq!(result.deletions, 0);
    }

    #[test]
    fn test_min_file_net_across_commits() {
        let input = "\
COMMIT 2026-03-15T10:00:00Z Dev
10\t0\tsrc/temp.rs
5\t0\tsrc/real.rs

COMMIT 2026-03-16T10:00:00Z Dev
0\t10\tsrc/temp.rs
3\t1\tsrc/real.rs
";

        let parsed = parse_git_log(input);
        let filter = ActivityFilter::new(None, None, Some(1));
        let result = apply_filters_and_aggregate(parsed, &filter);

        // temp.rs: 10 ins, 10 del → net 0 → filtered
        // real.rs: 8 ins, 1 del → net 7 → kept
        assert_eq!(result.files_changed, 1);
        assert_eq!(result.insertions, 8);
        assert_eq!(result.deletions, 1);
    }

    #[test]
    fn test_combined_filters() {
        let input = "\
COMMIT 2026-03-15T10:00:00Z Dev
5\t2\tsrc/main.rs
10\t0\tgenerated/api.rs
2\t1\tdata.json
3\t3\tsrc/churn.rs
";

        let parsed = parse_git_log(input);
        let filter = ActivityFilter::new(
            Some(vec!["json".to_string()]),
            Some(vec!["generated/*".to_string()]),
            Some(1),
        );
        let result = apply_filters_and_aggregate(parsed, &filter);

        // json excluded, generated/* excluded, churn.rs net=0 excluded
        assert_eq!(result.files_changed, 1);
        assert_eq!(result.insertions, 5);
        assert_eq!(result.deletions, 2);
    }
}
