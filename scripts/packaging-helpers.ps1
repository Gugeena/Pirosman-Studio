# Helpers for package-windows.ps1 (dot-sourced). Functions throw on failure.

# Return one line per forbidden local path found in the given binaries.
# Each path is searched case-insensitively as ASCII/UTF-8 and as UTF-16LE,
# with "\" and "/" separators, with "\\" (as in escaped strings), and without
# its drive letter (e.g. "/c/Users/..." or "\\?\C:\Users\...").
function Find-PathLeaks([string[]]$Path, [string[]]$Forbidden) {
    $latin1 = [Text.Encoding]::GetEncoding(28591)   # one char per byte, lossless
    $needles = @()
    foreach ($f in $Forbidden) {
        $p = "$f".TrimEnd('\', '/')
        if (-not $p) { continue }
        $forms = @($p, ($p -replace '\\', '/'), ($p -replace '\\', '\\'))
        if ($p -match '^[A-Za-z]:(.+)$') {
            $rest = $Matches[1]
            $forms += $rest, ($rest -replace '\\', '/'), ($rest -replace '\\', '\\')
        }
        foreach ($form in ($forms | Select-Object -Unique)) {
            foreach ($enc in @(@{ n = "UTF-8"; e = [Text.Encoding]::UTF8 }, @{ n = "UTF-16LE"; e = [Text.Encoding]::Unicode })) {
                $needles += [pscustomobject]@{ Path = $f; Form = $form; Encoding = $enc.n; Text = $latin1.GetString($enc.e.GetBytes($form)).ToLowerInvariant() }
            }
        }
    }
    $leaks = @()
    foreach ($file in $Path) {
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "path-leak check: $file not found" }
        $hay = $latin1.GetString([IO.File]::ReadAllBytes((Resolve-Path -LiteralPath $file).Path)).ToLowerInvariant()
        foreach ($n in $needles) {
            $i = $hay.IndexOf($n.Text, [StringComparison]::Ordinal)
            if ($i -ge 0) { $leaks += "$file contains $($n.Path) (as `"$($n.Form)`", $($n.Encoding), offset $i)" }
        }
    }
    return $leaks
}

# List a zip's entry names and sizes (System.IO.Compression, independent of tar).
function Get-ZipEntries([string]$Zip) {
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $z = [IO.Compression.ZipFile]::OpenRead($Zip)
    try { return @($z.Entries | ForEach-Object { [pscustomobject]@{ Name = $_.FullName; Length = $_.Length } }) } finally { $z.Dispose() }
}

# Zip the folder $Stage as <Zip> with one top-level folder named like $Stage,
# using Windows' bsdtar (standard "/" entry names; Compress-Archive in Windows
# PowerShell 5.1 writes "\"), then validate it with Test-PortableZip: every
# entry name, and each staged file present with the same size, nothing else.
function New-PortableZip([string]$Stage, [string]$Zip) {
    $tar = Join-Path $env:SystemRoot "System32\tar.exe"
    if (-not (Test-Path $tar)) { throw "$tar not found (Windows 10 1803 or later ships it)" }
    $stageItem = Get-Item -LiteralPath $Stage
    $top = $stageItem.Name
    if (Test-Path -LiteralPath $Zip) { Remove-Item -LiteralPath $Zip -Force }
    Push-Location $stageItem.Parent.FullName
    try { & $tar -a -c -f $Zip $top } finally { Pop-Location }
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $Zip)) { throw "tar.exe failed to create $Zip (exit $LASTEXITCODE)" }
    Test-PortableZip -Stage $Stage -Zip $Zip
}

function Test-PortableZip([string]$Stage, [string]$Zip) {
    $stageItem = Get-Item -LiteralPath $Stage
    $top = $stageItem.Name
    $entries = Get-ZipEntries $Zip
    foreach ($e in $entries) {
        if ($e.Name.Contains('\')) { throw "zip entry uses a backslash: $($e.Name)" }
        if (-not $e.Name.StartsWith("$top/", [StringComparison]::Ordinal)) { throw "zip entry outside `"$top/`": $($e.Name)" }
        if ($e.Name -match '(^|/)\.\.(/|$)' -or $e.Name -match '^(/|[A-Za-z]:)') { throw "unsafe zip entry: $($e.Name)" }
    }
    $files = @($entries | Where-Object { -not $_.Name.EndsWith('/') })
    $staged = @(Get-ChildItem -LiteralPath $stageItem.FullName -Recurse -File)
    foreach ($f in $staged) {
        $name = "$top/" + $f.FullName.Substring($stageItem.FullName.Length + 1).Replace('\', '/')
        $hit = $files | Where-Object { $_.Name -ceq $name }
        if (-not $hit) { throw "zip lacks $name" }
        if ($hit.Length -ne $f.Length) { throw "zip entry $name is $($hit.Length) bytes, staged file is $($f.Length)" }
    }
    if ($files.Count -ne $staged.Count) { throw "zip has $($files.Count) files, staged folder has $($staged.Count)" }
}
