//! Live-provider benchmark entry. Tests/listing never configure a provider.
#[path = "../bash_tasks.rs"]
mod tasks;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use palmcode_agent::{ApprovalScope, Engine, Event, EventSink, ProviderCfg, RequestMetrics};
use palmcode_exec::{EnvSpec, Executor, HostExecutor};
use serde_json::{Value, json};

#[derive(Default, Clone)]
struct State {
    pending: Vec<u64>,
    done: Option<bool>,
    asks: usize,
    tools: usize,
    bash: usize,
    errors: usize,
    timed_out: bool,
    requests: Vec<RequestMetrics>,
}

#[derive(Default)]
struct Sink(Mutex<State>);
impl EventSink for Sink {
    fn on_batch(&self, events: Vec<Event>) {
        let mut state = self.0.lock().unwrap();
        for event in events {
            match event {
                Event::ApprovalRequested { request_id, .. } => {
                    state.asks += 1;
                    state.pending.push(request_id);
                }
                Event::ToolStarted { name, .. } => {
                    state.tools += 1;
                    state.bash += usize::from(name == "bash");
                }
                Event::ToolFinished { ok, preview, .. } => {
                    state.errors += usize::from(!ok);
                    state.timed_out |= preview.contains("[timed out after ");
                }
                Event::MessageFinished { finish_reason, .. } => {
                    state.done = Some(finish_reason == "stop")
                }
                Event::Interrupted { .. } | Event::TurnError { .. } => state.done = Some(false),
                _ => {}
            }
        }
    }
    fn on_request_metrics(&self, metrics: RequestMetrics) {
        self.0.lock().unwrap().requests.push(metrics);
    }
}

#[derive(Default)]
struct Args {
    repeats: usize,
    model: Option<String>,
    trace: Option<PathBuf>,
    task: Option<String>,
    list: bool,
}

fn parse(args: impl IntoIterator<Item = String>) -> Result<Args, &'static str> {
    let mut result = Args {
        repeats: 1,
        ..Default::default()
    };
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--list" => result.list = true,
            "--repeats" => {
                result.repeats = args
                    .next()
                    .ok_or("missing repeats")?
                    .parse()
                    .map_err(|_| "invalid repeats")?
            }
            "--model" => result.model = Some(args.next().ok_or("missing model")?),
            "--trace-dir" => {
                result.trace = Some(args.next().ok_or("missing trace directory")?.into())
            }
            "--task" => result.task = Some(args.next().ok_or("missing task")?),
            _ => return Err("unknown flag"),
        }
    }
    if !(1..=100).contains(&result.repeats) {
        return Err("repeats must be 1..100");
    }
    if result
        .task
        .as_ref()
        .is_some_and(|id| !tasks::TASKS.iter().any(|t| t.id == id))
    {
        return Err("unknown task");
    }
    Ok(result)
}

fn verify(task: &tasks::Task, root: &Path) -> bool {
    let protected: &[&str] = match task.id {
        "sed-log" => &["access.log"],
        "grep-change" => &["verify.sh"],
        "tar-roundtrip" => &["input/a.txt", "input/sub/b.txt"],
        _ => &[],
    };
    for (path, original) in task
        .files
        .iter()
        .filter(|(path, _)| protected.contains(path))
    {
        if std::fs::read_to_string(root.join(path)).ok().as_deref() != Some(*original) {
            return false;
        }
    }
    let executor = HostExecutor::new();
    let env = EnvSpec {
        path: std::env::var("PATH").unwrap_or_default(),
        home: root.into(),
        tmpdir: root.into(),
        extra: Default::default(),
    };
    let mut spec = executor.spec(task.verify, root, env);
    spec.timeout = Duration::from_secs(10);
    executor
        .run(&spec, &mut |_: &[u8]| {})
        .is_ok_and(|r| r.exit == Some(0) && !r.timed_out)
}

fn request_json(m: &RequestMetrics) -> Value {
    let decode_ms = m.ttft_ms.map(|ttft| m.elapsed_ms.saturating_sub(ttft));
    json!({"ok":m.ok,"prompt_tokens":m.prompt_tokens,"completion_tokens":m.completion_tokens,
        "cached_tokens":m.cached_tokens,"ttft_ms":m.ttft_ms,"wall_ms":m.elapsed_ms,"retries":m.retries,
        "decode_tps":decode_ms.filter(|ms| *ms >= 250).zip(m.completion_tokens).map(|(ms,t)| f64::from(t)*1000.0/ms as f64)})
}

fn numeric_report(state: &State) -> Value {
    let prompt: Option<u64> = state
        .requests
        .iter()
        .map(|r| r.prompt_tokens.map(u64::from))
        .sum();
    let completion: Option<u64> = state
        .requests
        .iter()
        .map(|r| r.completion_tokens.map(u64::from))
        .sum();
    let cached: Option<u64> = state
        .requests
        .iter()
        .map(|r| r.cached_tokens.map(u64::from))
        .sum();
    let cache = cached
        .zip(prompt)
        .filter(|(c, p)| *p > 0 && c <= p)
        .map(|(c, p)| c as f64 / p as f64);
    json!({"turns":state.requests.len(),"tool_calls":state.tools,"bash_calls":state.bash,"tool_errors":state.errors,
        "tool_error_rate":if state.tools == 0 {None} else {Some(state.errors as f64/state.tools as f64)},
        "asks":state.asks,"prompt_tokens":prompt,"completion_tokens":completion,"cache_hit_rate":cache,
        "requests":state.requests.iter().map(request_json).collect::<Vec<_>>()})
}

fn protocol_pass(task: &tasks::Task, state: &State) -> bool {
    state.done == Some(true)
        && state.bash > 0
        && (!task.require_error || state.errors > 0)
        && (!task.require_timeout || state.timed_out)
}

fn write_trace(
    dir: &Path,
    task: &str,
    repeat: usize,
    auto: bool,
    value: &Value,
) -> Result<(), &'static str> {
    std::fs::create_dir_all(dir).map_err(|_| "trace directory failed")?;
    let file = dir.join(format!("{task}-r{repeat}-auto{auto}.json"));
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(file)
        .map_err(|_| "trace already exists or cannot be created")?;
    writeln!(out, "{value}").map_err(|_| "trace write failed")
}

fn run(args: Args) -> Result<(), &'static str> {
    if args.list {
        for task in tasks::TASKS {
            println!("{}", task.id);
        }
        return Ok(());
    }
    let base =
        std::env::var("PALMCODE_LLM_BASE_URL").map_err(|_| "PALMCODE_LLM_BASE_URL required")?;
    let key = zeroize::Zeroizing::new(
        std::env::var("PALMCODE_LLM_API_KEY").map_err(|_| "PALMCODE_LLM_API_KEY required")?,
    );
    let model = args
        .model
        .or_else(|| std::env::var("PALMCODE_LLM_MODEL").ok())
        .filter(|v| !v.is_empty())
        .ok_or("model required via --model or PALMCODE_LLM_MODEL")?;
    let mut results = Vec::new();
    for task in tasks::TASKS
        .iter()
        .filter(|t| args.task.as_ref().is_none_or(|id| id == t.id))
    {
        for repeat in 1..=args.repeats {
            // Alternate order to reduce the systematic benefit of a warm provider cache.
            for auto in if repeat % 2 == 1 {
                [false, true]
            } else {
                [true, false]
            } {
                let temp = tempfile::tempdir().map_err(|_| "temporary directory failed")?;
                let root = temp.path().join("workspace");
                tasks::seed(task, &root).map_err(|_| "fixture setup failed")?;
                let sink = Arc::new(Sink::default());
                let engine = Engine::start(&temp.path().join("bench.db"), sink.clone())
                    .map_err(|_| "engine start failed")?;
                engine
                    .configure(ProviderCfg {
                        base_url: base.clone(),
                        api_key: key.to_string(),
                        main_model: model.clone(),
                        fast_model: model.clone(),
                        context_window: 32768,
                        max_output_tokens: 4096,
                        sampling_json: Some("{\"temperature\":0}".into()),
                        extra_body_json: None,
                        continuous_usage: false,
                        first_token_secs: 60,
                        idle_secs: 60,
                    })
                    .map_err(|_| "provider configuration rejected")?;
                let root_text = root.to_str().ok_or("non-UTF8 workspace")?;
                let workspace = engine
                    .workspace_register("Bash benchmark", root_text)
                    .map_err(|_| "workspace registration failed")?;
                engine
                    .workspace_set_trusted(&workspace.id, true)
                    .map_err(|_| "workspace trust failed")?;
                engine
                    .set_workspace(root_text)
                    .map_err(|_| "workspace selection failed")?;
                let session = engine
                    .create_session(task.id)
                    .map_err(|_| "session creation failed")?;
                if auto && !engine.set_unattended_bash(&session.id, true) {
                    return Err("auto-approve unavailable");
                }
                let start = Instant::now();
                let prompt = format!(
                    "Use the bash tool to complete this task. Work only in this workspace. {}",
                    task.prompt
                );
                engine
                    .send(&session.id, &prompt)
                    .map_err(|_| "submission failed")?;
                loop {
                    let (pending, done, turns) = {
                        let mut state = sink.0.lock().unwrap();
                        (
                            std::mem::take(&mut state.pending),
                            state.done,
                            state.requests.len(),
                        )
                    };
                    for id in pending {
                        engine.resolve_approval(id, true, ApprovalScope::Once);
                    }
                    if done.is_some() {
                        break;
                    }
                    if start.elapsed() > Duration::from_secs(180) || turns >= 40 {
                        let _ = engine.stop(&session.id);
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                let elapsed = start.elapsed().as_millis();
                engine.shutdown();
                drop(engine);
                let state = sink.0.lock().unwrap().clone();
                let pass = protocol_pass(task, &state) && verify(task, &root);
                let mut report = numeric_report(&state);
                report["task"] = json!(task.id);
                report["repeat"] = json!(repeat);
                report["auto_approve"] = json!(auto);
                report["pass"] = json!(pass);
                report["wall_ms"] = json!(elapsed);
                if let Some(dir) = &args.trace {
                    write_trace(dir, task.id, repeat, auto, &report)?;
                }
                println!("{report}");
                results.push(report);
            }
        }
    }
    for auto in [false, true] {
        let rows: Vec<_> = results
            .iter()
            .filter(|r| r["auto_approve"] == auto)
            .collect();
        let mut rates = Vec::new();
        for rep in 1..=args.repeats {
            let subset: Vec<_> = rows.iter().filter(|r| r["repeat"] == rep).collect();
            rates.push(
                subset.iter().filter(|r| r["pass"] == true).count() as f64 / subset.len() as f64,
            );
        }
        let mean = rates.iter().sum::<f64>() / rates.len() as f64;
        let stddev =
            (rates.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / rates.len() as f64).sqrt();
        println!(
            "{}",
            json!({"summary":true,"auto_approve":auto,"runs":rows.len(),"pass_rate":mean,
            "repeat_pass_rates":rates,"spread_population_stddev":stddev,
            "metrics":summary_metrics(&rows),
            "per_task":tasks::TASKS.iter().filter_map(|task| {
                let task_rows:Vec<_> = rows.iter().filter(|r| r["task"] == task.id).collect();
                (!task_rows.is_empty()).then(|| json!({"task":task.id,"passes":task_rows.iter().filter(|r|r["pass"] == true).count(),"runs":task_rows.len(),
                    "asks":task_rows.iter().map(|r|r["asks"].clone()).collect::<Vec<_>>() }))
            }).collect::<Vec<_>>() })
        );
    }
    Ok(())
}

fn distribution(mut values: Vec<f64>) -> Value {
    values.retain(|v| v.is_finite());
    values.sort_by(f64::total_cmp);
    if values.is_empty() {
        return Value::Null;
    }
    let percentile = |p: f64| values[((values.len() as f64 * p).ceil() as usize).saturating_sub(1)];
    json!({"observations":values.len(),"min":values[0],"max":values[values.len()-1],
        "mean":values.iter().sum::<f64>()/values.len() as f64,"p50":percentile(0.5),"p95":percentile(0.95)})
}

fn summary_metrics(rows: &[&Value]) -> Value {
    let mut result = json!({});
    for name in [
        "turns",
        "tool_calls",
        "tool_errors",
        "tool_error_rate",
        "asks",
        "prompt_tokens",
        "completion_tokens",
        "wall_ms",
        "cache_hit_rate",
    ] {
        result[name] = distribution(rows.iter().filter_map(|r| r[name].as_f64()).collect());
    }
    for name in ["ttft_ms", "decode_tps"] {
        result[name] = distribution(
            rows.iter()
                .flat_map(|r| r["requests"].as_array().into_iter().flatten())
                .filter_map(|r| r[name].as_f64())
                .collect(),
        );
    }
    result
}

fn main() {
    if let Err(error) = parse(std::env::args().skip(1)).and_then(run) {
        eprintln!("bash_bench: {error}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_requires_observed_failure_and_timeout_then_success() {
        let task = tasks::TASKS
            .iter()
            .find(|t| t.id == "timeout-recovery")
            .unwrap();
        let mut state = State {
            done: Some(true),
            bash: 1,
            ..Default::default()
        };
        assert!(!protocol_pass(task, &state));
        state.errors = 1;
        assert!(!protocol_pass(task, &state));
        state.timed_out = true;
        assert!(protocol_pass(task, &state));
        state.done = Some(false);
        assert!(!protocol_pass(task, &state));
    }
    #[test]
    fn all_reference_solutions_pass_their_verifiers() {
        let solutions = [
            (
                "fix-shell-test",
                r#"printf "printf 'hello\\n'\n" > greet.sh"#,
            ),
            ("awk-log", "printf 'alice\t17\nbob\t3\n' > totals.tsv"),
            (
                "sed-log",
                "sed 's/token=[^ ]*/token=REDACTED/g' access.log > clean.log",
            ),
            (
                "find-xargs-rename",
                r#"find data -name '*.old' -print0 | xargs -0 -I{} sh -c 'mv "$1" "${1%.old}.txt"' sh '{}'"#,
            ),
            (
                "create-script",
                r#"printf '%s\n' 'total=0' 'for arg; do total=$((total + arg)); done' 'echo "$total"' > sum.sh; sh sum.sh 3 4 5 > result.txt"#,
            ),
            (
                "local-git",
                "git init -q && git config user.name Bench && git config user.email bench@example.invalid && git add note.txt && git commit -qm first && git checkout -qb work && printf 'new\n' > note.txt && git add note.txt && git commit -qm second && git diff --no-ext-diff --no-textconv HEAD~1 HEAD -- note.txt > change.diff",
            ),
            (
                "grep-change",
                "sed -i 's/OLD_PORT/NEW_PORT/g' a.conf nested/b.conf",
            ),
            ("text-transform", "printf 'alice\nbob\n' > normalized.txt"),
            (
                "restructure",
                "mv src archive && printf 'a.txt\nsub/b.txt\n' > manifest.txt",
            ),
            ("timeout-recovery", "printf recovered > recovered.txt"),
            (
                "failure-recovery",
                r#"printf "printf 'ready\\n'\n" > work.sh; sh work.sh > result.txt"#,
            ),
            (
                "join-data",
                "printf 'alice\t9\nbob\t7\n' > scores-by-name.tsv",
            ),
            (
                "tar-roundtrip",
                "tar -cf bundle.tar input && mkdir restored && tar -xf bundle.tar -C restored",
            ),
            ("count-errors", "printf 'E1\t1\nE2\t2\n' > errors.tsv"),
            (
                "idempotent-script",
                r#"printf '%s\n' 'grep -qx enabled=true settings.conf || printf "enabled=true\n" >> settings.conf' > ensure.sh; sh ensure.sh; sh ensure.sh"#,
            ),
        ];
        assert_eq!(solutions.len(), tasks::TASKS.len());
        for (id, solution) in solutions {
            let task = tasks::TASKS.iter().find(|t| t.id == id).unwrap();
            let dir = tempfile::tempdir().unwrap();
            tasks::seed(task, dir.path()).unwrap();
            let executor = HostExecutor::new();
            let env = EnvSpec {
                path: std::env::var("PATH").unwrap_or_default(),
                home: dir.path().into(),
                tmpdir: dir.path().into(),
                extra: Default::default(),
            };
            let mut spec = executor.spec(solution, dir.path(), env);
            spec.timeout = Duration::from_secs(10);
            let result = executor.run(&spec, &mut |_: &[u8]| {}).unwrap();
            assert_eq!(
                result.exit,
                Some(0),
                "{id} reference setup failed: {}",
                result.captured.render()
            );
            assert!(verify(task, dir.path()), "{id} rejected reference solution");
        }
    }
    #[test]
    fn summary_percentiles_exclude_missing_observations() {
        assert!(distribution(vec![]).is_null());
        let d = distribution(vec![4.0, 1.0, 3.0, 2.0]);
        assert_eq!(d["p50"], 2.0);
        assert_eq!(d["p95"], 4.0);
        let row = json!({"turns":2,"requests":[{"ttft_ms":12},{"ttft_ms":null}]});
        assert_eq!(summary_metrics(&[&row])["ttft_ms"]["observations"], 1);
    }
    #[test]
    fn verifiers_reject_all_unsolved_fixtures() {
        for task in tasks::TASKS {
            let dir = tempfile::tempdir().unwrap();
            tasks::seed(task, dir.path()).unwrap();
            assert!(
                !verify(task, dir.path()),
                "{} accepted unsolved fixture",
                task.id
            );
        }
    }
    #[test]
    fn verifier_accepts_correct_transform_and_rejects_partial_output() {
        let task = tasks::TASKS
            .iter()
            .find(|t| t.id == "text-transform")
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        tasks::seed(task, dir.path()).unwrap();
        std::fs::write(dir.path().join("normalized.txt"), "alice\nbob\n").unwrap();
        assert!(verify(task, dir.path()));
        std::fs::write(dir.path().join("normalized.txt"), "alice\n").unwrap();
        assert!(!verify(task, dir.path()));
    }
    #[test]
    fn shell_fix_verifier_rejects_replacing_the_test_with_success() {
        let task = &tasks::TASKS[0];
        let dir = tempfile::tempdir().unwrap();
        tasks::seed(task, dir.path()).unwrap();
        std::fs::write(dir.path().join("greet.sh"), "printf 'hello\\n'\n").unwrap();
        assert!(verify(task, dir.path()));
        std::fs::write(dir.path().join("test.sh"), "exit 0\n").unwrap();
        assert!(!verify(task, dir.path()));
    }
    #[test]
    fn flags_reject_invalid_runs_without_provider_access() {
        assert!(parse(["--repeats".into(), "0".into()]).is_err());
        assert!(parse(["--model".into()]).is_err());
        assert!(parse(["--task".into(), "unknown".into()]).is_err());
        assert_eq!(parse(["--repeats".into(), "3".into()]).unwrap().repeats, 3);
    }
    #[test]
    fn metrics_sum_requests_and_do_not_invent_cache_or_leak_text() {
        let state = State {
            requests: vec![
                RequestMetrics {
                    prompt_tokens: Some(10),
                    completion_tokens: Some(2),
                    cached_tokens: Some(5),
                    ..Default::default()
                },
                RequestMetrics {
                    prompt_tokens: Some(20),
                    completion_tokens: Some(3),
                    ..Default::default()
                },
            ],
            asks: 2,
            tools: 2,
            errors: 1,
            ..Default::default()
        };
        let value = numeric_report(&state);
        assert_eq!(value["prompt_tokens"], 30);
        assert_eq!(value["completion_tokens"], 5);
        assert!(value["cache_hit_rate"].is_null());
        assert_eq!(value["tool_error_rate"], 0.5);
        assert_eq!(value["asks"], 2);
        assert_eq!(value["turns"], 2);
    }
    #[test]
    fn numeric_trace_refuses_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let value = json!({"asks":1});
        write_trace(dir.path(), "task", 1, false, &value).unwrap();
        assert!(write_trace(dir.path(), "task", 1, false, &value).is_err());
    }
}
