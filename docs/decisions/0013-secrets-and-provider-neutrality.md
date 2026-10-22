# 0013. Secrets hygiene and provider neutrality

Date: 2026-10-22. Status: accepted.

## Context

This repo must become public for F-Droid. A handoff summary leaked a live
provider API key and the internal gateway hostname; tracked files carried the
hostname, gateway model aliases and `GRID_*` env names since M1-prep. The live
key was verified absent from the tree and from all git history; the hostname
resides in history commits (reported to the owner; history is theirs to
rewrite). Secrets and anyone's internal infrastructure must never re-enter
summaries, docs, ADRs or logs.

## Decision

1. The tree contains no provider key material, no gateway hostnames and no
   gateway model aliases. Endpoint, key and model are read from env only:
   `PALMCODE_LLM_BASE_URL`, `PALMCODE_LLM_API_KEY`, `PALMCODE_LLM_MODEL`
   (`PALMCODE_LLM_FAST_MODEL` for the fast role). No defaults are hardcoded.
2. Docs and fixtures say "gateway" and the role labels `main-model` /
   `fast-model`. Fixtures carry placeholders (`mock-main` / `mock-fast` model
   ids, `https://<gateway-host>/v1`, `<main-model-alias>`).
3. gitleaks 8.30.1 (MIT) runs in CI (release pinned, sha256-verified) and as
   `.githooks/pre-commit` (documented; enable with
   `git config core.hooksPath .githooks`). `.gitleaks.toml` extends the
   default rules with a dash-tolerant `sk-` key-shape rule. CI fails on
   findings.
4. GLM-family wire-format code stays: `SYSTEM_PROMPT_BUILD_GLM`, the `glm45`
   tool-call parser name, `mock-glm*` mock ids and the `glm-4.6` family
   detection tests denote public model-family wire compatibility, not
   provider infrastructure.

## Rejected

- **History rewrite.** Shared history; the owner handles it. The hostname
  entered at the M1-prep commit and persists through HEAD; upgrade CI to a
  full-history `gitleaks git` scan once it is rewritten.
- **Hostname/alias regression rules in gitleaks.** A detector must encode the
  identifier it detects, re-embedding it. Removal beats detection here.
- **`gitleaks/gitleaks-action`.** Dead since 2026-09 (Node 20 removed from
  runners); a checksum-pinned binary also matches our supply-chain rule.
- **Fail-closed hook when gitleaks is absent.** Would block FOSS
  contributors; the hook warns and CI is the enforcement gate.
- **Blanket `core/vendor/` exclusion from scanning.** gitleaks path
  allowlists skip files entirely (verified empirically), so a blanket
  exclusion would hide planted keys; instead only the enumerated
  upstream test-data files are excluded and the rest of vendor is scanned.

## Consequences

- Env contract is documented in README ("Secrets policy").
- Bench/live tooling (`edit-bench`, `scripts/probe-vllm.ps1`,
  `scripts/trace-sse.ps1`, `core/llm/tests/live.rs`) all read the
  `PALMCODE_LLM_*` vars and refuse to run without them (the bench defaults
  only to the local mock at `http://127.0.0.1:8787/v1`).
- Generic key-shape rules cover secrets; identifier hygiene relies on review
  plus this ADR's rule ("no provider identifiers in the tree").
