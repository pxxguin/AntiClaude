# Contributing to AntiClaude

Thank you for helping improve AntiClaude. Changes that affect Git history must put repository safety and predictable recovery ahead of convenience.

## Before You Start

Search existing issues before opening a new one. For a substantial feature, behavior change, or new external dependency, open an issue first so the scope and safety implications can be discussed.

Security vulnerabilities must not be reported in a public issue. Follow the [security policy](SECURITY.md) instead.

## Development Setup

Install a current stable Rust toolchain and Git, then clone the repository:

```sh
git clone https://github.com/pxxguin/AntiClaude.git
cd AntiClaude
cargo build --locked
```

Create a focused branch for your change. Keep unrelated formatting or refactoring out of the same pull request.

## Making Changes

- Keep commits atomic and write commit subjects and descriptions in English.
- Use Conventional Commit prefixes such as `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, and `chore:`.
- Add regression tests for bug fixes and tests for new behavior.
- Use temporary local and bare repositories in tests. Never run history-rewriting tests against a real remote repository.
- Preserve dry-run behavior, backup creation, lease checks, and atomic ref updates when changing the rewrite workflow.
- Avoid adding an external executable dependency unless the capability cannot reasonably be implemented or reused safely in Rust.
- Update user-facing documentation when commands, flags, requirements, or safety behavior change.

## Validation

Run all checks before submitting a pull request:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked --release
```

## Pull Requests

A pull request should explain the problem, the chosen approach, user-visible effects, and safety implications. Include the commands used to validate the change. Screenshots are useful only when output formatting changes.

By submitting a contribution, you agree that it may be distributed under either the [MIT License](LICENSE-MIT) or the [Apache License 2.0](LICENSE-APACHE), at the recipient's option.

All contributors must follow the [Code of Conduct](CODE_OF_CONDUCT.md).
