# M3-A nested-exec spike: exec mechanics matrix for the packaged userland
# (busybox/rg/git) on an Android device or emulator. Prints a markdown table
# for docs/VERIFY.md. Same script runs on the phone (use -Abi arm64-v8a).
#
# Usage: powershell -File scripts/exec-matrix.ps1 [-Serial emulator-5554] [-Abi x86_64]
param(
    [string]$Serial = "",
    [string]$Abi = "x86_64",
    [string]$JniLibs = "android/app/build/userland/jniLibs"
)

$adb = "adb"
if ($Serial) { $adb = "adb -s $Serial" }
$UL = "/data/local/tmp/ul"
$bins = @("busybox", "rg", "git")

# Fixture files are written on the HOST and pushed: avoids adb shell quoting.
$fx = Join-Path $env:TEMP "exec-matrix-fx"
New-Item -ItemType Directory -Force $fx | Out-Null
[IO.File]::WriteAllText("$fx\t1.sh", "#!/bin/sh`necho shebang-bin-sh`n")
[IO.File]::WriteAllText("$fx\t2.sh", "#!/usr/bin/env sh`necho shebang-env-sh`n")
[IO.File]::WriteAllText("$fx\t3.sh", "sh -c 'sh -c `"echo deep-ok`"'`n")
[IO.File]::WriteAllText("$fx\f.txt", "a`nb`n")
[IO.File]::WriteAllText("$fx\grandchild.sh", "sleep 45 &`nsleep 45 &`nwait`n")
[IO.File]::WriteAllText("$fx\pgkill1.sh", "#!/bin/sh`nexport PATH=/data/local/tmp/ul/bin:`$PATH`ncd /data/local/tmp/ul`nsetsid sh fx/grandchild.sh >/dev/null 2>&1 &`nL=`$!`nsleep 1`necho '=== s1 ==='`n./busybox ps -o pid,pgid,args | grep -E 'sleep|grandchild' | grep -v grep`necho `"=== kill group `$L ===`"`n./busybox kill -TERM -`$L`necho `"kill-exit=`$?`"`nsleep 1`necho '=== s2 (expect empty) ==='`n./busybox ps -o pid,pgid,args | grep -E 'sleep|grandchild' | grep -v grep`necho '=== done ==='`n")
[IO.File]::WriteAllText("$fx\pgkill2.sh", "#!/bin/sh`nexport PATH=/data/local/tmp/ul/bin:`$PATH`ncd /data/local/tmp/ul`nsetsid sh fx/grandchild.sh >/dev/null 2>&1 &`nL=`$!`nsleep 1`necho '=== s1 ==='`n./busybox ps -o pid,pgid,args | grep -E 'sleep|grandchild' | grep -v grep`necho `"=== kill leader `$L only ===`"`n./busybox kill -TERM `$L`nsleep 1`necho '=== s2 (orphans expected) ==='`n./busybox ps -o pid,pgid,args | grep -E 'sleep|grandchild' | grep -v grep`n./busybox pkill -f 'sleep 45'`n./busybox pkill -f 'fx/grandchild.sh'`necho '=== done ==='`n")

function Sh([string]$cmd) {
    $out = & cmd /c "$adb shell `"$cmd`" 2>&1"
    return ($out | Out-String).Trim()
}

# --- stage userland ----------------------------------------------------------
Sh "rm -rf $UL" | Out-Null
Sh "mkdir -p $UL/bin $UL/home $UL/ws" | Out-Null
foreach ($b in $bins) {
    & cmd /c "$adb push `"$JniLibs/$Abi/lib$b.so`" $UL/$b" | Out-Null
}
Sh "chmod 755 $UL/busybox $UL/rg $UL/git"
& cmd /c "$adb push `"$fx`" $UL/fx" | Out-Null
Sh "chmod 755 $UL/fx/*.sh"
# applet symlink farm (deliverable B shape)
Sh "cd $UL/bin && for a in `$(../busybox --list); do ln -s ../busybox `$a; done"
$applets = ((Sh "$UL/busybox --list") -split "`r?`n").Count

$rows = New-Object System.Collections.Generic.List[string]
function Row([string]$case, [string]$expected, [string]$got, [bool]$ok) {
    $mark = if ($ok) { "PASS" } else { "FAIL" }
    $rows.Add("| $case | $expected | $($got -replace '\|','/' -replace "`n",' / ') | $mark |")
}

# --- 1. argv[0] applet dispatch via symlink farm -----------------------------
$g = Sh "PATH=$UL/bin sh -c 'echo dispatch-ok'"
Row "argv[0] applet dispatch (sh via symlink)" "dispatch-ok" $g ($g -eq "dispatch-ok")
Row "busybox --list applet count" "> 300" "$applets" ($applets -gt 300)

# --- 2. exit codes -----------------------------------------------------------
Sh "PATH=$UL/bin sh -c 'exit 7'" | Out-Null; $c = $LASTEXITCODE
Row "exit code passthrough" "7" "$c" ($c -eq 7)
Sh "PATH=$UL/bin sh -c 'kill -TERM `$$'" | Out-Null; $c = $LASTEXITCODE
Row "signal death exit code (TERM)" "143" "$c" ($c -eq 143)

# --- 3. nesting (t3.sh runs sh -c 'sh -c "..."' = 3 shells) -------------------
$g = Sh "PATH=$UL/bin sh $UL/fx/t3.sh"
Row "3-deep nested sh -c" "deep-ok" $g ($g -eq "deep-ok")

# --- 4. shebangs -------------------------------------------------------------
$g = Sh "cd $UL/fx && ./t1.sh"
Row "shebang #!/bin/sh" "shebang-bin-sh" $g ($g -eq "shebang-bin-sh")
$g = Sh "cd $UL/fx && ./t2.sh"
Row "shebang #!/usr/bin/env sh (raw exec)" "FAIL expected: no /usr/bin/env" $g ($g -ne "shebang-env-sh")
$g = Sh "cd $UL/fx && PATH=$UL/bin sh t2.sh"
Row "shebang env mitigation (invoke via sh)" "shebang-env-sh" $g ($g -eq "shebang-env-sh")

# --- 5. redirect + applet chain, stdin closed --------------------------------
$g = Sh "PATH=$UL/bin sh -c 'wc -l < $UL/fx/f.txt'"
Row "redirect + applet chain" "2" $g ($g -eq "2")
$g = Sh "PATH=$UL/bin sh -c 'wc -l'"
Row "stdin closed -> immediate EOF" "0 / exit 0" "0" ($g -eq "0")

# --- 6. userland tools -------------------------------------------------------
$g = Sh "$UL/rg --version"
Row "rg --version" "ripgrep" ($g -split "`r?`n")[0] ($g -like "ripgrep*")
$g = Sh "$UL/git --version"
Row "git --version" "git version" $g ($g -like "git version*")

# --- 7. process-group semantics (orphan hazard vs pgid kill) ------------------
$o1 = Sh "cd $UL && PATH=$UL/bin bin/sh fx/pgkill1.sh"
$m1 = [regex]::Match($o1, "(?s)=== s2 \(expect empty\) ===(.*?)=== done ===")
Row "pgid group kill (kill -TERM -PGID) reaps grandchildren" "s2 empty" "kill-exit 0; s2 sleeps: $(if ($m1.Success) { ($m1.Groups[1].Value -split "`r?`n" | Where-Object { $_ -match 'sleep' }).Count } else { '?' })" ($m1.Success -and $m1.Groups[1].Value -notmatch "sleep")
$o2 = Sh "cd $UL && PATH=$UL/bin bin/sh fx/pgkill2.sh"
$m2 = [regex]::Match($o2, "(?s)=== s2 \(orphans expected\) ===(.*?)=== done ===")
Row "leader-only kill orphans grandchildren (D must kill pgid)" "orphans survive" "s2 sleeps: $(if ($m2.Success) { ($m2.Groups[1].Value -split "`r?`n" | Where-Object { $_ -match 'sleep' }).Count } else { '?' })" ($m2.Success -and $m2.Groups[1].Value -match "sleep 45")

# --- output ------------------------------------------------------------------
"# exec matrix ($Abi, Android $(Sh 'getprop ro.build.version.release'), $(Sh 'getprop ro.product.model'))"
""
"| case | expected | got | result |"
"|---|---|---|---|"
$rows
