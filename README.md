<p align="center">
  <img width="28%" src="AntiClaude.svg" alt="AntiClaude logo" />
</p>

<h1 align="center">AntiClaude</h1>

<p align="center">
  Remove Claude and Anthropic attribution from a GitHub repository in one command.
</p>

**AntiClaude is a Rust CLI that cleans AI attribution from Git history and safely updates affected remote refs.**

Claude Code can add `Co-Authored-By` trailers and generation notices to commit messages. Removing them from the latest commit is not enough: older commits, branches, and tags may still retain the attribution. AntiClaude handles the cleanup workflow without modifying the repository's file contents or human authorship. An experimental, explicitly selected command is available for attempting to refresh GitHub's cached contributor statistics.

## Get Started

### Requirements

- Rust 1.85 or newer
- [Git](https://git-scm.com/)
- [`git-filter-repo`](https://github.com/newren/git-filter-repo)
- [GitHub CLI](https://cli.github.com/), authenticated with `gh auth login`

On macOS, the external tools can be installed with Homebrew:

```sh
brew install git-filter-repo gh
gh auth login
```

### Install

Prebuilt archives for Linux, macOS, and Windows are published on the [Releases page](https://github.com/pxxguin/AntiClaude/releases).

To build and install from this repository:

```sh
cargo install --path .
```

### Run

Pass a GitHub `OWNER/REPO` slug or full repository URL:

```sh
anticlaude clean OWNER/REPO
```

```sh
anticlaude clean https://github.com/OWNER/REPO
```

AntiClaude clones the repository into a temporary directory, shows the affected branches and tags, and asks for confirmation before changing the remote.

## What AntiClaude Does

One command performs the full cleanup sequence:

1. Clones every remote branch into a temporary working directory.
2. Detects Claude and Anthropic traces across all branches and tags.
3. Creates a restorable Git bundle in `~/.anticlaude-backups/`.
4. Rewrites matching commit messages with `git-filter-repo`.
5. Verifies that the remote refs have not changed since cloning.
6. Updates every affected branch and tag in one atomic push.
7. Deletes the temporary local clone.

AntiClaude removes message attribution such as:

```text
Co-Authored-By: Claude <noreply@anthropic.com>
Generated with Claude Code
```

It does not modify tracked files or replace commit author and committer identities.

## Commands and Options

Preview the operation without changing anything:

```sh
anticlaude clean OWNER/REPO --dry-run
```

Skip the confirmation prompt:

```sh
anticlaude clean OWNER/REPO --yes
```

Experimentally refresh GitHub's Contributors graph after cleaning:

```sh
anticlaude refresh OWNER/REPO
```

Skip the backup bundle:

```sh
anticlaude clean OWNER/REPO --no-backup
```

Prevent Claude Code from adding attribution to future commits:

```sh
anticlaude prevent
```

For the complete CLI reference:

```sh
anticlaude --help
```

## Safety

History rewriting changes commit IDs and requires force-pushing. Use AntiClaude only on repositories where you are authorized to rewrite history. Contributors refresh is intentionally separate because it temporarily renames the default branch and creates an empty commit.

- A backup bundle is created before rewriting unless `--no-backup` is supplied.
- A single atomic push prevents a repository from being left with only some refs updated.
- Explicit force-with-lease checks prevent overwriting refs changed after AntiClaude cloned the repository.
- Protected branches, protected tags, and repository rulesets may reject the push.
- Collaborators must synchronize their local clones after rewritten history is pushed.
- Annotated release tags may receive new object IDs, while GitHub release assets remain associated with their tag names.
- `anticlaude refresh` is experimental; GitHub does not guarantee cache invalidation and statistics may take time to settle.
- Forks generally should not be rewritten when they must remain compatible with their upstream repository.

Restore refs from a backup bundle with:

```sh
git fetch ~/.anticlaude-backups/REPO-TIMESTAMP.bundle '*:*'
```

## Development

Run the test suite and static analysis before submitting changes:

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
```

### Project Structure

```text
src/
├── main.rs      # executable entry point
├── lib.rs       # library entry point and command dispatch
├── cli.rs       # command-line arguments and command model
├── app.rs       # repository cleanup and GitHub refresh workflow
└── settings.rs  # Claude Code attribution settings
```

## Contributions and Feedback

Bug reports, feature requests, and pull requests are welcome. When reporting a failure, include the AntiClaude version, operating system, command options, and sanitized command output. Do not include access tokens or private repository content.

## Acknowledgements

AntiClaude is a Rust implementation inspired by [ediiloupatty/declaude](https://github.com/ediiloupatty/declaude).

## License

This project is licensed under the [MIT License](LICENSE).
