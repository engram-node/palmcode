param(
    [ValidateRange(1,1000)][int]$Turns = 50,
    [ValidateRange(0,1000)][int]$TokensPerSecond = 0,
    [switch]$Ui,
    [string]$Result = 'stage3-050.json',
    [switch]$SelfTest
)
$ErrorActionPreference = 'Stop'

function Quantile($Values, [double]$Fraction) {
    $ordered = @($Values | Sort-Object)
    if (!$ordered.Count) { throw 'Missing timing samples' }
    $ordered[[math]::Max(0, [math]::Ceiling($Fraction * $ordered.Count) - 1)]
}

function Parse-Timings([string[]]$Lines, [int]$Expected) {
    $gaps = [Collections.Generic.List[long]]::new()
    $turns = [Collections.Generic.List[object]]::new()
    $phases = @{}
    $active = $false
    $gapCount = 0
    foreach ($line in $Lines) {
        if ($line -match 'PHASE benchmark_turn_start=(\d+)') {
            if ($active -or [int]$Matches[1] -ne $turns.Count + 1) { throw 'Out-of-order turn start' }
            $phases = @{}; $active = $true; $gapCount = 0
        } elseif ($active -and $line -match 'PHASE name=(\w+) us=(\d+)') {
            $name = $Matches[1]
            $phases[$name] = [long]$phases[$name] + [long]$Matches[2]
        } elseif ($active -and $line -match 'OVERHEAD turn_gap_us=(\d+)') {
            $gaps.Add([long]$Matches[1])
            $gapCount++
        } elseif ($active -and $line -match 'PHASE turn=(\d+) wall_ms=(\d+) tools=(\d+)') {
            if ([int]$Matches[1] -ne $turns.Count + 1 -or $gapCount -ne 1 -or [int]$Matches[3] -ne 1) { throw 'Invalid scripted turn completion' }
            $turns.Add([ordered]@{ turn=[int]$Matches[1]; wall_ms=[long]$Matches[2]; tools=[int]$Matches[3]; phase_us=$phases })
            $active = $false
        }
    }
    if ($gaps.Count -ne $Expected -or $turns.Count -ne $Expected) {
        throw "Incomplete run: gaps=$($gaps.Count) turns=$($turns.Count) expected=$Expected"
    }
    [ordered]@{ overhead_us=@($gaps); turns=@($turns.ToArray()) }
}

if ($SelfTest) {
    if ((Quantile @(4,1,3,2) 0.5) -ne 2 -or (Quantile @(4,1,3,2) 0.95) -ne 4) { throw 'quantile regression' }
    $sample = Parse-Timings @('PHASE benchmark_turn_start=1','PHASE name=persist us=3','PHASE name=persist us=4','OVERHEAD turn_gap_us=9','PHASE turn=1 wall_ms=2 tools=1') 1
    if ($sample.turns[0].phase_us.persist -ne 7 -or $sample.overhead_us[0] -ne 9) { throw 'phase grouping regression' }
    $rejected = $false
    try { Parse-Timings @() 1 | Out-Null } catch { $rejected = $true }
    if (!$rejected) { throw 'incomplete run accepted' }
    foreach ($bad in @(
        @('PHASE benchmark_turn_start=2'),
        @('PHASE benchmark_turn_start=1','PHASE benchmark_turn_start=2'),
        @('PHASE benchmark_turn_start=1','OVERHEAD turn_gap_us=1','PHASE turn=1 wall_ms=2 tools=0'),
        @('PHASE benchmark_turn_start=1','OVERHEAD turn_gap_us=1','OVERHEAD turn_gap_us=2','PHASE turn=1 wall_ms=2 tools=1')
    )) {
        $rejected = $false
        try { Parse-Timings $bad 1 | Out-Null } catch { $rejected = $true }
        if (!$rejected) { throw 'invalid protocol accepted' }
    }
    Write-Output 'Timing parser self-tests passed'
    exit 0
}

$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
if ([IO.Path]::GetFileName($Result) -ne $Result -or !$Result.EndsWith('.json')) { throw 'Result must be a JSON filename' }
$adb = (Get-Command adb.exe -ErrorAction Stop).Source
$mockExe = Join-Path $root 'core/target/debug/palmcode-bench.exe'
$build = Join-Path $root 'build'
$resultDir = Join-Path $root 'docs/results'
[IO.Directory]::CreateDirectory($resultDir) | Out-Null
# A mock must never inherit real provider configuration or credentials.
Get-ChildItem Env:PALMCODE_LLM_* | ForEach-Object { Remove-Item -LiteralPath ('Env:' + $_.Name) }
$port = 8787
$url = "http://127.0.0.1:$port/v1"
$uiText = $Ui.IsPresent.ToString().ToLowerInvariant()
$mockCommand = "core/target/debug/palmcode-bench.exe stage3 $Turns $port $TokensPerSecond"
$instrumentCommand = "adb shell am instrument -w -e url $url -e turns $Turns -e ui $uiText dev.palmcode.bench/dev.palmcode.bench.BenchInstrumentation"
$mock = $null
$instrument = $null
try {
    $devices = @(& $adb devices | Where-Object { $_ -match '\sdevice$' })
    if ($devices.Count -ne 1) { throw 'Exactly one attached device is required' }
    & $adb shell am force-stop dev.palmcode.bench
    $mock = Start-Process -FilePath $mockExe -ArgumentList @('stage3',$Turns,$port,$TokensPerSecond) -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $build 'stage3-mock.out') -RedirectStandardError (Join-Path $build 'stage3-mock.err')
    $ready = $false
    for ($n=0; $n -lt 50; $n++) {
        if ((Get-Content (Join-Path $build 'stage3-mock.out') -Raw -ErrorAction SilentlyContinue) -match 'stage3 mock ready') { $ready=$true; break }
        Start-Sleep -Milliseconds 100
    }
    if (!$ready) { throw 'Mock did not start' }
    & $adb reverse "tcp:$port" "tcp:$port"
    $instrument = Start-Process -FilePath $adb -ArgumentList @('shell','am','instrument','-w','-e','url',$url,'-e','turns',$Turns,'-e','ui',$uiText,'dev.palmcode.bench/dev.palmcode.bench.BenchInstrumentation') -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $build 'stage3-instrument.out') -RedirectStandardError (Join-Path $build 'stage3-instrument.err')
    $phonePid = ''
    for ($n=0; $n -lt 100; $n++) {
        $phonePid = ((& $adb shell pidof dev.palmcode.bench) -join '').Trim()
        if ($phonePid) { break }
        Start-Sleep -Milliseconds 100
    }
    if (!$phonePid) { throw 'Benchmark process did not start' }
    $logCommand = "adb logcat -d --pid=$phonePid -v brief -s palmcode:I"
    $deadline = [DateTime]::UtcNow.AddMinutes(20)
    do {
        $lines = @(& $adb logcat -d "--pid=$phonePid" -v brief -s palmcode:I)
        if ($lines -match 'PHASE failed') { throw 'Benchmark reported failure' }
        if ($lines -match "PHASE complete turns=$Turns") { break }
        if ($instrument.HasExited) { throw 'Instrumentation exited before benchmark completion' }
        Start-Sleep -Milliseconds 500
    } while ([DateTime]::UtcNow -lt $deadline)
    $raw = Parse-Timings $lines $Turns
    $phaseSummary = [ordered]@{}
    foreach ($name in @('persist','request_build','serialize','connect','dispatch')) {
        $values = @($raw.turns | ForEach-Object { [long]$_.phase_us[$name] })
        $phaseSummary[$name] = [ordered]@{ p50_us=(Quantile $values 0.5); p95_us=(Quantile $values 0.95) }
    }
    $gfx = @()
    if ($Ui) { $gfx = @(& $adb shell dumpsys gfxinfo dev.palmcode.bench) }
    $gfxSummary = @($gfx | Where-Object { $_ -match 'Total frames|Janky frames|percentile|Number Missed|Number High|Number Slow|Number Frame|Number GPU' } | ForEach-Object { $_.Trim() })
    if (!$instrument.WaitForExit(30000)) { throw 'Instrumentation did not terminate' }
    if (!((Get-Content (Join-Path $build 'stage3-instrument.out') -Raw) -match 'complete=true')) { throw 'Instrumentation completion was not acknowledged' }
    $mockSpans = @(Get-Content (Join-Path $build 'stage3-mock.out') | ForEach-Object {
        if ($_ -match 'MOCK_STREAM n=(\d+) us=(\d+)') { [ordered]@{ response=[int]$Matches[1]; us=[long]$Matches[2] } }
    })
    $resultObject = [ordered]@{
        date='2026-10-26'; device=((& $adb shell getprop ro.product.model) -join '').Trim(); sdk=((& $adb shell getprop ro.build.version.sdk) -join '').Trim()
        commands=@($mockCommand,"adb reverse tcp:$port tcp:$port",$instrumentCommand,$logCommand)
        turns=$Turns; requested_tokens_per_second=$TokensPerSecond; ui=$Ui.IsPresent
        measurement_boundary='tool-stream-end_to_reqwest-dispatch'
        bench_apk_sha256=(Get-FileHash (Join-Path $root 'android/app/build/outputs/apk/bench/app-universal-bench.apk') -Algorithm SHA256).Hash.ToLowerInvariant()
        mock_exe_sha256=(Get-FileHash $mockExe -Algorithm SHA256).Hash.ToLowerInvariant()
        overhead_p50_us=(Quantile $raw.overhead_us 0.5); overhead_p95_us=(Quantile $raw.overhead_us 0.95)
        phase_totals_per_turn=$phaseSummary; raw=$raw; mock_streams=$mockSpans; gfxinfo=$gfxSummary
    }
    if ($Ui) { $resultObject.commands += 'adb shell dumpsys gfxinfo dev.palmcode.bench' }
    $json = $resultObject | ConvertTo-Json -Depth 10 -Compress
    [IO.File]::WriteAllText((Join-Path $resultDir $Result),$json+"`n",[Text.UTF8Encoding]::new($false))
    Write-Output ("turns={0} p50_us={1} p95_us={2} result={3}" -f $Turns,$resultObject.overhead_p50_us,$resultObject.overhead_p95_us,$Result)
    $gfxSummary
} finally {
    if ($instrument -and !$instrument.HasExited) { Stop-Process -Id $instrument.Id -Force -ErrorAction SilentlyContinue }
    & $adb shell am force-stop dev.palmcode.bench
    & $adb reverse --remove "tcp:$port" 2>$null
    if ($mock -and !$mock.HasExited) { Stop-Process -Id $mock.Id -Force -ErrorAction SilentlyContinue }
}
