# Thin wrapper kept for muscle memory — the real harness is phone-probe.ps1.
param([string]$Serial = "")
if ($Serial) { & "$PSScriptRoot\phone-probe.ps1" -Serial $Serial } else { & "$PSScriptRoot\phone-probe.ps1" }
if ($LASTEXITCODE -ne 0) { throw "M0-DEVICE-VERIFY FAIL (exit $LASTEXITCODE)" }
"M0-DEVICE-VERIFY PASS"
