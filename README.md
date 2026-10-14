# AntiClaude

AntiClaude removes Claude/Anthropic attribution from every branch and tag in a GitHub repository, force-pushes the cleaned history, and refreshes GitHub's cached Contributors graph.

It is a Rust implementation inspired by [declaude](https://github.com/ediiloupatty/declaude).

## Install

Prerequisites:

- Rust 1.85 or newer
- `git`
- [`git-filter-repo`](https://github.com/newren/git-filter-repo)
- [GitHub CLI](https://cli.github.com/) authenticated with `gh auth login`

Install the executable from this checkout:

```sh
cargo install --path .
```

## Use

Run the complete operation with one command:

```sh
anticlaude OWNER/REPO
```

A full GitHub URL also works:

```sh
anticlaude https://github.com/OWNER/REPO
```

AntiClaude shows the affected refs and asks for confirmation. Use `-y` in automation:

```sh
anticlaude OWNER/REPO -y
```

Useful options:

```text
--dry-run      inspect without changing anything
--no-refresh   skip the GitHub Contributors graph refresh
--no-backup    skip the backup bundle (not recommended)
```

Prevent Claude Code from adding attribution to future commits:

```sh
anticlaude prevent
```

## Safety and behavior

- A restorable bundle is written to `~/.anticlaude-backups/` before history changes.
- Only commit messages are cleaned. Claude author or committer identities are reported but not changed.
- All remote branches are materialized and rewritten.
- Only tags whose object changed are force-pushed.
- AntiClaude waits for GitHub to index the rewritten history before refreshing the graph.
- The graph refresh renames the default branch temporarily, restores it, then pushes one empty `chore: refresh GitHub contributors` commit using the latest commit's author identity.
- Protected branches, protected tags, and repository rulesets can reject force-pushes.

Restore a backup into a clone with:

```sh
git fetch ~/.anticlaude-backups/REPO-TIMESTAMP.bundle '*:*'
```

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

## License

MIT

