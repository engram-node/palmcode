# VLLM — the provider interface (v1)

v1 supports **vLLM serving only** (any deployment speaking vLLM's OpenAI
API: a hosted gateway or a self-hosted `vllm serve`). No other provider
code, fixtures, or docs exist in this tree. `core/llm` keeps a generic
HTTP+SSE transport, but the only adapter shipped is vLLM's.

## Endpoints used

| Endpoint | Purpose |
|---|---|
| `GET {base}/models` | capability sniff: model ids, **`max_model_len`** → context window |
| `POST {base}/chat/completions` (SSE) | the agent loop |
| `POST {base}/tokenize` | prompt token counts (`{model, prompt}` → `count`) |

Base URL + API key come from app settings; the key is Keystore-held and
never logged (AGENTS.md security rules).

## Server flags the app depends on

Started with defaults, a vLLM server **rejects** the requests an agent
needs. The probe (`scripts/probe-vllm.ps1`) detects each case and reports an
actionable fix:

- **Tool calling**: `tool_choice: "auto"` returns HTTP 400 unless the server
  runs `--enable-auto-tool-choice --tool-call-parser <parser>`. The 400 body
  says so; the probe maps the error text to this exact remediation. Parser
  names are recorded per model in `traces/*/metadata.json` (e.g. glm-family
  models → parser `glm45`, llama → `llama3_json`).
- **Reasoning fields**: reasoning-capable models emit chain-of-thought in a
  separate delta field — `reasoning_content` (majority) or `reasoning`
  (some vLLM reasoning parsers). The adapter accepts both as *hidden
  thinking*: never shown as the answer, never re-sent, but it **consumes
  the completion budget** (see below).
- **`continuous_usage_stats`**: when the server sets
  `--enable-continuous-usage-stats`, every SSE chunk carries `usage`. Probe
  detects; the UI shows live rates from stream timing regardless.

## Request shape (hard rules)

1. **No sampling params by default.** No `temperature`, `top_p`, `top_k`,
   `repetition_penalty`, `seed` — server/model defaults apply. The only
   request fields the client sends: `model`, `messages`, `stream: true`,
   `max_tokens` (cap), `tools`/`tool_choice` when calling, and
   `chat_template_kwargs` inside `extra_body` **if the user configures them
   per model role** (e.g. `{"enable_thinking": false}` on the fast role).
2. **Model roles**: `main` (default `glm-latest`) for the agent loop,
   `fast` (default `glm-5.3-flash`) for snippets/diffs/titles. Both
   overridable in settings.
3. **Reasoning models need headroom.** `max_tokens` < ~512 produces
   `finish_reason: "length"` with empty visible content (all budget burned
   in `reasoning_content`). Defaults: main ≥ 4096, fast ≥ 1024.
4. **Tools go out OpenAI-nested** — `{"type":"function","function":{name,
   description, parameters}}`. The tools crate keeps a provider-neutral
   flat form (`input_schema`) and the builder translates (M2; sending the
   flat form is a **HTTP 400** behind litellm:
   `('body','tools',0,'function') Field required` — observed on device
   2026-10-12). `tool_choice: "auto"` accompanies non-empty tools.
5. **Tool-call frames (GLM parser `glm45`)**: streamed as
   `delta.tool_calls[]` chunks — index-keyed, `id`+`function.name` on the
   first chunk of a call, argument JSON appended in later
   `function.arguments` deltas. Parallel calls interleave by `index`; one
   trace lacked `index` on later calls (accumulator falls back to the open
   call — `accumulator::tests::indexless_id_fallback`).

## Token accounting

- Prompt size: `POST /tokenize` first; if the request fails in any way
  (404/501 upstream-missing, 403 on gateways that don't route it — observed
  on the test gateway), fall back to a **chars/3.5** heuristic and mark the
  count as estimated.
- Live metrics shown in the agent UI: **TTFT** (request → first content
  byte), **tokens/s** (completion tokens from usage, or chunk count, over
  stream duration), and **prefix-cache hit rate** —
  `usage.prompt_tokens_details.cached_tokens / prompt_tokens` when the
  server exposes it (vLLM ≥ 0.6 with APC enabled).

## Prefix stability (why and how)

vLLM prefix caching keys on the *byte prefix* of the rendered prompt.
The cache either works or the whole prompt re-prefills (~seconds on long
sessions). Rules for the agent:

1. The message list is **append-only** during a turn; nothing mutates
   earlier messages (no inline edits, no per-turn compaction).
2. The prefix (system prompt + tool definitions + history) is **byte
   stable**: fixed tool order, deterministic JSON key order everywhere
   (no hashmap serialization), identical whitespace.
3. Compaction happens **in batches at a threshold** (default 80% of
   `max_model_len`), never per turn; each compaction logs the TTFT cost
   of the next request so cache behavior is observable.
4. `cache_control` / Anthropic-style explicit cache points: out of scope.

## Timeouts & aborts

| Timer | Default | Configurable |
|---|---|---|
| Connect | 10 s | no |
| First token (request → first SSE byte) | 120 s | yes |
| Idle stall (gap between SSE bytes) | 60 s | yes |
| Total stream | none | yes |

Any timeout or user abort must **close the HTTP connection** (not just
stop reading); M1 acceptance verifies against a mock server that the
server side observes the client disconnect.

## Fixtures & the parser contract

`traces/` (git-ignored) / committed fixtures under `core/llm/fixtures/vllm/`
recorded by `scripts/trace-sse.ps1` against the real endpoint. Cases:
`simple`, `reasoning`, `tool_call`, `parallel_tools`,
`tool_result_followup`, `long_prefill`, and `error` (400 response shape)
when reproducible. Every fixture directory carries `metadata.json`:
model ids, server version fields, tool parser name, capture date.

M1 acceptance for the SSE parser:

- **Re-chunk property test**: replay each fixture with the byte stream
  re-chunked at *random* boundaries (seeded, k trials); the parsed event
  sequence must equal the canonical parse.
- Mid-chunk JSON splits, CRLF/LF mixes, `data: [DONE]`, comment lines
  (`:` keepalives) all covered by fixtures or unit tests.

M2 notes on the fixture set:

- `tool_call` / `parallel_tools` / `tool_result_followup` are real Grid
  GLM captures; their in-fixture tool names (`read_file`, …) predate the
  M2 tool rename — the tool-loop tests **rewrite the names in memory at
  replay**; the bytes on disk stay untouched.
- **No fixture carries `usage`** (Grid ignores `include_usage`; usage
  arrives only in the final chunk's `usage` when the server adds it, and
  Grid's tool streams show none). Tests that need usage (`note_usage_
  sample`, EMA token accounting) build synthetic usage chunks; do not
  trust real-traffic fixtures for usage-dependent assertions.

## Probe

`scripts/probe-vllm.ps1 -BaseUrl <url>` performs all detections above and
prints a pasteable table (models + max_model_len, tool_choice status +
remediation, reasoning field name, /tokenize status, usage-stats flavor,
TTFT/tok-s sample). Run it before filing endpoint bugs.

## Observed behavior — Grid gateway (`https://grid.ai.juspay.net/v1`)

Verified end-to-end **from the app on a physical phone** (motorola edge 50
neo, Android 16, debug build, in-app probe 2026-10-11):

| Detection | Result |
|---|---|
| `/models` | 200, 34 models; `max_model_len` **absent on every entry** (context window falls back to the stored default 128k) |
| `/tokenize` | **HTTP 403** → heuristic chars/3.5 counting is the production path here |
| `tool_choice: "auto"` | accepted (no `--enable-auto-tool-choice` remediation needed) |
| Reasoning | streamed via `reasoning_content` |
| `stream_options.include_usage` | **ignored** → final-chunk usage only; continuous tick disabled |
| Cache counters | `cached_tokens` present in final usage |
| Sample chat | TTFT **410 ms**, ~51 tok/s (probe), full turn ttft 2142 ms on first message |

Consequences baked into the runtime: capability flags prefill from the probe
but stay user-overridable; token counts for context budgeting come from the
heuristic; per-turn usage arrives only at `finish`.
