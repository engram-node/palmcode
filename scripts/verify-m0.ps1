# M0 on-device verification (not committed long-term; superseded by a proper
# androidTest in a later milestone).
param([string]$Serial = "")
$ErrorActionPreference = "Stop"
$adb = "C:\Users\OM\AppData\Local\Android\Sdk\platform-tools\adb.exe"
if ($Serial) { $target = @("-s", $Serial) } else { $target = @() }
$apk = "C:\repos\palmcode\android\app\build\outputs\apk\debug\app-debug.apk"

function Dump-Ui {
    & $adb @target shell uiautomator dump /sdcard/dump.xml | Out-Null
    & $adb @target pull /sdcard/dump.xml "$env:TEMP\dump.xml" | Out-Null
    return [xml](Get-Content "$env:TEMP\dump.xml" -Raw)
}

& $adb @target install -r $apk
& $adb @target shell am start -W -n dev.pocket/dev.pocket.app.MainActivity | Out-Null
Start-Sleep -Seconds 6

$ui = Dump-Ui
$greetingNode = $ui.SelectSingleNode("//*[contains(@text,'pocket core')]")
if (-not $greetingNode) { throw "greeting text not found in UI dump" }
"GREETING    : $($greetingNode.text)"

# find the clickable ancestor of the probe-button label and tap its center
$btn = $ui.SelectSingleNode("//node[@text='Run exec probe']/ancestor-or-self::node[@clickable='true'][1]")
if (-not $btn) { $btn = $ui.SelectSingleNode("//node[@text='Run exec probe']/parent::node[@clickable='true']") }
if (-not $btn) { throw "probe button not found" }
$bounds = $btn.bounds   # [l,t][r,b]
if ($bounds -match "\[(\d+),(\d+)\]\[(\d+),(\d+)\]") {
    $l = [int]$Matches[1]; $t = [int]$Matches[2]; $r = [int]$Matches[3]; $b = [int]$Matches[4]
    $x = [int](($l + $r) / 2); $y = [int](($t + $b) / 2)
    "TAP         : $x,$y"; & $adb @target shell input tap $x $y
}
Start-Sleep -Seconds 4

$ui2 = Dump-Ui
$probe = $ui2.SelectSingleNode("//*[contains(@text,'exit=')]")
if (-not $probe) { throw "probe result not found" }
"PROBE       : $($probe.text)"
if ($probe.text -match "exit=42" -and $probe.text -match "exec-probe ok") { "M0-DEVICE-VERIFY PASS" } else { throw "probe output unexpected: $($probe.text)" }
