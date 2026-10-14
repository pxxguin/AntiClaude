use anyhow::{bail, Context, Result};
use regex::Regex;
use serde_json::Value;
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::LazyLock,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tempfile::TempDir;

const REFRESH: &str = "chore: refresh GitHub contributors";
static TARGET_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\w.-]+/[\w.-]+$").unwrap());
static IDENTITY_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?i)claude|anthropic").unwrap());

struct Target {
    url: String,
    slug: String,
}

struct RefUpdate {
    remote_ref: String,
    local_ref: String,
    expected_sha: String,
}

pub(crate) fn clean(options: &crate::cli::CleanOptions) -> Result<()> {
    let target = normalize(&options.target)?;
    require("git", &["--version"])?;
    let gh = available("gh") && cmd("gh", &["auth", "status"], None).is_ok();
    let temp = TempDir::new()?;
    let repo = temp.path().join(target.slug.rsplit('/').next().unwrap());
    let rp = repo.to_string_lossy();
    println!("Cloning {}...", target.slug);
    if gh {
        cmd(
            "gh",
            &[
                "repo",
                "clone",
                &target.slug,
                &rp,
                "--",
                "--no-single-branch",
            ],
            None,
        )?;
    } else {
        cmd(
            "git",
            &["clone", "--no-single-branch", &target.url, &rp],
            None,
        )?;
    }
    materialize(&repo)?;
    let hit_count = hits(&repo, "--all")?;
    let branches = dirty_refs(&repo, "refs/heads")?;
    let tags = dirty_refs(&repo, "refs/tags")?;
    println!(
        "\nRepository: {}\n  traced commits: {}\n  affected branches: {}\n  affected tags: {}",
        target.slug,
        hit_count,
        show(&branches),
        show(&tags)
    );
    print_trace_preview(&repo, hit_count)?;
    let identities = identity_hits(&repo)?;
    if identities > 0 {
        println!("  warning: {identities} Claude author/committer identities are not rewritten");
    }
    if options.dry_run {
        println!("\nDry run complete; nothing changed.");
        return Ok(());
    }
    if !options.yes {
        confirm(hit_count > 0)?;
    }
    if hit_count > 0 {
        rewrite_and_push(&repo, &target.slug, &branches, &tags, !options.no_backup)?;
    }
    println!("\nDone. Repository history is clean.");
    Ok(())
}

pub(crate) fn refresh(options: &crate::cli::RefreshOptions) -> Result<()> {
    let target = normalize(&options.target)?;
    require("git", &["--version"])?;
    require("gh", &["--version"])?;
    cmd("gh", &["auth", "status"], None).context("authenticate first with `gh auth login`")?;

    eprintln!("WARNING: refresh is experimental and will temporarily rename the default branch");
    eprintln!(
        "and push one empty commit. GitHub does not guarantee that this refreshes its cache."
    );
    if !options.yes {
        confirm_refresh()?;
    }
    if server_hits(&target.slug)? != 0 {
        bail!(
            "repository history still contains Claude traces; run `anticlaude clean {}` first",
            target.slug
        )
    }

    let temp = TempDir::new()?;
    let repo = temp.path().join(target.slug.rsplit('/').next().unwrap());
    let repo_path = repo.to_string_lossy();
    cmd(
        "gh",
        &[
            "repo",
            "clone",
            &target.slug,
            &repo_path,
            "--",
            "--no-single-branch",
        ],
        None,
    )?;
    refresh_graph(&repo, &target.slug)?;
    println!("\nRefresh completed. GitHub may still take time to update contributor statistics.");
    Ok(())
}

fn rewrite_and_push(
    repo: &Path,
    slug: &str,
    branches: &[String],
    tags: &[String],
    backup_enabled: bool,
) -> Result<()> {
    if backup_enabled {
        println!("Backup: {}", backup(repo, slug)?.display());
    } else {
        println!("Warning: backup disabled.");
    }
    let origin = git(repo, &["remote", "get-url", "origin"])?;
    let updates = planned_updates(repo, branches, tags)?;
    let report = crate::rewrite::rewrite_repository(repo)?;
    println!("Rewrote {} commit message(s).", report.commits_rewritten);
    if report.signed_commits_stripped + report.signed_tags_stripped > 0 {
        println!(
            "Warning: removed invalidated signatures from {} commit(s) and {} tag(s).",
            report.signed_commits_stripped, report.signed_tags_stripped
        );
    }
    if !crate::rewrite::traced_commits(repo, &["--branches", "--tags"])?.is_empty() {
        bail!("traces remain after rewriting; refusing to push")
    }
    let _ = cmd("git", &["remote", "remove", "origin"], Some(repo));
    cmd(
        "git",
        &["remote", "add", "origin", origin.trim()],
        Some(repo),
    )?;
    atomic_push(repo, &updates).context(
        "atomic push rejected; no refs were updated. The remote may have changed, or a protection rule may block rewriting",
    )?;
    Ok(())
}

fn planned_updates(repo: &Path, branches: &[String], tags: &[String]) -> Result<Vec<RefUpdate>> {
    branches
        .iter()
        .map(|name| (format!("refs/heads/{name}"), name.clone()))
        .chain(tags.iter().map(|name| {
            let reference = format!("refs/tags/{name}");
            (reference.clone(), reference)
        }))
        .map(|(remote_ref, local_ref)| {
            let expected_sha = remote_ref_sha(repo, &remote_ref)?;
            Ok(RefUpdate {
                remote_ref,
                local_ref,
                expected_sha,
            })
        })
        .collect()
}

fn remote_ref_sha(repo: &Path, reference: &str) -> Result<String> {
    let output = git(repo, &["ls-remote", "origin", reference])?;
    output
        .split_whitespace()
        .next()
        .map(str::to_owned)
        .with_context(|| format!("remote ref `{reference}` disappeared before rewriting"))
}

fn atomic_push(repo: &Path, updates: &[RefUpdate]) -> Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let mut owned = vec![
        "push".to_owned(),
        "--atomic".to_owned(),
        "origin".to_owned(),
    ];
    for update in updates {
        owned.push(format!(
            "--force-with-lease={}:{}",
            update.remote_ref, update.expected_sha
        ));
    }
    for update in updates {
        owned.push(format!("{}:{}", update.local_ref, update.remote_ref));
    }
    let args: Vec<_> = owned.iter().map(String::as_str).collect();
    let mut dry_run = args.clone();
    dry_run.insert(1, "--dry-run");
    cmd("git", &dry_run, Some(repo))?;
    println!("Atomically updating {} affected ref(s)...", updates.len());
    cmd("git", &args, Some(repo))?;
    Ok(())
}

fn normalize(s: &str) -> Result<Target> {
    let v = s.trim().trim_end_matches('/').trim_end_matches(".git");
    let slug = v
        .strip_prefix("https://github.com/")
        .or_else(|| v.strip_prefix("http://github.com/"))
        .or_else(|| v.strip_prefix("git@github.com:"))
        .unwrap_or(v);
    if !TARGET_PATTERN.is_match(slug) {
        bail!("only GitHub OWNER/REPO slugs and github.com URLs are supported")
    }
    Ok(Target {
        url: format!("https://github.com/{slug}.git"),
        slug: slug.into(),
    })
}
fn cmd(program: &str, args: &[&str], cwd: Option<&Path>) -> Result<Output> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(d) = cwd {
        command.current_dir(d);
    }
    let output = command
        .output()
        .with_context(|| format!("run `{program}`"))?;
    if !output.status.success() {
        let details = String::from_utf8_lossy(if output.stderr.is_empty() {
            &output.stdout
        } else {
            &output.stderr
        });
        bail!(
            "`{} {}` failed: {}",
            program,
            args.join(" "),
            details.trim()
        )
    }
    Ok(output)
}
fn available(p: &str) -> bool {
    Command::new(p).arg("--version").output().is_ok()
}
fn require(p: &str, a: &[&str]) -> Result<()> {
    cmd(p, a, None)
        .map(|_| ())
        .with_context(|| format!("required command `{p}` is unavailable"))
}
fn git(r: &Path, a: &[&str]) -> Result<String> {
    Ok(String::from_utf8_lossy(&cmd("git", a, Some(r))?.stdout)
        .trim()
        .into())
}
fn gh(a: &[&str]) -> Result<String> {
    Ok(String::from_utf8_lossy(&cmd("gh", a, None)?.stdout)
        .trim()
        .into())
}
fn refs(r: &Path, ns: &str) -> Result<Vec<String>> {
    Ok(git(r, &["for-each-ref", "--format=%(refname:short)", ns])?
        .lines()
        .map(str::to_owned)
        .collect())
}
fn remote_branches(r: &Path) -> Result<Vec<String>> {
    Ok(git(r, &["ls-remote", "--heads", "origin"])?
        .lines()
        .filter_map(|l| l.split("refs/heads/").nth(1))
        .map(str::to_owned)
        .collect())
}
fn materialize(r: &Path) -> Result<()> {
    let cur = git(r, &["branch", "--show-current"])?;
    for b in remote_branches(r)? {
        if b != cur {
            cmd(
                "git",
                &["branch", "--force", &b, &format!("origin/{b}")],
                Some(r),
            )?;
        }
    }
    Ok(())
}
fn hits(r: &Path, reference: &str) -> Result<usize> {
    Ok(crate::rewrite::traced_commits(r, &[reference])?.len())
}
fn print_trace_preview(repo: &Path, total: usize) -> Result<()> {
    if total == 0 {
        return Ok(());
    }
    let traced = crate::rewrite::traced_commits(repo, &["--all"])?;
    println!("  commit message preview:");
    for commit in traced.iter().take(20) {
        let summary = git(repo, &["show", "-s", "--format=%h%x09%s", commit])?;
        println!("    {summary}");
    }
    if total > 20 {
        println!("    ... and {} more", total - 20);
    }
    Ok(())
}
fn dirty_refs(r: &Path, ns: &str) -> Result<Vec<String>> {
    Ok(refs(r, ns)?
        .into_iter()
        .filter(|x| hits(r, x).unwrap_or(0) > 0)
        .collect())
}
fn identity_hits(r: &Path) -> Result<usize> {
    Ok(git(r, &["log", "--all", "--format=%an <%ae>|%cn <%ce>"])?
        .lines()
        .filter(|identity| IDENTITY_PATTERN.is_match(identity))
        .count())
}
fn backup(repo: &Path, slug: &str) -> Result<PathBuf> {
    let backup_dir = dirs::home_dir()
        .context("find home directory")?
        .join(".anticlaude-backups");
    fs::create_dir_all(&backup_dir)?;
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let path = backup_dir.join(format!(
        "{}-{timestamp}.bundle",
        slug.rsplit('/').next().unwrap()
    ));
    let path_string = path.to_string_lossy();
    cmd(
        "git",
        &["bundle", "create", &path_string, "--all"],
        Some(repo),
    )?;
    Ok(path)
}
fn confirm(rewrite: bool) -> Result<()> {
    print!(
        "This will {}update affected remote refs atomically. Type `yes`: ",
        if rewrite { "rewrite history and " } else { "" }
    );
    io::stdout().flush()?;
    let mut s = String::new();
    io::stdin().read_line(&mut s)?;
    if !matches!(s.trim().to_lowercase().as_str(), "y" | "yes") {
        bail!("aborted")
    }
    Ok(())
}

fn confirm_refresh() -> Result<()> {
    print!("Type `refresh` to accept the experimental branch rename and empty commit: ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if answer.trim() != "refresh" {
        bail!("aborted")
    }
    Ok(())
}
fn show(v: &[String]) -> String {
    if v.is_empty() {
        "-".into()
    } else {
        v.join(", ")
    }
}

fn server_hits(slug: &str) -> Result<usize> {
    let mut rs: Vec<_> = gh(&[
        "api",
        &format!("repos/{slug}/branches"),
        "--paginate",
        "--jq",
        ".[].name",
    ])?
    .lines()
    .map(str::to_owned)
    .collect();
    if let Ok(t) = gh(&[
        "api",
        &format!("repos/{slug}/tags"),
        "--paginate",
        "--jq",
        ".[].name",
    ]) {
        rs.extend(t.lines().map(str::to_owned));
    }
    let q="[.[]|select(.commit.message|test(\"(?i)(co-authored-by:.*(claude|anthropic))|(generated with claude)|(noreply@anthropic)\"))]|length";
    let mut n = 0;
    for r in rs {
        n += gh(&[
            "api",
            &format!("repos/{slug}/commits?sha={r}"),
            "--paginate",
            "--jq",
            q,
        ])?
        .lines()
        .filter_map(|x| x.parse::<usize>().ok())
        .sum::<usize>();
    }
    Ok(n)
}
fn exists(slug: &str, b: &str) -> bool {
    cmd("gh", &["api", &format!("repos/{slug}/branches/{b}")], None).is_ok()
}
fn rename(slug: &str, a: &str, b: &str) -> Result<()> {
    cmd(
        "gh",
        &[
            "api",
            "-X",
            "POST",
            &format!("repos/{slug}/branches/{a}/rename"),
            "-f",
            &format!("new_name={b}"),
        ],
        None,
    )
    .map(|_| ())
}
fn flush(slug: &str) -> Result<()> {
    let d = gh(&["api", &format!("repos/{slug}"), "--jq", ".default_branch"])?;
    let t = format!("{d}-anticlaude-tmp");
    if exists(slug, &t) {
        let _ = cmd(
            "gh",
            &[
                "api",
                "-X",
                "DELETE",
                &format!("repos/{slug}/git/refs/heads/{t}"),
            ],
            None,
        );
    }
    println!("Flushing contributor cache ({d} -> {t} -> {d})...");
    rename(slug, &d, &t)?;
    thread::sleep(Duration::from_secs(2));
    for i in 0..5 {
        if rename(slug, &t, &d).is_ok() {
            cmd(
                "gh",
                &[
                    "api",
                    "-X",
                    "PATCH",
                    &format!("repos/{slug}"),
                    "-f",
                    &format!("default_branch={d}"),
                ],
                None,
            )?;
            if exists(slug, &t) {
                let _ = cmd(
                    "gh",
                    &[
                        "api",
                        "-X",
                        "DELETE",
                        &format!("repos/{slug}/git/refs/heads/{t}"),
                    ],
                    None,
                );
            }
            return Ok(());
        }
        if i < 4 {
            thread::sleep(Duration::from_secs(2));
        }
    }
    bail!("could not restore branch `{d}`; rename `{t}` manually")
}
fn refresh_commit(repo: &Path, amend: bool) -> Result<()> {
    let branch = git(repo, &["branch", "--show-current"])?;
    let author_name = git(repo, &["log", "-1", "--format=%an"])?;
    let author_email = git(repo, &["log", "-1", "--format=%ae"])?;
    let name_config = format!("user.name={author_name}");
    let email_config = format!("user.email={author_email}");
    let mut commit_args = vec![
        "-c",
        "commit.gpgsign=false",
        "-c",
        &name_config,
        "-c",
        &email_config,
        "commit",
    ];
    if amend {
        commit_args.extend(["--amend", "--allow-empty", "-m", REFRESH]);
    } else {
        commit_args.extend(["--allow-empty", "-m", REFRESH]);
    }
    cmd("git", &commit_args, Some(repo))?;
    let mut push_args = vec!["push", "origin", &branch];
    if amend {
        push_args.push("--force");
    }
    cmd("git", &push_args, Some(repo))?;
    Ok(())
}
fn logins(slug: &str) -> Result<Option<Vec<String>>> {
    let o = cmd(
        "gh",
        &["api", &format!("repos/{slug}/stats/contributors")],
        None,
    )?;
    let s = String::from_utf8_lossy(&o.stdout);
    if s.trim().is_empty() || s.trim() == "[]" {
        return Ok(None);
    }
    let v: Value = serde_json::from_str(&s)?;
    Ok(Some(
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.pointer("/author/login")?.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
    ))
}
fn refresh_graph(r: &Path, slug: &str) -> Result<()> {
    for attempt in 0..3 {
        flush(slug)?;
        refresh_commit(r, attempt > 0)?;
        println!("Waiting for GitHub to rebuild the contributors graph...");
        for _ in 0..10 {
            if let Some(v) = logins(slug)? {
                if !v.iter().any(|login| IDENTITY_PATTERN.is_match(login)) {
                    return Ok(());
                }
                break;
            }
            thread::sleep(Duration::from_secs(6));
        }
    }
    bail!("history is clean, but GitHub's contributors graph is still stale")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slug() {
        assert_eq!(normalize("owner/repo").unwrap().slug, "owner/repo");
    }
    #[test]
    fn url() {
        assert_eq!(
            normalize("https://github.com/owner/repo.git").unwrap().slug,
            "owner/repo"
        );
    }
    #[test]
    fn rejects() {
        assert!(normalize("https://example.com/a/b").is_err());
    }

    #[test]
    fn rewrite_removes_trace_and_preserves_message() {
        let temp = TempDir::new().unwrap();
        cmd("git", &["init", "-b", "main"], Some(temp.path())).unwrap();
        fs::write(temp.path().join("file.txt"), "content").unwrap();
        cmd("git", &["add", "file.txt"], Some(temp.path())).unwrap();
        cmd(
            "git",
            &[
                "-c",
                "user.name=Tester",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "feat: 유니코드 메시지를 보존한다",
                "-m",
                "Co-Authored-By: Human <human@example.com>\nCo-Authored-By: Claude <noreply@anthropic.com>",
            ],
            Some(temp.path()),
        )
        .unwrap();
        crate::rewrite::rewrite_repository(temp.path()).unwrap();
        let message = git(temp.path(), &["log", "-1", "--format=%B"]).unwrap();
        assert!(message.contains("feat: 유니코드 메시지를 보존한다"));
        assert!(message.contains("Co-Authored-By: Human <human@example.com>"));
        assert!(!Regex::new("(?i)claude|anthropic")
            .unwrap()
            .is_match(&message));
    }

    #[test]
    fn refresh_commit_reuses_latest_author_and_pushes() {
        let temp = TempDir::new().unwrap();
        let remote = temp.path().join("remote.git");
        let work = temp.path().join("work");
        let remote_path = remote.to_string_lossy();
        let work_path = work.to_string_lossy();
        cmd("git", &["init", "--bare", "-b", "main", &remote_path], None).unwrap();
        cmd("git", &["clone", &remote_path, &work_path], None).unwrap();
        fs::write(work.join("file.txt"), "content").unwrap();
        cmd("git", &["add", "file.txt"], Some(&work)).unwrap();
        cmd(
            "git",
            &[
                "-c",
                "user.name=Original Author",
                "-c",
                "user.email=author@example.com",
                "commit",
                "-m",
                "initial",
            ],
            Some(&work),
        )
        .unwrap();
        cmd("git", &["push", "origin", "main"], Some(&work)).unwrap();

        refresh_commit(&work, false).unwrap();

        assert_eq!(git(&work, &["log", "-1", "--format=%s"]).unwrap(), REFRESH);
        assert_eq!(
            git(&work, &["log", "-1", "--format=%an <%ae>"]).unwrap(),
            "Original Author <author@example.com>"
        );
        assert_eq!(
            git(&work, &["rev-parse", "HEAD"]).unwrap(),
            git(&work, &["rev-parse", "origin/main"]).unwrap()
        );
    }

    #[test]
    fn atomic_push_rejects_concurrent_remote_change_without_partial_update() {
        let temp = TempDir::new().unwrap();
        let remote = temp.path().join("remote.git");
        let work = temp.path().join("work");
        let concurrent = temp.path().join("concurrent");
        let remote_path = remote.to_string_lossy();
        let work_path = work.to_string_lossy();
        let concurrent_path = concurrent.to_string_lossy();
        cmd("git", &["init", "--bare", "-b", "main", &remote_path], None).unwrap();
        cmd("git", &["clone", &remote_path, &work_path], None).unwrap();
        commit_file(&work, "base.txt", "base", "base");
        cmd("git", &["push", "origin", "main"], Some(&work)).unwrap();
        cmd("git", &["switch", "-c", "develop"], Some(&work)).unwrap();
        commit_file(&work, "develop.txt", "initial", "develop");
        cmd("git", &["push", "origin", "develop"], Some(&work)).unwrap();
        cmd("git", &["switch", "main"], Some(&work)).unwrap();

        let updates = planned_updates(&work, &["main".into(), "develop".into()], &[]).unwrap();
        let main_before = remote_sha(&remote, "refs/heads/main");
        commit_file(&work, "main.txt", "local", "local main");
        cmd("git", &["switch", "develop"], Some(&work)).unwrap();
        commit_file(&work, "develop.txt", "local", "local develop");

        cmd("git", &["clone", &remote_path, &concurrent_path], None).unwrap();
        cmd("git", &["switch", "develop"], Some(&concurrent)).unwrap();
        commit_file(&concurrent, "remote.txt", "remote", "concurrent change");
        cmd("git", &["push", "origin", "develop"], Some(&concurrent)).unwrap();

        assert!(atomic_push(&work, &updates).is_err());
        assert_eq!(remote_sha(&remote, "refs/heads/main"), main_before);
    }

    #[test]
    fn atomic_push_updates_annotated_tag_and_branch_together() {
        let temp = TempDir::new().unwrap();
        let remote = temp.path().join("remote.git");
        let work = temp.path().join("work");
        let remote_path = remote.to_string_lossy();
        let work_path = work.to_string_lossy();
        cmd("git", &["init", "--bare", "-b", "main", &remote_path], None).unwrap();
        cmd("git", &["clone", &remote_path, &work_path], None).unwrap();
        commit_file(
            &work,
            "release.txt",
            "release",
            "release\n\nCo-Authored-By: Claude <noreply@anthropic.com>",
        );
        cmd(
            "git",
            &[
                "-c",
                "user.name=Tester",
                "-c",
                "user.email=test@example.com",
                "tag",
                "-a",
                "v1.0.0",
                "-m",
                "release",
            ],
            Some(&work),
        )
        .unwrap();
        cmd(
            "git",
            &["push", "origin", "main", "refs/tags/v1.0.0"],
            Some(&work),
        )
        .unwrap();
        rewrite_and_push(
            &work,
            "owner/repo",
            &["main".into()],
            &["v1.0.0".into()],
            false,
        )
        .unwrap();

        assert_eq!(hits(&remote, "--all").unwrap(), 0);
        assert!(!remote_sha(&remote, "refs/tags/v1.0.0").is_empty());
    }

    #[test]
    fn clean_history_keeps_the_same_commit_id() {
        let temp = TempDir::new().unwrap();
        cmd("git", &["init", "-b", "main"], Some(temp.path())).unwrap();
        commit_file(temp.path(), "clean.txt", "clean", "feat: clean history");
        let before = git(temp.path(), &["rev-parse", "HEAD"]).unwrap();
        let report = crate::rewrite::rewrite_repository(temp.path()).unwrap();
        assert_eq!(git(temp.path(), &["rev-parse", "HEAD"]).unwrap(), before);
        assert_eq!(report.commits_rewritten, 0);
    }

    #[test]
    fn rewrite_preserves_merge_topology() {
        let temp = TempDir::new().unwrap();
        cmd("git", &["init", "-b", "main"], Some(temp.path())).unwrap();
        commit_file(temp.path(), "base.txt", "base", "base");
        cmd("git", &["switch", "-c", "feature"], Some(temp.path())).unwrap();
        commit_file(
            temp.path(),
            "feature.txt",
            "feature",
            "feature\n\nCo-Authored-By: Claude <noreply@anthropic.com>",
        );
        cmd("git", &["switch", "main"], Some(temp.path())).unwrap();
        commit_file(temp.path(), "main.txt", "main", "main work");
        cmd(
            "git",
            &[
                "-c",
                "user.name=Tester",
                "-c",
                "user.email=test@example.com",
                "merge",
                "--no-ff",
                "feature",
                "-m",
                "merge feature",
            ],
            Some(temp.path()),
        )
        .unwrap();

        crate::rewrite::rewrite_repository(temp.path()).unwrap();

        cmd("git", &["fsck", "--no-dangling"], Some(temp.path())).unwrap();
        let parents = git(temp.path(), &["rev-list", "--parents", "-1", "main"]).unwrap();
        assert_eq!(parents.split_whitespace().count(), 3);
        assert_eq!(hits(temp.path(), "--all").unwrap(), 0);
    }

    fn commit_file(repo: &Path, name: &str, content: &str, message: &str) {
        fs::write(repo.join(name), content).unwrap();
        cmd("git", &["add", name], Some(repo)).unwrap();
        cmd(
            "git",
            &[
                "-c",
                "user.name=Tester",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                message,
            ],
            Some(repo),
        )
        .unwrap();
    }

    fn remote_sha(remote: &Path, reference: &str) -> String {
        git(remote, &["rev-parse", reference]).unwrap()
    }
}
