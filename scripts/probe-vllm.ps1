# vLLM endpoint probe (M1 prep, item 3). Measures the real behaviors
# docs/VLLM.md depends on and prints a pasteable report.
#
#   $env:PALMCODE_LLM_API_KEY = "sk-..."   # optional (unauthenticated vLLM is fine)
#   powershell -ExecutionPolicy Bypass -File scripts/probe-vllm.ps1
#
# Checks: /models (max_model_len) - tool_choice=auto viability incl. the
# actionable 400 remediation - reasoning field name - /tokenize - usage
# flavor (continuous vs final-chunk) - TTFT + tokens/s with live stream
# timing, honoring connect/first-token/idle-stall timeouts.
#
# NB: no $ErrorActionPreference="Stop" (PS5.1 native-stderr footgun).
param(
    [string]$BaseUrl = "https://grid.ai.juspay.net/v1",
    [string]$MainModel = "glm-latest",
    [string]$FastModel = "glm-5.3-flash",
    [int]$FirstTokenTimeoutSec = 120,
    [int]$IdleStallTimeoutSec = 60,
    [int]$ConnectTimeoutSec = 10
)

Add-Type -AssemblyName System.Net.Http
try { [System.Net.ServicePointManager]::SecurityProtocol = [System.Net.SecurityProtocolType]::Tls12 } catch {}

$apiKey = $env:PALMCODE_LLM_API_KEY
if (-not $apiKey) { $apiKey = $env:GLM_API_KEY }

$rows = New-Object System.Collections.Generic.List[string]
function Add-Row([string]$k, [string]$v) { $rows.Add("| $k | $v |") }
function ErrMsg($e) { if ($e.Exception.InnerException) { return $e.Exception.InnerException.Message } return $e.Exception.Message }

function New-Client([double]$timeoutSec) {
    $c = [System.Net.Http.HttpClient]::new()
    $c.Timeout = [TimeSpan]::FromSeconds($timeoutSec)
    if ($script:apiKey) { $c.DefaultRequestHeaders.Authorization = "Bearer $script:apiKey" }
    return ,$c
}

function Post-Json([System.Net.Http.HttpClient]$client, [string]$url, [object]$body) {
    $json = $body | ConvertTo-Json -Depth 10 -Compress
    $content = [System.Net.Http.StringContent]::new($json, [Text.Encoding]::UTF8, "application/json")
    return $client.PostAsync($url, $content).GetAwaiter().GetResult()
}

# ---------- 1. /models ----------
$c = New-Client $ConnectTimeoutSec
try {
    $r = $c.GetAsync("$BaseUrl/models").GetAwaiter().GetResult()
    $models = ($r.Content.ReadAsStringAsync().GetAwaiter().GetResult() | ConvertFrom-Json).data
    $names = @($models | ForEach-Object { $_.id })
    Add-Row "/models" "$($names.Count) models: $($names -join ', ')"
    foreach ($m in $names) {
        $len = ($models | Where-Object { $_.id -eq $m }).max_model_len
        if ($len) { Add-Row "max_model_len $m" "$len" } else { Add-Row "max_model_len $m" "ABSENT" }
    }
} catch { Add-Row "/models" ("FAIL: " + (ErrMsg $_)) }
finally { $c.Dispose() }

# ---------- 2. /tokenize ----------
$c = New-Client $ConnectTimeoutSec
try {
    $r = Post-Json $c "$BaseUrl/tokenize" @{ model = $FastModel; prompt = "hello world from palmcode" }
    if ($r.IsSuccessStatusCode) {
        $t = $r.Content.ReadAsStringAsync().GetAwaiter().GetResult() | ConvertFrom-Json
        Add-Row "/tokenize" "OK; 'hello world from palmcode' = $($t.count) tokens"
    } else { Add-Row "/tokenize" "HTTP $([int]$r.StatusCode) - heuristic fallback (chars/3.5) engaged" }
} catch { Add-Row "/tokenize" (("FAIL: " + (ErrMsg $_)) + " - heuristic fallback engaged") }
finally { $c.Dispose() }

# ---------- 3. tool_choice=auto ----------
$c = New-Client ([Math]::Max(60, $FirstTokenTimeoutSec))
$toolDef = @{ type = "function"; function = @{ name = "read_file"; description = "Read a workspace file"; parameters = @{ type = "object"; properties = @{ path = @{ type = "string" } }; required = @("path") } } }
try {
    $r = Post-Json $c "$BaseUrl/chat/completions" @{
        model = $MainModel
        messages = @(@{ role = "user"; content = "Read the file src/main.rs using the read_file tool." })
        tools = @($toolDef); tool_choice = "auto"; max_tokens = 512; stream = $false
    }
    $body = $r.Content.ReadAsStringAsync().GetAwaiter().GetResult()
    if (-not $r.IsSuccessStatusCode) {
        $msg = $body; try { $msg = ($body | ConvertFrom-Json).error.message } catch {}
        if ($msg -match "tool") {
            $fix = "server needs --enable-auto-tool-choice --tool-call-parser <parser> (glm-family: glm45)"
        } else { $fix = "unknown; inspect body" }
        $msg = $msg -replace "\s+", " "
        Add-Row "tool_choice=auto" "HTTP $([int]$r.StatusCode) - $fix [server: '$msg']"
    } else {
        $j = $body | ConvertFrom-Json
        $calls = $j.choices[0].message.tool_calls
        if ($calls) { Add-Row "tool_choice=auto" "OK; $($calls.Count) call(s): $($calls[0].function.name)($($calls[0].function.arguments))" }
        else { Add-Row "tool_choice=auto" "200 but NO tool call emitted (finish=$($j.choices[0].finish_reason))" }
    }
} catch { Add-Row "tool_choice=auto" ("FAIL: " + (ErrMsg $_)) }
finally { $c.Dispose() }

# ---------- 4. streaming: TTFT, tok/s, reasoning field, usage flavor ----------
$c = [System.Net.Http.HttpClient]::new()
$c.Timeout = [TimeSpan]::FromMilliseconds([System.Threading.Timeout]::Infinite)
if ($apiKey) { $c.DefaultRequestHeaders.Authorization = "Bearer $apiKey" }
$sw = [Diagnostics.Stopwatch]::StartNew()
$firstByteMs = -1; $idleMax = 0; $chunks = 0; $reasonField = ""; $usageAll = $true; $usageAny = $false; $finish = ""; $completion = 0; $cachedTok = $null; $promptTok = 0
try {
    $req = [System.Net.Http.HttpRequestMessage]::new([System.Net.Http.HttpMethod]::Post, "$BaseUrl/chat/completions")
    $bodyJson = @{ model = $MainModel; messages = @(@{ role = "user"; content = "What is 17*23? Think briefly, then answer with just the number." }); max_tokens = 1024; stream = $true; stream_options = @{ include_usage = $true } } | ConvertTo-Json -Depth 8 -Compress
    $req.Content = [System.Net.Http.StringContent]::new($bodyJson, [Text.Encoding]::UTF8, "application/json")
    $ctsFirst = [System.Threading.CancellationTokenSource]::new($FirstTokenTimeoutSec * 1000)
    $resp = $c.SendAsync($req, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead, $ctsFirst.Token).GetAwaiter().GetResult()
    $connectMs = $sw.ElapsedMilliseconds
    if (-not $resp.IsSuccessStatusCode) { throw [Exception]::new("HTTP $([int]$resp.StatusCode): " + $resp.Content.ReadAsStringAsync().GetAwaiter().GetResult()) }
    $stream = $resp.Content.ReadAsStreamAsync().GetAwaiter().GetResult()
    $buf = New-Object byte[] 65536; $pending = ""
    $lastRead = $sw.Elapsed
    while ($true) {
        $ctsIdle = [System.Threading.CancellationTokenSource]::new($IdleStallTimeoutSec * 1000)
        if ($firstByteMs -lt 0) { $ctsIdle.CancelAfter($FirstTokenTimeoutSec * 1000) }
        $n = $stream.ReadAsync($buf, 0, $buf.Length, $ctsIdle.Token).GetAwaiter().GetResult()
        if ($n -eq 0) { break }
        $now = $sw.Elapsed
        if ($firstByteMs -lt 0) { $firstByteMs = [long]$now.TotalMilliseconds }
        $gap = [long]($now - $lastRead).TotalMilliseconds; if ($gap -gt $idleMax) { $idleMax = $gap }
        $lastRead = $now
        $pending += [Text.Encoding]::UTF8.GetString($buf, 0, $n)
        while (($i = $pending.IndexOf("`n")) -ge 0) {
            $line = $pending.Substring(0, $i).Trim(); $pending = $pending.Substring($i + 1)
            if (-not $line.StartsWith("data:")) { continue }
            $payload = $line.Substring(5).Trim()
            if ($payload -eq "[DONE]") { continue }
            $chunks++
            $ev = $null; try { $ev = $payload | ConvertFrom-Json } catch {}
            if (-not $ev) { continue }
            if ($ev.usage) {
                $usageAny = $true
                if ($ev.usage.completion_tokens) { $completion = $ev.usage.completion_tokens }
                if ($ev.usage.prompt_tokens) { $promptTok = $ev.usage.prompt_tokens }
                $det = $ev.usage.prompt_tokens_details
                if ($det -and $det.PSObject.Properties["cached_tokens"]) { $cachedTok = $det.cached_tokens }
            } else { $usageAll = $false }
            $d = $ev.choices[0].delta
            if ($d -and -not $reasonField) {
                if ($d.PSObject.Properties["reasoning_content"] -and $d.reasoning_content) { $reasonField = "reasoning_content" }
                elseif ($d.PSObject.Properties["reasoning"] -and $d.reasoning) { $reasonField = "reasoning" }
            }
            if ($ev.choices[0].finish_reason) { $finish = $ev.choices[0].finish_reason }
        }
    }
    $totalMs = $sw.ElapsedMilliseconds
    $stream.Dispose()
    $genMs = $totalMs - $firstByteMs
    if ($completion -gt 0 -and $genMs -gt 0) { $tps = [math]::Round($completion / ($genMs / 1000), 1) } else { $tps = "n/a" }
    Add-Row "connect" "$connectMs ms"
    Add-Row "TTFT" "$firstByteMs ms"
    Add-Row "stream" "$chunks chunks in $totalMs ms; max idle gap $idleMax ms; finish=$finish"
    Add-Row "tokens/s" "$tps (completion=$completion)"
    if ($reasonField) { Add-Row "reasoning field" $reasonField } else { Add-Row "reasoning field" "none observed" }
    if ($usageAll -and $usageAny) { Add-Row "usage flavor" "continuous (every chunk)" }
    elseif ($usageAny) { Add-Row "usage flavor" "final-chunk only (stream_options.include_usage)" }
    else { Add-Row "usage flavor" "none" }
    if ($cachedTok -ne $null) {
        $pct = if ($promptTok -gt 0) { [math]::Round(100.0 * $cachedTok / $promptTok, 1) } else { "?" }
        Add-Row "prefix-cache" "exposed: $cachedTok/$promptTok cached ($pct%)"
    } else { Add-Row "prefix-cache" "prompt_tokens_details.cached_tokens not exposed" }
} catch {
    Add-Row "streaming" ("FAIL: " + (ErrMsg $_))
} finally { $c.Dispose() }

# ---------- report ------
""
"| Check | Result |"
"|---|---|"
$rows | ForEach-Object { $_ }
""
exit 0
