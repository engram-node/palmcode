# M2 / M3 closure — 2026-10-27

Acceptance is **not closed**. DONE means the stated implementation/test
scope has evidence; it does not imply unlisted Android behavior. Tests
mentioned below are in the current tree. Historical device evidence stays
dated in VERIFY.md. PENDING-DEVICE requires maintainer device validation;
MISSING covers implementation, measurement or validation evidence still absent.

Source inventory: the recoverable M2 design is ADR 0011; M3 contracts are
ADRs 0014/0015 and the dated VERIFY.md rows. This table also covers every
Stage 2/3 requirement supplied in this conversation and the Stage 4 additions.
A separate original specification is not present in the supplied context or
the targeted milestone-document search. Exact row-for-row equivalence to
that unavailable document cannot be certified; the traceability gap below
is explicit rather than inventing acceptance criteria or deliverable letters.

Stage 4 implementation: `dd01e08`; named test coverage: `c145b07`.

## M2 recorded criteria

| Criterion | Status | Commit or test evidence / remaining gap |
|---|---|---|
| Original-spec traceability | MISSING | Need the authoritative standalone text to certify that no original criterion was omitted; ADR-derived inventory below is complete for ADR 0011's recorded requirements. |
| A: real tool-call/request loop until final answer | DONE | `tool_loop::read_tool_executes_and_loops_to_answer`; `bash_tool_runs_and_streams_output`. |
| A: parallel read-only calls, deterministic result order | DONE | `tool_loop::parallel_reads_all_finish_in_call_order`. |
| A: serial writes and bounded model iterations | DONE | `run_turn` / `execute_tool_batch`; `MAX_TOOL_ITERATIONS = 40`; bash batches use serial branch. |
| A/G: per-iteration context guard | DONE | `usage_calibrates_bytes_per_token_ema`; `run_turn` checks the 85% guard before dispatch. |
| B: read, ls, glob, grep, write, edit, apply_patch | DONE | `core/tools::schemas`, tool-loop tests, edit golden tests, patch and syntax integration tests. |
| B/A: todoread, todowrite, question | MISSING | Mentioned in ADR 0011; no schemas/agent dispatch for these tools. The ADR's question proposal is tentative; original-spec requirement cannot be confirmed. |
| C: canonical roots, symlink/parent escape rejection | DONE | `paths::parent_escape_denied`, `absolute_outside_denied`, `symlink_escape_denied`, `private_roots_are_canonical_not_lexical`. |
| C: Unicode/case/Windows path adversarial coverage | DONE | `unicode_lookalike_dots_stay_inside_not_denied`, `case_variant_of_root_allowed`, `ntfs_ads_rejected`, `empty_and_control_rejected`. Unicode lookalikes remain literal filenames inside the root. |
| C: read-before-write and stale-write detection | DONE | `guards::write_without_read_refused_then_allowed`, `stale_after_external_change`, `deleted_after_read_is_stale`; `golden_stale_guard_blocks_edit`. |
| C: bounded file-tool output and spill references | DONE | `caps::over_line_cap_spills_and_points`, `over_byte_cap_cuts_at_char_boundary`. |
| C: post-edit syntax checks and warnings | DONE | `syntax::nine_grammars_flag_broken_and_pass_clean`; `write_appends_syntax_warning_on_broken_code`, `patch_checks_every_touched_known_file`. Current extensions cover Rust/Python/JS/TS/TSX/JSON/C/C++/Go/Java. |
| C: recorded Kotlin/YAML/Markdown/shell grammar coverage | MISSING | ADR 0011 lists these; `syntax::language_for` has no mappings for them. No vendor/grammar changes made in Stage 4. |
| D: builtin rules, trusted project ask/deny, session/project grants | DONE | `defaults_reads_allow_writes_ask`, `trusted_project_keeps_deny_ask_strips_allow`, `untrusted_ignores_project_config_completely`, `db_project_approvals_allow`, `session_grant_and_auto_accept`. |
| D: user-global permission configuration in the app | MISSING | Permission loader supports an optional global path; agent calls `Engine::load(..., None, ...)`, so the app does not supply one. |
| D: secrets ask, .git protection, malformed config fails closed | DONE | `defaults_git_deny_and_secrets_ask`, `palmcode_dir_writes_always_ask`, `malformed_config_is_error_not_allow`; adversarial bash suites. |
| D: tool output cannot grant permissions | DONE | `denied_write_runs_nothing`; permission updates originate from config/UI approval APIs, not tool-result parsing. Bash grants match exact commands. |
| D/H: once/session/project/deny in-app approval flow | PENDING-DEVICE | `write_waits_for_approval_then_errors_on_bad_args`, `denied_write_runs_nothing`, `accept_edits_skips_the_sheet`; historical M2 approvals in VERIFY.md, current UI needs maintainer checks. |
| E: workspace registry, metrics, selection/removal and restore | DONE | `register_list_metrics_remove`, `delete_files_only_inside_import_dir`, `removing_active_root_clears_the_workspace`, `most_recent_workspace_is_restored_on_restart`. |
| E: current SAF import and process-death UX | PENDING-DEVICE | Historical 2026-10-12 import/restore evidence in VERIFY.md; current app/device acceptance remains manual. |
| F: persisted calls, preimages and event order | DONE | `tool_call_lifecycle`, `preimages_roundtrip`, writer FIFO/rollback tests; `155f290`. |
| F: interrupted calls become outcome-unknown; resume never replays | DONE | `pending_calls_marked_interrupted_never_rerun`, `interrupted_bash_resume_never_replays_the_persisted_command`; historical real-process crash regression in VERIFY.md Stage 1. |
| F: recovery through the current Android app after kill | PENDING-DEVICE | Maintainer kill-mid-command/reopen/resume check remains pending. Host/store proof does not replace this. |
| F: retention/GC and freelist maintenance primitives | DONE | Store `gc` and `db_size_and_gc` exist. |
| F: automatic 30-day retention/boot VACUUM and DB-size settings | MISSING | No agent/FFI wiring for `gc`/database size was found; store unit coverage alone does not satisfy the recorded UI/automatic behavior. |
| G: persisted per-model bytes/token EMA | DONE | `usage_calibrates_bytes_per_token_ema`; `ema_bpt:*` store metadata. |
| G: recorded extra 5% safety margin and calibration-error log | MISSING | Current estimate divides bytes by EMA and uses the 85% threshold; no extra 5% factor or separate calibration-error metadata is wired. |
| H: virtualized/collapsible tool cards, preimage diffs, rehydration | DONE | `efac398`; `persisted_cards_preserve_all_tool_kinds_and_bound_unicode_output`; historical diff evidence in VERIFY.md. |
| H: current footer/autoscroll, touch-scroll behavior, short-decode display | PENDING-DEVICE | UI code/clamp exists; maintainer interaction checks remain pending. Stage 3 paced read cards do not exercise live bash scrolling. |
| I: deterministic schemas and byte-stable prefix before workspace tail | DONE | `prompt_prefix_byte_stable_env_trails`, `prompt_family_switch`. |
| I: recorded environment tail date/storage fields | MISSING | `env_block` currently emits the workspace root only. |
| I: provider cache validation | DONE | Historical cache/benchmark evidence retained in BENCHMARKS.md and VERIFY.md; not rerun live in Stage 4. New bash-benchmark cache values remain unmeasured. |
| J: host/mock tool, permission, storage, recovery tests | DONE | Named suites above plus the Stage 4 J table below. Sandbox-blocked process tests are explicitly separated. |
| K: edit benchmark implementation and recorded provider study | DONE | `core/bench/src/bin/edit_bench.rs`, `edit_bench_tasks.rs`; historical BENCHMARKS.md results. No new live edit run claimed. |

## M3 and Stage 2 recorded criteria

| Criterion | Status | Commit or test evidence / remaining gap |
|---|---|---|
| A: nested-exec/argv0/shebang/process-group spike | DONE | ADR 0014; dated emulator matrix and later physical-phone `scripts/exec-matrix.ps1` evidence in VERIFY.md. |
| B: source-built pinned BusyBox/rg/git and native packaging | DONE | Existing build gates; `ef5901c` curates source-built BusyBox; dated artifact/alignment evidence in VERIFY.md. No binaries added. |
| Android native discovery, farm refresh and self-test failure injection | DONE | Historical userland tests/device harness in VERIFY.md Stage 1; stale-link repair documented there. |
| Current packaged farm/self-test on both device ABIs | PENDING-DEVICE | Builds cover both ABIs; current Android self-test rerun and ABI behavior remain manual. |
| C/D: HostExecutor and NativeProcExecutor contract, fresh shell, cwd/stdin | DONE | `echo_exit_zero_and_output`, `stdin_is_closed_immediate_eof`, `exit_code_passes_through`; native historical matrix evidence. |
| D: merged stdout/stderr, bounded capture, full bounded spill/drain | DONE | `stdout_and_stderr_merge`, `head_tail_capture_and_spill`, `output_cap_bounds_sink_capture_and_spill_and_drains_to_exit`; Stage 4 partial exec run passed these before timeout. |
| D: allowlisted child env and sanitized git helpers/config | DONE | `env_is_not_inherited_from_the_parent`, `env_sanitization_rejects_provider_and_external_diff_overrides`, `git_overrides_and_credentials_cannot_be_injected`. Built-in git diffs require `--no-ext-diff --no-textconv`; reference solution tests exercise this. |
| D: timeout/cancel kills grandchildren and survives closed pipes/setsid | MISSING | Tests exist, with historical device proof; current host acceptance is UNRUN after the 60-second cap, cancellation failure and grandchild-timeout stall. Must rerun outside this sandbox. |
| D: restart orphan identity validation and discovery | DONE | `restart_sweep_finds_nested_pidfiles_without_touching_other_files`, `sweep_rejects_reused_pid_and_wrong_boot_and_consumes_legacy_records`; historical device evidence. |
| D: real process orphan sweep after restart, current app | PENDING-DEVICE | Linux/Android `restart_orphan_sweep_reaps_the_group` now exercises `sweep_spill`; not executable as a Linux process check on this Windows host. Full app kill/restart remains manual. |
| D: bash real tool-loop integration and batched live output | DONE | `bash_tool_runs_and_streams_output`, `bash_tap_flushes_on_timer_and_on_exit`; `ToolOutput` batching at size/time boundaries. |
| F: conservative shell parsing and adversarial permissions | DONE | `explicit_flag_grammars_reject_bypasses`, `generated_dangerous_arguments_never_allow`, `obfuscation_attempts_never_allow`, `canonical_paths_secrets_flag_values_and_literal_expansions`, symlink and substitution suites. |
| F: exact bash approval grants; project trust | DONE | `bash_approvals_are_exact_match_not_globs`; `22aaa75` and permission tests. |
| 2.1: trusted-only, default-off per-session unattended toggle/reset | DONE | `unattended_bash_is_trusted_opt_in_and_not_persisted`; `22aaa75`. |
| 2.1: network/outside/secret/root-rm/metadata/parse hard asks | DONE | `unattended_hard_asks_override_session_and_project_grants`; outside-root Deny remains Deny. Unknown, interpreter and recursive effects retain prompts. |
| 2.1: visible toggle/banner and reset UX | PENDING-DEVICE | Implementation in `22aaa75`; current interaction remains manual. |
| 2.2: typed foreground service/ADR, Stop, active-only lifetime/wake lock | DONE | `15e82b5`, `24f61fb`; `RunTrackerTest` covers submission/Stop state; Android build/lint evidence. |
| 2.2: notification permission and once-only battery explanation flow | PENDING-DEVICE | Implemented in `15e82b5`; permission prompts and battery explanation need maintainer observation. |
| 2.2: screen-off, Doze/app standby and Android service timeout behavior | PENDING-DEVICE | No new runtime claim. Maintainer long-run/Stop/wake-lock checks remain pending. |
| 2.3: background approval notification and same event protocol | DONE | `233ec2e`; `actionsUseTheInAppPermissionProtocol`, `malformedAndStaleActionsFailClosed`, `notificationActionsCannotCrossSessionsOrProcessNonces`. |
| 2.3: Approve once / Always for session / Deny and session deep link | PENDING-DEVICE | Receiver/protocol built; actual delivery, action handling and navigation require device checks. Pure JVM tests do not establish Android PendingIntent behavior. |
| 2.4: four-child concurrency cap and queue | DONE | `d3f42bd`; limiter unit test; conservative serial bash exchanges remain serial. |
| 2.4: phantom-process-killer troubleshooting entry | DONE | docs/TROUBLESHOOTING.md includes text adb testing instructions; no global device settings changed here. |
| 2.5: capped/virtualized live bash output, exit/signal, duration, Kill | PENDING-DEVICE | `efac398`; output/cancellation-state tests exist. Actual live output scrolling, pause/follow, chips and Kill remain manual. Kill currently stops the whole turn and clears cached grants. |
| 2.5: Settings diagnostics/self-test rerun | PENDING-DEVICE | Implemented in `efac398`; runtime status/button remains manual. |
| 2.5: rehydrate read/write/edit/bash cards | DONE | `persisted_cards_preserve_all_tool_kinds_and_bound_unicode_output`, `interrupted_card_does_not_present_stale_output_as_success`; `efac398`. |
| H / 2.6: curated BusyBox applet build and prompt catalog | DONE | `ef5901c`: 125-app allowlist, generated catalog, recorded token estimate and APK before/after evidence in VERIFY.md Stage 2. |
| H: static catalog, sh-script convention and prompt cache prefix | DONE | `prompt_prefix_byte_stable_env_trails`; ADRs 0015/0024/0029. Catalog now documents safe built-in git diff flags. |
| 2.7: release-derived isolated debug-signed bench; no release components | DONE | `ffb56cf`, `5b13f3d`; bench unit tests, merged-manifest/DEX inspection and Stage 3 instrumented device execution. |
| 2.7: exact shell am-start command to a non-exported activity | MISSING | Android forbids direct external launch. Bench-only instrumentation is the measured replacement (ADR 0028); run-as proposal remains unverified. |
| 3.1: bounded single SQLite writer and batched commits | DONE | `155f290`; writer FIFO, rollback, backpressure, reopen and FULL/WAL pragma tests; ADR 0026. |
| 3.1: event order and crash semantics through persistence failure | DONE | `persistence_failure_prevents_tool_execution`, store recovery tests; full process-kill acceptance remains in the blocked/manual rows above. |
| 3.2: persist/build/serialize/connect/dispatch alog spans | DONE | `155f290`, `5b13f3d`; numeric Stage 3 results. Connect includes response-header wait; dispatch ends after serialization. |
| 3.3: scripted mock over reverse, required 50/200-turn budget | DONE | `5b13f3d`; BENCHMARKS.md commands and raw JSON. Both p95 results below budget; no optimization iteration needed. |
| 3.3: paced UI jank with tool cards | MISSING | Measured current/legacy gfxinfo counters disagree (both preserved). Read-card workload does not establish live bash scrolling/jank acceptance. |
| 3.4: exact measurement commands, small raw JSON and corrected VERIFY rows | DONE | `5b13f3d`; docs/results/stage3-*.json and BENCHMARKS.md. Manual rows remain pending. |
| J: named host/process/permissions/recovery/notification tests | DONE | Inventory below; test presence does not waive listed execution gaps. |
| K / 4.1: deterministic bash benchmark, paired modes and metrics | DONE | `dd01e08`: `bash_bench` has 15 tasks, reference/negative verifiers, numeric traces and request metrics; ten offline benchmark tests. docs/BASH_BENCH.md and ADR 0029. |
| K / 4.1: provider pass rate, stability/cache/ASKS qualification | MISSING | Intentionally not run live. Maintainer supplies endpoint/model through host environment and pastes both summary lines. |

## J named-test inventory

| Gap | Named test(s) | Current execution evidence |
|---|---|---|
| Host executor | `echo_exit_zero_and_output`, `exit_code_passes_through`, `stdin_is_closed_immediate_eof`, `stdout_and_stderr_merge` | Passed before Stage 4 exec suite stalled. |
| Grandchild kill | `timeout_kills_tree_including_grandchildren`; Linux/Android `successful_leader_exit_kills_closed_pipe_and_setsid_children` | UNRUN for current acceptance: cap reached in timeout case. Historical device evidence remains dated in VERIFY.md. |
| Cap/spill/drain | `head_tail_capture_and_spill`, `output_cap_bounds_sink_capture_and_spill_and_drains_to_exit` | Passed before exec cap. |
| Environment sanitation | `env_is_not_inherited_from_the_parent`, new `env_sanitization_rejects_provider_and_external_diff_overrides` | Passed before exec cap; no provider configuration printed. |
| Adversarial permissions | `explicit_flag_grammars_reject_bypasses`, `generated_dangerous_arguments_never_allow`, `unattended_hard_asks_override_session_and_project_grants` | Host per-crate/workspace suites; current gate status in VERIFY.md. |
| Interrupted bash resume | New `interrupted_bash_resume_never_replays_the_persisted_command`; historical real-process crash test | New deterministic store/mock test passes; real-process sandbox acceptance still blocked. |
| Restart orphan sweep | New `restart_sweep_finds_nested_pidfiles_without_touching_other_files`; strengthened Linux/Android `restart_orphan_sweep_reaps_the_group` | Discovery/idempotence test passed; real process case needs Linux/Android rerun. |
| Notification protocol | New `notificationActionsCannotCrossSessionsOrProcessNonces` plus existing actions/stale tests | JVM gate recorded in VERIFY.md; device delivery/action/navigation pending. |
| Request statistics | New `request_metrics_report_mock_usage_without_wire_content` | Local mock test passes; no live provider used. |

Excluded by explicit scope: M4 features, HTTPS git, iOS, history scrub,
vendor drop and F-Droid scanner work. No push, remote/settings change or
live gateway run was performed. Existing unrelated untracked files were
left untouched.
