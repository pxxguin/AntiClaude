# Security Policy

AntiClaude rewrites Git history and updates remote references. A security defect can cause data loss, overwrite concurrent work, disclose repository data, or execute an unintended command. Please report suspected vulnerabilities privately.

## Supported Versions

Security fixes are provided for the latest released version. Older versions may not receive patches; reproduce the issue with the latest release when it is safe to do so.

## Reporting a Vulnerability

Use [GitHub's private vulnerability reporting](https://github.com/pxxguin/AntiClaude/security/advisories/new). Do not open a public issue, pull request, or discussion for an undisclosed vulnerability.

Include as much of the following as possible:

- The affected AntiClaude version and installation method.
- The operating system and relevant Git or GitHub CLI versions.
- A minimal reproduction using a disposable repository.
- The expected behavior, actual behavior, and potential impact.
- Sanitized logs or proof-of-concept material.
- Any known mitigations or conditions required for exploitation.

Do not include access tokens, credentials, private repository contents, or personal information. Revoke any credential that may have been exposed.

The maintainer will review the report, confirm the affected scope, and coordinate a fix and disclosure when appropriate. Please allow time for a patch to be prepared before publishing details.

## Security-Sensitive Areas

Reports are especially helpful when they involve:

- Command or argument injection.
- Unsafe ref selection, ref overwrite, or force-with-lease bypass.
- Partial or non-atomic remote updates.
- Backup bundle exposure or incomplete recovery data.
- Temporary repository or filesystem permission issues.
- Path traversal or unintended file modification.
- Corruption of commits, annotated tags, signatures, or Claude Code settings.
- Leakage of repository URLs, credentials, or command output.

General bugs without a security impact may be reported through [GitHub Issues](https://github.com/pxxguin/AntiClaude/issues).
