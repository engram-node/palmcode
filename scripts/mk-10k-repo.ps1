# mk-10k-repo.ps1 — deterministic ~10k-file tree for M2-gap item 2 device runs.
# Content derives ONLY from the index (no RNG): byte-identical across runs.
# Usage: scripts/mk-10k-repo.ps1 -Out C:\path\to\bench10k [-Count 10000]
param([Parameter(Mandatory = $true)][string]$Out, [int]$Count = 10000)

$exts = @("rs", "py", "md", "json", "txt")
if (Test-Path $Out) { Remove-Item -Recurse -Force $Out }
for ($i = 0; $i -lt $Count; $i++) {
    $dir = Join-Path $Out ("src/mod{0:d3}" -f ($i % 200))
    New-Item -ItemType Directory -Force $dir | Out-Null
    $ext = $exts[$i % $exts.Count]
    $f = Join-Path $dir ("f{0:d5}.{1}" -f $i, $ext)
    $lines = 20 + ($i % 61)   # 20..80 lines
    $sb = [System.Text.StringBuilder]::new()
    for ($l = 0; $l -lt $lines; $l++) {
        switch ($ext) {
            "rs"   { [void]$sb.AppendLine(("// line {0} of f{1}; fn handle_request_{1}_{0}(x: u32) -> u32 {{ x + {0} }}" -f $l, $i)) }
            "py"   { [void]$sb.AppendLine(("# line {0} of f{1}: def handle_request_{1}_{0}(x): return x + {0}" -f $l, $i)) }
            "md"   { [void]$sb.AppendLine(("- item {0} of doc f{1}: see handle_request_{1}_{0} for details" -f $l, $i)) }
            "json" { [void]$sb.AppendLine(('  "line_{0}": {{"file": "f{1}", "ref": "handle_request_{1}_{0}"}},' -f $l, $i)) }
            default { [void]$sb.AppendLine(("plain text line {0} of f{1} mentioning handle_request_{1}_{0}" -f $l, $i)) }
        }
    }
    [System.IO.File]::WriteAllText($f, $sb.ToString())
}
$files = (Get-ChildItem $Out -Recurse -File).Count
$bytes = (Get-ChildItem $Out -Recurse -File | Measure-Object Length -Sum).Sum
"generated $files files, $([math]::Round($bytes/1MB,1)) MB at $Out"
