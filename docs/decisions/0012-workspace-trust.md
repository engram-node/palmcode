# 0012: Workspace trust + DB-persisted project approvals

Status: accepted (M3 pre-work), supersedes the M2-D "project approvals write
`.palmcode/config.json`" behavior.

## Context

M2 treated every workspace as fully trusted: `<root>/.palmcode/config.json`
rules (including `allow`) applied verbatim, and "Always in project"
approvals were persisted BY WRITING THAT FILE. Both are unsafe once the app
imports arbitrary third-party trees via SAF: a hostile or merely sloppy repo
could ship a self-granting config, and our own writes taught the repo the
grants it needed.

## Decision

- Workspaces have a `trusted` flag (`workspaces.trusted`, default 0 on
  import; user toggles it in the Workspaces screen).
- **Untrusted**: `.palmcode/config.json` is ignored entirely (read errors,
  malformed JSON, allow/deny — all moot).
- **Trusted**: the file is parsed fail-closed, but `allow` rules are
  stripped at load; only `ask`/`deny` apply. A repo can tighten its own
  sandbox, never loosen it.
- "Always for project" approvals persist in the app DB
  (`project_approvals`, keyed by workspace id), never in repo files. They
  form a layer between session grants and project config.
- Tool writes to any `.palmcode/**` path fall to Ask (built-in), so the
  model can propose config edits but never make them silently.
- RootSet containment: canonicalization failure now hard-errors EXCEPT on
  PermissionDenied (Android FUSE blocks realpath for app UIDs), where the
  lexical basis stays the shared fallback. Host test asserts canonical
  roots for private storage.

## Alternatives rejected

- Keep writing `.palmcode/config.json` for project grants (rejected: trust
  leaks into the trusteed repo; also pollutes user projects).
- Allow rules in trusted project configs (rejected: prompt-injection inside
  a trusted repo could still plant an allow via a normal edit → silent
  escalation later).
- RootSet hard-error on every canonicalize failure (rejected: breaks
  FUSE-attached dev workspaces entirely).

## Consequences

- Existing M2 installs: one-column migration (`workspaces.trusted`,
  default 0); previously file-persisted approvals are NOT migrated — users
  re-approve once (documented, one Ask each).
- The Kotlin Workspaces screen gains a trust toggle; flips apply to new
  tool contexts (next turn).
