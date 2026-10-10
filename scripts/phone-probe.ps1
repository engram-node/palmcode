# One-command physical-device probe (M0 gate). Prints results as markdown you
# can paste into docs/VERIFY.md. Run on THIS machine with a phone attached
# via USB debugging:
#
#   powershell -ExecutionPolicy Bypass -File scripts/phone-probe.ps1 -Serial <adb-serial>
#
# Optional: -GitBinary <path to stripped aarch64 git> — autodetects the M0
# spike build under MSYS2 home. Pass -GitBinary "" to skip the git section.
param(
    [string]$Serial = "",
    [string]$GitBinary = "C:\msys64\home\OM\palmcode-spike\git-2.56.0\git"
)
# NB: no $ErrorActionPreference="Stop" — PowerShell 5.1 turns *any* native
# stderr output (adb progress messages) into an ending NativeCommandError.
# Guards instead: explicit exit-code checks + content assertions below.
$sdk  = "C:\Users\OM\AppData\Local\Android\Sdk"
$adb  = "$sdk\platform-tools\adb.exe"
$re   = "$sdk\ndk\30.0.16248370\toolchains\llvm\prebuilt\windows-x86_64\bin\llvm-readelf.exe"
$strip = "$sdk\ndk\30.0.16248370\toolchains\llvm\prebuilt\windows-x86_64\bin\llvm-strip.exe"
$apk  = "C:\repos\palmcode\android\app\build\outputs\apk\debug\app-debug.apk"

if ($Serial) { $target = @("-s", $Serial) } else {
    $devs = (& $adb devices) -split "`n" | Where-Object { $_ -match "`tdevice$" }
    if ($devs.Count -ne 1) { throw "pass -Serial; devices found: $($devs.Count)" }
    $target = @("-s", ($devs[0] -split "`t")[0])
}

function Sh([string]$cmd) { (& $adb @target shell $cmd 2>$null | Out-String).Trim() }
function Dump-Ui {
    & $adb @target shell uiautomator dump /sdcard/dump.xml 2>$null | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "uiautomator dump failed ($LASTEXITCODE)" }
    & $adb @target pull /sdcard/dump.xml "$env:TEMP\palmcode-probe-dump.xml" 2>$null | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "adb pull failed ($LASTEXITCODE)" }
    return [xml](Get-Content "$env:TEMP\palmcode-probe-dump.xml" -Raw)
}

# ---- identity
$model = Sh "getprop ro.product.model"
$av    = Sh "getprop ro.build.version.release"
$sdkLv = Sh "getprop ro.build.version.sdk"
$abi   = Sh "getprop ro.product.cpu.abi"
if (-not (Test-Path $apk)) { throw "build first: ./gradlew assembleDebug ($apk missing)" }

# ---- 1. FFI greeting
& $adb @target install -r $apk 2>$null | Out-Null
if ($LASTEXITCODE -ne 0) { throw "adb install failed ($LASTEXITCODE)" }
& $adb @target shell am start -W -n dev.palmcode/dev.palmcode.app.MainActivity 2>$null | Out-Null
Start-Sleep -Seconds 6
$ui = Dump-Ui
$greet = ($ui.SelectSingleNode("//*[contains(@text,'palmcode core')]")).text
if (-not $greet) { throw "greeting not rendered" }

# ---- 2. exec-through-symlink (tap the button, read result)
$btn = $ui.SelectSingleNode("//node[@text='Run exec probe']/ancestor-or-self::node[@clickable='true'][1]")
if (-not $btn) { $btn = $ui.SelectSingleNode("//node[@text='Run exec probe']/parent::node[@clickable='true']") }
if ($btn.bounds -match "\[(\d+),(\d+)\]\[(\d+),(\d+)\]") {
    $x = [int](([int]$Matches[1] + [int]$Matches[3]) / 2); $y = [int](([int]$Matches[2] + [int]$Matches[4]) / 2)
    & $adb @target shell input tap $x $y 2>$null
}
Start-Sleep -Seconds 4
$ui2 = Dump-Ui
# the app prints result + symlink target into ONE Text node: line1 = result,
# line2 = target path
$probeRaw = ($ui2.SelectSingleNode("//*[starts-with(@text,'exit=')]")).text
if (-not ($probeRaw -match "exit=42" -and $probeRaw -match "exec-probe ok")) { throw "exec probe failed: $probeRaw" }
$probeLines = $probeRaw -split "`r?`n" | Where-Object { $_.Trim() }
$probe = ($probeLines[0] -replace "\s+", " ").Trim()
$probePath = if ($probeLines[1]) { ($probeLines[1] -replace "\s+", " ").Trim() } else { "" }

# ---- 3. 16 KB alignment of the natives the phone actually runs
$work = "$env:TEMP\palmcode-probe-apk"
Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $work | Out-Null
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [System.IO.Compression.ZipFile]::OpenRead($apk)
foreach ($e in $zip.Entries) {
    if ($e.FullName -match "^lib/$abi/.*\.so$") {
        $dst = Join-Path $work $e.Name
        [System.IO.Compression.ZipFileExtensions]::ExtractToFile($e, $dst, $true)
    }
}
$zip.Dispose()
$align = foreach ($f in Get-ChildItem $work -Filter *.so) {
    $loads = (& $re -lW $f.FullName) | Select-String "LOAD"
    $bad = $loads | Where-Object { $_ -notmatch "0x4000\s*$" }
    if ($bad) { "$($f.Name): MISALIGNED" } else { "$($f.Name): ok" }
}

# ---- 4. git on-device (skip if binary not available)
$gitResult = "SKIP (no binary)"
if ($GitBinary -and (Test-Path $GitBinary)) {
    $stripped = "$env:TEMP\palmcode-probe-git"
    & $strip -o $stripped $GitBinary
    & $adb @target push $stripped /data/local/tmp/git 2>$null | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "adb push failed ($LASTEXITCODE)" }
    $gv = Sh "chmod 755 /data/local/tmp/git; /data/local/tmp/git --version"
    $seq = @(
        "export HOME=/data/local/tmp; cd /data/local/tmp; rm -rf probe-repo",
        "/data/local/tmp/git init -q probe-repo",
        "cd probe-repo && echo a > a && /data/local/tmp/git add a",
        "/data/local/tmp/git -c user.email=t@t -c user.name=t commit -qm c1",
        "echo b >> a && /data/local/tmp/git add a",
        "/data/local/tmp/git -c user.email=t@t -c user.name=t commit -qm c2",
        "/data/local/tmp/git log --oneline && /data/local/tmp/git status --short && echo GIT_SEQ_OK"
    ) -join "; "
    $log = Sh $seq
    Sh "rm -rf /data/local/tmp/git /data/local/tmp/probe-repo /sdcard/dump.xml" | Out-Null
    $gitSize = [math]::Round((Get-Item $stripped).Length / 1MB, 2)
    $gitResult = if ($log -match "GIT_SEQ_OK" -and $log -match "c2") { "$gv; init/add/commit x2/log/status OK; $gitSize MB" } else { "FAIL: $log" }
}

# ---- pasteable block
$date = Get-Date -Format "yyyy-MM-dd"
""
"| Field | Result |"
"|---|---|"
"| Device | $model, Android $av (API $sdkLv), $abi |"
"| Date | $date |"
"| FFI greeting | ``$greet`` |"
"| Exec-via-symlink | ``$probe`` |"
"| Symlink target | ``$probePath`` |"
"| 16 KB alignment | $($align -join '; ') |"
"| git on-device | $gitResult |"
""
"PROBE PASS"
exit 0
