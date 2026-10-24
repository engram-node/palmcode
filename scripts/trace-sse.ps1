# SSE trace recorder (M1). Captures real streaming responses from a vLLM(-like)
# endpoint so core/llm's SSE parser and vLLM quirk handling can be built and
# regression-tested offline. v1 fixture set is vLLM-only (docs/VLLM.md).
#
# Secrets policy:
#   - API key comes from an ENV VAR only (never args, never files).
#   - Output dir MUST be git-ignored unless -AllowCommit (scrubbed fixtures
#     may then live under core/llm/fixtures/vllm/).
#   - Every captured byte is scrubbed before writing to disk; if the key
#     survives scrubbing anywhere, the file is deleted and the run aborts.
#
# Usage:
#   $env:PALMCODE_LLM_API_KEY = "sk-..."
#   powershell -ExecutionPolicy Bypass -File scripts/trace-sse.ps1
param(
    [string]$OutDir = "traces",
    [string]$BaseUrl = $env:PALMCODE_LLM_BASE_URL,
    [string]$Model = $env:PALMCODE_LLM_MODEL,
    [string]$FastModel = $env:PALMCODE_LLM_FAST_MODEL,
    [int]$MaxTokens = 4096,
    [switch]$AllowCommit
)

$repo = Split-Path $PSScriptRoot -Parent
$outAbs = Join-Path $repo $OutDir
New-Item -ItemType Directory -Force $outAbs | Out-Null

# --- output policy
Push-Location $repo
$ignored = git check-ignore $OutDir 2>$null
Pop-Location
if (-not $ignored -and -not $AllowCommit) {
    throw "$OutDir/ is not git-ignored; refusing WITHOUT -AllowCommit (scrubbed fixtures only)."
}

# --- key from env only
$apiKey = $env:PALMCODE_LLM_API_KEY
if (-not $apiKey) { throw "set `$env:PALMCODE_LLM_API_KEY first" }

$readFile = @{
    type     = "function"
    function = @{
        name        = "read_file"
        description = "Read a file from the workspace"
        parameters  = @{ type = "object"; properties = @{ path = @{ type = "string" } }; required = @("path") }
    }
}
$listFiles = @{
    type     = "function"
    function = @{
        name        = "list_files"
        description = "List files under a directory"
        parameters  = @{ type = "object"; properties = @{ dir = @{ type = "string" } }; required = @("dir") }
    }
}
# deterministic ~6 KB prefill payload (byte-stable across runs)
$prefill = ("The quick brown fox jumps over the lazy dog. " * 340)

$cases = @(
    @{
        name = "simple"; model = $FastModel
        body = @{
            model = $FastModel; stream = $true; max_tokens = 64
            messages = @(@{ role = "user"; content = "Reply with the word: pong" })
        }
    },
    @{
        name = "reasoning"; model = $Model
        body = @{
            model = $Model; stream = $true; max_tokens = $MaxTokens
            messages = @(@{ role = "user"; content = "What is 17*23? Think briefly, then answer with just the number." })
        }
    },
    @{
        name = "tool_call"; model = $Model
        body = @{
            model = $Model; stream = $true; max_tokens = $MaxTokens; tool_choice = "auto"; tools = @($readFile)
            messages = @(@{ role = "user"; content = "Read the file src/main.rs with the read_file tool." })
        }
    },
    @{
        name = "parallel_tools"; model = $Model
        body = @{
            model = $Model; stream = $true; max_tokens = $MaxTokens; tool_choice = "auto"; tools = @($readFile, $listFiles)
            messages = @(@{ role = "user"; content = "In ONE step: read Cargo.toml with read_file AND list the src directory with list_files." })
        }
    },
    @{
        name = "tool_result_followup"; model = $Model
        body = @{
            model = $Model; stream = $true; max_tokens = $MaxTokens; tools = @($readFile)
            messages = @(
                @{ role = "user"; content = "Read the file src/main.rs with the read_file tool." },
                @{ role = "assistant"; content = $null; tool_calls = @(@{
                    id = "call_1"; type = "function"
                    function = @{ name = "read_file"; arguments = '{"path":"src/main.rs"}' } }) },
                @{ role = "tool"; tool_call_id = "call_1"; content = 'fn main() { println!("hi"); }' }
            )
        }
    },
    @{
        name = "long_prefill"; model = $Model
        body = @{
            model = $Model; stream = $true; max_tokens = 256
            messages = @(@{ role = "user"; content = "Context: $prefill`nQuestion: which animal does the fox jump over? One word." })
        }
    },
    @{
        name = "error_malformed_tool"; model = $Model; allow_non_sse = $true
        body = @{
            model = $Model; stream = $true; max_tokens = 64
            messages = @(@{ role = "user"; content = "hi" })
            tools = @(@{ type = "function" })   # missing function.name etc.
        }
    }
)

function Scrub([string]$text, [string]$key) {
    $t = $text
    if ($key) { $t = $t.Replace($key, "<REDACTED-KEY>") }
    $t = [regex]::Replace($t, 'sk-[A-Za-z0-9_\-]{18,}', '<REDACTED-KEY>')
    $t = [regex]::Replace($t, '[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}', '<REDACTED-EMAIL>')
    $t = [regex]::Replace($t, '("user"\s*:\s*")[^"]*(")', '$1<REDACTED-USER>$2')
    return $t
}

# server introspection for metadata.json (vLLM: /version; proxies may 404)
$serverInfo = @{}
foreach ($p in @("/version", "/v1/version")) {
    try {
        $u = ($BaseUrl -replace "/v1/?$", "") + $p
        $r = Invoke-WebRequest -Uri $u -Headers @{ Authorization = "Bearer $apiKey" } -TimeoutSec 15 -UseBasicParsing
        $serverInfo[$p.Trim('/')] = $r.Content.Substring(0, [Math]::Min(300, $r.Content.Length))
    } catch { $serverInfo[$p.Trim('/')] = "unavailable" }
}

foreach ($case in $cases) {
    $body = ($case.body | ConvertTo-Json -Depth 8 -Compress)
    $req = New-TemporaryFile; Set-Content $req $body -NoNewline
    $raw = New-TemporaryFile
    "CASE $($case.name) (model=$($case.model)) ..."
    # Key passes via stdin config - never argv (process list), never disk.
    "header = `"Authorization: Bearer $apiKey`"" | & curl.exe -sS -N --config - `
        -H "Content-Type: application/json" -H "Cache-Control: no-cache" `
        -d "@$req" -o $raw -w "%{http_code}" --max-time 300 `
        "$BaseUrl/chat/completions" | ForEach-Object { $script:code = $_ }
    $sse = Get-Content $raw -Raw
    $isSse = $sse -match "^data:" -or $sse -match "`ndata:"
    if (-not $isSse -and -not $case.allow_non_sse) {
        Remove-Item $req, $raw -ErrorAction SilentlyContinue
        throw "case $($case.name): not SSE (HTTP $script:code): $($sse.Substring(0, [Math]::Min(300, $sse.Length)))"
    }
    $scrubbed = Scrub $sse $apiKey
    if ($apiKey -and $scrubbed.Contains($apiKey)) { throw "scrub failed: key present; deleting capture" }

    $ssePath = Join-Path $outAbs "$($case.name).sse"
    Set-Content $ssePath $scrubbed -NoNewline
    if ($isSse) {
        $events = $scrubbed -split "`n" | Where-Object { $_ -match "^data: " } |
            ForEach-Object { $_.Substring(6).TrimStart() } | Where-Object { $_ -ne "[DONE]" }
        Set-Content (Join-Path $outAbs "$($case.name).jsonl") ($events -join "`n")
        $meta = "$((($events | Where-Object { $_ }).Count)) data chunks"
    } else {
        $meta = "HTTP $script:code, non-SSE body ($($sse.Length) bytes)"
    }
    "      -> $meta"
    Remove-Item $req, $raw -ErrorAction SilentlyContinue
}

# metadata.json - fixture provenance (parser names must be filled from the
# server config of the endpoint under test; introspectable fields only here)
$metaObj = [ordered]@{
    captured_at  = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    endpoint     = $BaseUrl
    models       = @{ main = $Model; fast = $FastModel }
    server       = $serverInfo
    tool_parser  = "unknown-server-side-flag (--tool-call-parser); see docs/VLLM.md"
    cases        = $cases | ForEach-Object { $_.name }
    scrubbed     = $true
}
$metaObj | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $outAbs "metadata.json")
""
"Trace set complete in $OutDir/ + metadata.json. Cases: $($cases.name -join ', ')"
exit 0
