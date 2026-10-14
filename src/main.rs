use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use regex::Regex;
use serde_json::Value;
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tempfile::TempDir;

const REFRESH: &str = "chore: refresh GitHub contributors";
const CALLBACK: &str = r#"import re
text=message.decode('utf-8','replace'); out=[]
for line in text.split('\n'):
 low=line.strip().lower()
 if low.startswith('co-authored-by:') and ('claude' in low or 'anthropic' in low): continue
 if low.startswith('🤖 generated with') or low.startswith('generated with claude'): continue
 line=re.sub(r'(?i)\s*co-authored-by:\s*[^\n]*(?:claude|anthropic)[^\n]*$','',line)
 line=re.sub(r'(?i)\s*🤖?\s*generated with claude(?: code)?[^\n]*','',line)
 line=re.sub(r'(?i)\s*<?noreply@anthropic\.com>?','',line)
 if low and not line.strip(): continue
 out.append(line)
return (re.sub(r'\n{3,}','\n\n','\n'.join(out)).rstrip('\n')+'\n').encode('utf-8')"#;

#[derive(Parser)]
#[command(
    name = "anticlaude",
    version,
    about = "Remove Claude/AI attribution from a GitHub repository in one command"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Action>,
    /// GitHub URL or OWNER/REPO slug
    target: Option<String>,
    /// Skip confirmation
    #[arg(short = 'y', long)]
    yes: bool,
    /// Show the plan only
    #[arg(long)]
    dry_run: bool,
    /// Skip contributors graph refresh
    #[arg(long)]
    no_refresh: bool,
    /// Skip the restorable backup bundle
    #[arg(long)]
    no_backup: bool,
}
#[derive(Subcommand)]
enum Action {
    /// Disable future Claude Code attribution
    Prevent,
}
#[derive(Clone)]
struct Target {
    url: String,
    slug: String,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1)
    }
}
fn run() -> Result<()> {
    let c = Cli::parse();
    if matches!(c.command, Some(Action::Prevent)) {
        return prevent();
    }
    let target = normalize(
        c.target
            .as_deref()
            .context("provide a GitHub URL or OWNER/REPO slug")?,
    )?;
    clean(target, c.yes, c.dry_run, !c.no_refresh, !c.no_backup)
}

fn clean(target: Target, yes: bool, dry: bool, refresh: bool, backup_enabled: bool) -> Result<()> {
    require("git", &["--version"])?;
    require("git-filter-repo", &["--version"])?;
    let gh = available("gh");
    if gh {
        cmd("gh", &["auth", "status"], None).context("authenticate first with `gh auth login`")?;
    } else if refresh {
        bail!("GitHub CLI `gh` is required for the contributors graph refresh")
    }
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
    let identities = identity_hits(&repo)?;
    if identities > 0 {
        println!("  warning: {identities} Claude author/committer identities are not rewritten");
    }
    if dry {
        println!("\nDry run complete; nothing changed.");
        return Ok(());
    }
    if !yes {
        confirm(hit_count > 0)?
    }
    if hit_count > 0 {
        if backup_enabled {
            println!("Backup: {}", backup(&repo, &target.slug)?.display())
        } else {
            println!("Warning: backup disabled.")
        }
        let origin = git(&repo, &["remote", "get-url", "origin"])?;
        cmd(
            "git",
            &["filter-repo", "--force", "--message-callback", CALLBACK],
            Some(&repo),
        )?;
        if hits(&repo, "--all")? > 0 {
            bail!("traces remain after rewriting; refusing to push")
        }
        let _ = cmd("git", &["remote", "remove", "origin"], Some(&repo));
        cmd(
            "git",
            &["remote", "add", "origin", origin.trim()],
            Some(&repo),
        )?;
        for b in &branches {
            println!("Force-pushing branch {b}...");
            cmd("git", &["push", "origin", b, "--force"], Some(&repo))
                .with_context(|| format!("push {b}; check branch protection"))?;
        }
        for t in &tags {
            println!("Force-pushing tag {t}...");
            cmd(
                "git",
                &["push", "origin", &format!("refs/tags/{t}"), "--force"],
                Some(&repo),
            )
            .with_context(|| format!("push tag {t}; check tag protection"))?;
        }
        if refresh && !wait_clean(&target.slug)? {
            bail!("clean history was pushed, but GitHub is still indexing it; rerun shortly to refresh the graph")
        }
    }
    if refresh {
        refresh_graph(&repo, &target.slug)?;
        println!("\nDone. History and contributors graph are clean.")
    } else {
        println!("\nDone. Graph refresh skipped.")
    }
    Ok(())
}

fn normalize(s: &str) -> Result<Target> {
    let v = s.trim().trim_end_matches('/').trim_end_matches(".git");
    let slug = v
        .strip_prefix("https://github.com/")
        .or_else(|| v.strip_prefix("http://github.com/"))
        .or_else(|| v.strip_prefix("git@github.com:"))
        .unwrap_or(v);
    if !Regex::new(r"^[\w.-]+/[\w.-]+$").unwrap().is_match(slug) {
        bail!("only GitHub OWNER/REPO slugs and github.com URLs are supported")
    }
    Ok(Target {
        url: format!("https://github.com/{slug}.git"),
        slug: slug.into(),
    })
}
fn cmd(p: &str, a: &[&str], cwd: Option<&Path>) -> Result<Output> {
    let mut c = Command::new(p);
    c.args(a);
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    let o = c.output().with_context(|| format!("run `{p}`"))?;
    if !o.status.success() {
        let e = String::from_utf8_lossy(if o.stderr.is_empty() {
            &o.stdout
        } else {
            &o.stderr
        });
        bail!("`{} {}` failed: {}", p, a.join(" "), e.trim())
    }
    Ok(o)
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
    Ok(git(
        r,
        &[
            "log",
            reference,
            "-i",
            "-E",
            "--grep=co-authored-by:.*(claude|anthropic)",
            "--grep=generated with claude",
            "--grep=noreply@anthropic",
            "--format=%H",
        ],
    )?
    .lines()
    .count())
}
fn dirty_refs(r: &Path, ns: &str) -> Result<Vec<String>> {
    Ok(refs(r, ns)?
        .into_iter()
        .filter(|x| hits(r, x).unwrap_or(0) > 0)
        .collect())
}
fn identity_hits(r: &Path) -> Result<usize> {
    let re = Regex::new("(?i)claude|anthropic").unwrap();
    Ok(git(r, &["log", "--all", "--format=%an <%ae>|%cn <%ce>"])?
        .lines()
        .filter(|x| re.is_match(x))
        .count())
}
fn backup(r: &Path, slug: &str) -> Result<PathBuf> {
    let d = dirs::home_dir()
        .context("find home directory")?
        .join(".anticlaude-backups");
    fs::create_dir_all(&d)?;
    let n = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let p = d.join(format!("{}-{n}.bundle", slug.rsplit('/').next().unwrap()));
    let s = p.to_string_lossy();
    cmd("git", &["bundle", "create", &s, "--all"], Some(r))?;
    Ok(p)
}
fn confirm(rewrite: bool) -> Result<()> {
    print!(
        "This will {}force-push and refresh GitHub's cache. Type `yes`: ",
        if rewrite { "rewrite history, " } else { "" }
    );
    io::stdout().flush()?;
    let mut s = String::new();
    io::stdin().read_line(&mut s)?;
    if !matches!(s.trim().to_lowercase().as_str(), "y" | "yes") {
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
fn wait_clean(slug: &str) -> Result<bool> {
    for i in 0..6 {
        let n = server_hits(slug)?;
        if n == 0 {
            return Ok(true);
        }
        if i == 0 {
            println!("GitHub still reports {n} traces; waiting for indexing...")
        }
        thread::sleep(Duration::from_secs(5));
    }
    Ok(false)
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
fn refresh_commit(r: &Path, amend: bool) -> Result<()> {
    let b = git(r, &["branch", "--show-current"])?;
    let n = git(r, &["log", "-1", "--format=%an"])?;
    let e = git(r, &["log", "-1", "--format=%ae"])?;
    let name_config = format!("user.name={n}");
    let email_config = format!("user.email={e}");
    let mut a = vec![
        "-c",
        "commit.gpgsign=false",
        "-c",
        &name_config,
        "-c",
        &email_config,
        "commit",
    ];
    if amend {
        a.extend(["--amend", "--allow-empty", "-m", REFRESH])
    } else {
        a.extend(["--allow-empty", "-m", REFRESH])
    }
    cmd("git", &a, Some(r))?;
    let mut p = vec!["push", "origin", &b];
    if amend {
        p.push("--force")
    }
    cmd("git", &p, Some(r))?;
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
    let re = Regex::new("(?i)claude|anthropic").unwrap();
    for attempt in 0..3 {
        flush(slug)?;
        refresh_commit(r, attempt > 0)?;
        println!("Waiting for GitHub to rebuild the contributors graph...");
        for _ in 0..10 {
            if let Some(v) = logins(slug)? {
                if !v.iter().any(|x| re.is_match(x)) {
                    return Ok(());
                }
                break;
            }
            thread::sleep(Duration::from_secs(6));
        }
    }
    bail!("history is clean, but GitHub's contributors graph is still stale")
}
fn prevent() -> Result<()> {
    let p = dirs::home_dir()
        .context("find home directory")?
        .join(".claude/settings.json");
    let mut v: Value = if p.exists() {
        serde_json::from_str(&fs::read_to_string(&p)?)?
    } else {
        serde_json::json!({})
    };
    v.as_object_mut()
        .context("settings must be a JSON object")?
        .insert("includeCoAuthoredBy".into(), Value::Bool(false));
    if let Some(d) = p.parent() {
        fs::create_dir_all(d)?
    }
    fs::write(&p, format!("{}\n", serde_json::to_string_pretty(&v)?))?;
    println!("Set includeCoAuthoredBy=false in {}", p.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slug() {
        assert_eq!(normalize("owner/repo").unwrap().slug, "owner/repo")
    }
    #[test]
    fn url() {
        assert_eq!(
            normalize("https://github.com/owner/repo.git").unwrap().slug,
            "owner/repo"
        )
    }
    #[test]
    fn rejects() {
        assert!(normalize("https://example.com/a/b").is_err())
    }

    #[test]
    fn rewrite_removes_trace_and_preserves_message() {
        if !available("git-filter-repo") {
            return;
        }
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
                "feat: keep this",
                "-m",
                "Co-Authored-By: Claude <noreply@anthropic.com>",
            ],
            Some(temp.path()),
        )
        .unwrap();
        cmd(
            "git",
            &["filter-repo", "--force", "--message-callback", CALLBACK],
            Some(temp.path()),
        )
        .unwrap();
        let message = git(temp.path(), &["log", "-1", "--format=%B"]).unwrap();
        assert!(message.contains("feat: keep this"));
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
}
