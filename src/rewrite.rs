use anyhow::{bail, Context, Result};
use regex::bytes::Regex;
use std::{
    collections::HashMap,
    io::Write,
    path::Path,
    process::{Command, Stdio},
    sync::LazyLock,
};

static COAUTHOR_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^co-authored-by:.*(?:claude|anthropic).*$").unwrap());
static GENERATED_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:🤖\s*)?generated with claude.*$").unwrap());
static INLINE_COAUTHOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s*co-authored-by:\s*[^\n]*(?:claude|anthropic)[^\n]*$").unwrap()
});
static INLINE_GENERATED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s*(?:🤖\s*)?generated with claude(?: code)?[^\n]*").unwrap()
});
static ANTHROPIC_EMAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s*<?noreply@anthropic\.com>?").unwrap());

#[derive(Default)]
pub(crate) struct RewriteReport {
    pub commits_rewritten: usize,
    pub signed_commits_stripped: usize,
    pub signed_tags_stripped: usize,
}

pub(crate) fn rewrite_repository(repo: &Path) -> Result<RewriteReport> {
    let commits = git_text(
        repo,
        &[
            "rev-list",
            "--topo-order",
            "--reverse",
            "--branches",
            "--tags",
        ],
    )?;
    let mut mapped = HashMap::new();
    let mut report = RewriteReport::default();
    for old in commits.lines().filter(|line| !line.is_empty()) {
        let raw = git_bytes(repo, &["cat-file", "commit", old])?;
        let (rewritten, message_changed, signature_stripped) = rewrite_commit(&raw, &mapped)?;
        let new = write_object(repo, "commit", &rewritten)?;
        if new != old {
            report.commits_rewritten += usize::from(message_changed);
            report.signed_commits_stripped += usize::from(signature_stripped);
        }
        mapped.insert(old.to_owned(), new);
    }

    rewrite_branch_refs(repo, &mapped)?;
    rewrite_tag_refs(repo, &mapped, &mut report)?;
    Ok(report)
}

pub(crate) fn traced_commits(repo: &Path, revisions: &[&str]) -> Result<Vec<String>> {
    let mut args = vec!["rev-list"];
    args.extend_from_slice(revisions);
    let commits = git_text(repo, &args)?;
    commits
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(
            |commit| match git_bytes(repo, &["cat-file", "commit", commit]) {
                Ok(raw) => {
                    let split = raw.windows(2).position(|window| window == b"\n\n")?;
                    (scrub_message(&raw[split + 2..]) != raw[split + 2..])
                        .then(|| Ok(commit.to_owned()))
                }
                Err(error) => Some(Err(error)),
            },
        )
        .collect()
}

pub(crate) fn scrub_message(message: &[u8]) -> Vec<u8> {
    let mut lines: Vec<Vec<u8>> = Vec::new();
    for line in message.split(|byte| *byte == b'\n') {
        if COAUTHOR_LINE.is_match(line) || GENERATED_LINE.is_match(line) {
            continue;
        }
        let line = INLINE_COAUTHOR.replace_all(line, &b""[..]);
        let line = INLINE_GENERATED.replace_all(&line, &b""[..]);
        let line = ANTHROPIC_EMAIL.replace_all(&line, &b""[..]);
        if !line.is_empty() || lines.last().is_none_or(|previous| !previous.is_empty()) {
            lines.push(line.into_owned());
        }
    }
    while lines.last().is_some_and(Vec::is_empty) {
        lines.pop();
    }
    let mut output = lines.join(&b'\n');
    output.push(b'\n');
    output
}

fn rewrite_commit(raw: &[u8], mapped: &HashMap<String, String>) -> Result<(Vec<u8>, bool, bool)> {
    let split = raw
        .windows(2)
        .position(|window| window == b"\n\n")
        .context("commit object has no message separator")?;
    let headers = &raw[..split];
    let message = &raw[split + 2..];
    let cleaned = scrub_message(message);
    let message_changed = cleaned != message;
    let parent_changed = headers.split(|byte| *byte == b'\n').any(|line| {
        line.strip_prefix(b"parent ")
            .and_then(|parent| std::str::from_utf8(parent).ok())
            .and_then(|parent| mapped.get(parent).map(|new| new != parent))
            .unwrap_or(false)
    });
    if !message_changed && !parent_changed {
        return Ok((raw.to_vec(), false, false));
    }
    let mut output = Vec::with_capacity(raw.len());
    let mut skipping_signature = false;
    let mut signature_stripped = false;
    for line in headers.split(|byte| *byte == b'\n') {
        if line.starts_with(b"gpgsig ") || line.starts_with(b"mergetag ") {
            skipping_signature = true;
            signature_stripped = true;
            continue;
        }
        if skipping_signature && line.starts_with(b" ") {
            continue;
        }
        skipping_signature = false;
        if let Some(old_parent) = line.strip_prefix(b"parent ") {
            let old_parent = std::str::from_utf8(old_parent).context("non-UTF-8 parent hash")?;
            let new_parent = mapped
                .get(old_parent)
                .with_context(|| format!("parent {old_parent} was not rewritten first"))?;
            output.extend_from_slice(b"parent ");
            output.extend_from_slice(new_parent.as_bytes());
        } else {
            output.extend_from_slice(line);
        }
        output.push(b'\n');
    }
    output.push(b'\n');
    output.extend_from_slice(&cleaned);
    Ok((output, message_changed, signature_stripped))
}

fn rewrite_branch_refs(repo: &Path, mapped: &HashMap<String, String>) -> Result<()> {
    for line in git_text(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads",
        ],
    )?
    .lines()
    {
        let (reference, old) = line.split_once(' ').context("invalid branch ref listing")?;
        let new = mapped
            .get(old)
            .with_context(|| format!("branch tip {old} was not rewritten"))?;
        git_status(repo, &["update-ref", reference, new, old])?;
    }
    Ok(())
}

fn rewrite_tag_refs(
    repo: &Path,
    mapped: &HashMap<String, String>,
    report: &mut RewriteReport,
) -> Result<()> {
    let mut tag_map = HashMap::new();
    for line in git_text(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname) %(objecttype)",
            "refs/tags",
        ],
    )?
    .lines()
    {
        let mut parts = line.splitn(3, ' ');
        let reference = parts.next().context("missing tag ref")?;
        let old = parts.next().context("missing tag object")?;
        let object_type = parts.next().context("missing tag type")?;
        let new = if object_type == "tag" {
            rewrite_annotated_tag(repo, old, mapped, &mut tag_map, report)?
        } else {
            mapped.get(old).cloned().unwrap_or_else(|| old.to_owned())
        };
        if new != old {
            git_status(repo, &["update-ref", reference, &new, old])?;
        }
    }
    Ok(())
}

fn rewrite_annotated_tag(
    repo: &Path,
    old: &str,
    commits: &HashMap<String, String>,
    tags: &mut HashMap<String, String>,
    report: &mut RewriteReport,
) -> Result<String> {
    if let Some(new) = tags.get(old) {
        return Ok(new.clone());
    }
    let raw = git_bytes(repo, &["cat-file", "tag", old])?;
    let split = raw
        .windows(2)
        .position(|window| window == b"\n\n")
        .context("tag object has no message separator")?;
    let mut headers = raw[..split].to_vec();
    let message = &raw[split + 2..];
    let object_line_end = headers
        .iter()
        .position(|byte| *byte == b'\n')
        .context("tag object has no target")?;
    let target = std::str::from_utf8(
        headers[..object_line_end]
            .strip_prefix(b"object ")
            .context("invalid tag target")?,
    )?;
    let target_type = git_text(repo, &["cat-file", "-t", target])?;
    let new_target = if target_type == "tag" {
        rewrite_annotated_tag(repo, target, commits, tags, report)?
    } else {
        commits
            .get(target)
            .cloned()
            .unwrap_or_else(|| target.to_owned())
    };
    if new_target == target {
        tags.insert(old.to_owned(), old.to_owned());
        return Ok(old.to_owned());
    }
    headers.splice(..object_line_end, format!("object {new_target}").bytes());
    let (message, stripped) = strip_tag_signature(message);
    report.signed_tags_stripped += usize::from(stripped);
    let mut rewritten = headers;
    rewritten.extend_from_slice(b"\n\n");
    rewritten.extend_from_slice(message);
    let new = write_object(repo, "tag", &rewritten)?;
    tags.insert(old.to_owned(), new.clone());
    Ok(new)
}

fn strip_tag_signature(message: &[u8]) -> (&[u8], bool) {
    [
        b"-----BEGIN PGP SIGNATURE-----".as_slice(),
        b"-----BEGIN SSH SIGNATURE-----".as_slice(),
    ]
    .iter()
    .filter_map(|marker| {
        message
            .windows(marker.len())
            .position(|window| window == *marker)
    })
    .min()
    .map_or((message, false), |position| (&message[..position], true))
}

fn write_object(repo: &Path, kind: &str, data: &[u8]) -> Result<String> {
    let mut child = Command::new("git")
        .args(["hash-object", "-t", kind, "-w", "--stdin"])
        .current_dir(repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .context("open git stdin")?
        .write_all(data)?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!(
            "git hash-object failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn git_bytes(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git").args(args).current_dir(repo).output()?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}
fn git_text(repo: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8_lossy(&git_bytes(repo, args)?)
        .trim()
        .to_owned())
}
fn git_status(repo: &Path, args: &[&str]) -> Result<()> {
    git_bytes(repo, args).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scrub_preserves_human_and_unicode_content() {
        let input = "feat: 한글\n\nCo-Authored-By: Human <human@example.com>\nCo-Authored-By: Claude <noreply@anthropic.com>\n";
        let output = String::from_utf8(scrub_message(input.as_bytes())).unwrap();
        assert!(output.contains("feat: 한글"));
        assert!(output.contains("Human <human@example.com>"));
        assert!(!output.to_ascii_lowercase().contains("claude"));
        assert!(!output.to_ascii_lowercase().contains("anthropic"));
    }
    #[test]
    fn scrub_preserves_invalid_utf8() {
        let input = b"subject \xff\n\nCo-Authored-By: Claude <noreply@anthropic.com>\n";
        assert!(scrub_message(input).contains(&0xff));
    }
}
