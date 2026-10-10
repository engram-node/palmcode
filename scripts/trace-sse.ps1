# SSE trace recorder (M1). Captures real streaming responses from an
# OpenAI-compatible endpoint so core/llm's SSE parser and provider-quirk
# handling can be built and regression-tested offline.
#
# Secrets policy:
#   - API key comes from an ENV VAR only (never args, never files).
#   - Output dir MUST be git-ignored (abort otherwise).
#   - Every captured byte is scrubbed before writing to disk; if the key
#     survives scrubbing anywhere, the file is deleted and the run aborts.
#   - Nothing here is ever committed: traces/ is in .gitignore.
#
# Usage:
#   $env:PALMCODE_LLM_API_KEY = "sk-..."
#   powershell -ExecutionPolicy Bypass -File scripts/trace-sse.ps1
param(
    [string]$OutDir = "traces",
    [string]$BaseUrl = "https://grid.ai.juspay.net/v1",
    [string]$Model = "glm-latest",
    [string]$FastModel = "glm-5.3-flash",
    [int]$MaxTokens = 4096
)

$repo = Split-Path $PSScriptRoot -Parent
$outAbs = Join-Path $repo $OutDir
New-Item -ItemType Directory -Force $outAbs | Out-Null

# --- must be git-ignored
Push-Location $repo
$ignored = git check-ignore $OutDir
Pop-Location
if (-not $ignored) { throw "$OutDir/ is not git-ignored; refusing to write traces there." }

# --- key from env only
$apiKey = $env:PALMCODE_LLM_API_KEY
if (-not $apiKey) { $apiKey = $env:GLM_API_KEY }
if (-not $apiKey) { throw "set `$env:PALMCODE_LLM_API_KEY (or GLM_API_KEY) first" }

$cases = @(
    @{
        name   = "simple"
        model  = $FastModel
        body   = @{
            model      = $FastModel
            messages   = @(@{ role = "user"; content = "Reply with the word: pong" })
            max_tokens = 64
            stream     = $true
        }
    },
    @{
        name   = "reasoning"
        model  = $Model
        body   = @{
            model      = $Model
            messages   = @(@{ role = "user"; content = "What is 17*23? Think briefly, then answer with just the number." })
            max_tokens = $MaxTokens
            stream     = $true
        }
    },
    @{
        name   = "tool_call"
        model  = $Model
        body   = @{
            model      = $Model
            messages   = @(@{ role = "user"; content = "Read the file src/main.rs with the read_file tool." })
            max_tokens = $MaxTokens
            stream     = $true
            tools      = @(@{
                type     = "function"
                function = @{
                    name        = "read_file"
                    description = "Read a file from the workspace"
                    parameters  = @{
                        type       = "object"
                        properties = @{ path = @{ type = "string" } }
                        required   = @("path")
                    }
                }
            })
            tool_choice = "auto"
        }
    },
    @{
        name   = "tool_result_followup"
        model  = $Model
        body   = @{
            model      = $Model
            messages   = @(
                @{ role = "user"; content = "Read the file src/main.rs with the read_file tool." },
                @{ role = "assistant"; content = $null; tool_calls = @(@{
                    id = "call_1"; type = "function"
                    function = @{ name = "read_file"; arguments = '{"path":"src/main.rs"}' } }) },
                @{ role = "tool"; tool_call_id = "call_1"; content = 'fn main() { println!("hi"); }' }
            )
            max_tokens = $MaxTokens
            stream     = $true
            tools      = @()
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

foreach ($case in $cases) {
    if ($case.body.stream -ne $true) { throw "internal error: stream must be on" }
    $body = ($case.body | ConvertTo-Json -Depth 8 -Compress)
    $req = New-TemporaryFile; Set-Content $req $body -NoNewline
    $raw = New-TemporaryFile
    "CASE $($case.name) (model=$($case.model)) ..."
    # Key passes via stdin config — never argv (process list), never disk.
    "header = `"Authorization: Bearer $apiKey`"" | & curl.exe -sS -N --config - `
        -H "Content-Type: application/json" `
        -H "Cache-Control: no-cache" `
        -d "@$req" -o $raw --max-time 300 `
        "$BaseUrl/chat/completions"
    if ($LASTEXITCODE -ne 0) { Remove-Item $req, $raw -ErrorAction SilentlyContinue; throw "curl failed ($LASTEXITCODE) for case $($case.name)" }
    $sse = Get-Content $raw -Raw
    if (-not ($sse -match "^data:" -or $sse -match "`ndata:")) {
        Remove-Item $req, $raw -ErrorAction SilentlyContinue
        throw "case $($case.name): response is not SSE (auth/model/permission problem?):`n$($sse.Substring(0, [Math]::Min(400, $sse.Length)))"
    }

    $scrubbed = Scrub $sse $apiKey
    if ($apiKey -and $scrubbed.Contains($apiKey)) {
        throw "scrub failed: key still present; deleting capture"
    }
    $ssePath = Join-Path $outAbs "$($case.name).sse"
    Set-Content $ssePath $scrubbed -NoNewline

    # parsed jsonl: one object per data line (minus the sentinel)
    $events = $scrubbed -split "`n" | Where-Object { $_ -match "^data: " } |
        ForEach-Object { $_.Substring(6).TrimStart() } | Where-Object { $_ -ne "[DONE]" }
    $jlPath = Join-Path $outAbs "$($case.name).jsonl"
    Set-Content $jlPath ($events -join "`n")

    $chunks = ($events | Where-Object { $_ -and $_.Trim() }).Count
    "      -> $chunks data chunks; wrote $([IO.Path]::GetFileName($ssePath)) + $([IO.Path]::GetFileName($jlPath))"
    Remove-Item $req, $raw -ErrorAction SilentlyContinue
}

""
"Trace set complete in $OutDir/. Endpoint used: POST $BaseUrl/chat/completions (SSE)."
"Models exercised: $Model, $FastModel. Add more -cases here as M1 needs them."
exit 0
