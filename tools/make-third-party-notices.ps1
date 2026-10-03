# Regenerates THIRD-PARTY-NOTICES.md from the components Werk bundles directly:
# the npm runtime dependencies and the workspace crates' Rust dependencies.
# Run from the repo root: pwsh tools/make-third-party-notices.ps1
$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $root

$licenseFilePattern = '^(LICEN[CS]E|COPYING|NOTICE|UNLICENSE)'

function Get-LicenseFiles([string]$dir) {
    if (!$dir -or !(Test-Path -LiteralPath $dir)) { return @() }
    Get-ChildItem -LiteralPath $dir -File -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match $licenseFilePattern -and $_.Length -lt 200000 }
}

function Get-Copyright([string]$dir) {
    $out = @()
    foreach ($f in Get-LicenseFiles $dir) {
        $text = Get-Content -LiteralPath $f.FullName -Raw -ErrorAction SilentlyContinue
        if (!$text) { continue }
        foreach ($m in [regex]::Matches($text, '(?im)^\s*(copyright[^\r\n]{2,160})$')) {
            $line = ($m.Groups[1].Value -replace '\s+', ' ').Trim()
            if ($line -match '(?i)\b(notice|license|law|permission|reproduce|derivative|attached|included|owner|infringement)\b') { continue }
            $out += $line
        }
        if ($out.Count -gt 0) { break }
    }
    $out | Select-Object -First 2 -Unique
}

# License-text detection: one pass over every local license file, first text wins.
$detectors = [ordered]@{
    "MIT"                 = 'Permission is hereby granted, free of charge'
    "ISC"                 = "Permission to use, copy, modify, and/or distribute this software"
    "BSD-3-Clause"        = 'Neither the name of'
    "BSD-2-Clause"        = 'Redistributions of source code must retain'
    "Apache-2.0"          = 'Apache License\s+Version 2\.0'
    "MPL-2.0"             = 'Mozilla Public License Version 2\.0'
    "Unicode-3.0"         = 'UNICODE LICENSE V3'
    "Zlib"                = "This software is provided 'as-is'"
    "CDLA-Permissive-2.0" = 'Community Data License Agreement'
    "Unlicense"           = 'free and unencumbered software released into the public domain'
    "BSL-1.0"             = 'Boost Software License'
    "0BSD"                = 'Permission to use, copy, modify, and/or distribute this software for any purpose'
    "CC0-1.0"             = 'CC0 1\.0 Universal'
    "LLVM-exception"      = 'LLVM Exceptions to the Apache 2\.0 License'
}
$texts = @{}

function Scan-LicenseTexts([string]$dir) {
    foreach ($f in Get-LicenseFiles $dir) {
        $text = Get-Content -LiteralPath $f.FullName -Raw -ErrorAction SilentlyContinue
        if (!$text) { continue }
        foreach ($id in $detectors.Keys) {
            if (!$texts.ContainsKey($id) -and $text -match $detectors[$id]) {
                $texts[$id] = $text.Trim()
            }
        }
    }
}

function Split-LicenseIds([string]$expr) {
    if (!$expr) { return @() }
    $clean = $expr -replace '[()\[\]]', ' ' -replace '/', ' OR '
    $clean -split '\s+(?:OR|AND|WITH)\s+' | ForEach-Object { $_.Trim() } | Where-Object { $_ }
}

# ── npm runtime dependencies (direct) ────────────────────────────────────────
$pkg = Get-Content "package.json" -Raw | ConvertFrom-Json
$npm = @()
foreach ($name in $pkg.dependencies.PSObject.Properties.Name) {
    $dir = Join-Path "node_modules" $name
    $pj = Join-Path $dir "package.json"
    if (!(Test-Path $pj)) { continue }
    $j = Get-Content $pj -Raw | ConvertFrom-Json
    $lic = if ($j.license) { $j.license } else { "Unknown" }
    $npm += [pscustomobject]@{
        Name      = $name
        Version   = $j.version
        License   = $lic
        Copyright = (Get-Copyright $dir) -join "; "
    }
    Scan-LicenseTexts $dir
}
$npm = $npm | Sort-Object Name

# ── Rust dependencies (direct, runtime only) ─────────────────────────────────
$meta = cargo metadata --format-version 1 2>$null | ConvertFrom-Json -AsHashtable
$byId = @{}
foreach ($p in $meta.packages) { $byId[$p.id] = $p }
$registry = Get-ChildItem (Join-Path $env:USERPROFILE ".cargo\registry\src") -Directory -ErrorAction SilentlyContinue |
    Select-Object -First 1 -ExpandProperty FullName
$rust = @{}
foreach ($node in $meta.resolve.nodes) {
    if ($byId[$node.id].source) { continue }  # workspace members only
    foreach ($dep in $node.deps) {
        $kinds = @($dep.dep_kinds | Where-Object { !$_.kind })  # normal deps, not dev/build
        if ($kinds.Count -eq 0) { continue }
        $p = $byId[$dep.pkg]
        if (!$p.source) { continue }  # our own workspace crates
        $dir = if ($registry) { Join-Path $registry "$($p.name)-$($p.version)" } else { $null }
        $rust[$p.name] = [pscustomobject]@{
            Name      = $p.name
            Version   = $p.version
            License   = if ($p.license) { $p.license } else { "Unknown" }
            Copyright = (Get-Copyright $dir) -join "; "
        }
        Scan-LicenseTexts $dir
    }
}
$rust = $rust.Values | Sort-Object Name

# ── Render ───────────────────────────────────────────────────────────────────
$needed = @{}
foreach ($p in ($npm + $rust)) { foreach ($id in (Split-LicenseIds $p.License)) { $needed[$id] = $true } }

$out = New-Object System.Collections.Generic.List[string]
$out.Add("# Third-party notices")
$out.Add("")
$out.Add("Werk (<https://github.com/otacoo/werk>) is distributed under the Apache")
$out.Add("License 2.0 (see [LICENSE](LICENSE)). It bundles the components below;")
$out.Add("the full license text for each license follows the lists.")
$out.Add("")
$out.Add("Generated by ``tools/make-third-party-notices.ps1`` from the npm runtime")
$out.Add("dependencies and the workspace crates' direct Rust dependencies.")
$out.Add("")
$out.Add("## JavaScript packages")
$out.Add("")
$out.Add("| Package | Version | License |")
$out.Add("| --- | --- | --- |")
foreach ($p in $npm) { $out.Add("| $($p.Name) | $($p.Version) | $($p.License) |") }
$out.Add("")
$out.Add("## Rust crates")
$out.Add("")
$out.Add("| Crate | Version | License |")
$out.Add("| --- | --- | --- |")
foreach ($p in $rust) { $out.Add("| $($p.Name) | $($p.Version) | $($p.License) |") }
$out.Add("")
$out.Add("## Copyright notices")
$out.Add("")
foreach ($p in ($npm + $rust | Where-Object { $_.Copyright })) {
    $out.Add("- $($p.Name) $($p.Version) — $($p.Copyright)")
}
$out.Add("")
$out.Add("## License texts")
$out.Add("")
$order = @("MIT", "ISC", "BSD-3-Clause", "BSD-2-Clause", "Apache-2.0", "MPL-2.0", "Unicode-3.0", "Zlib", "CDLA-Permissive-2.0", "Unlicense", "0BSD", "BSL-1.0", "CC0-1.0", "LLVM-exception")
foreach ($id in $order) {
    if (!$needed.ContainsKey($id) -or !$texts.ContainsKey($id)) { continue }
    $out.Add("### $id")
    $out.Add("")
    $out.Add('```')
    $out.Add($texts[$id])
    $out.Add('```')
    $out.Add("")
}

$dest = Join-Path $root "THIRD-PARTY-NOTICES.md"
Set-Content -LiteralPath $dest -Value ($out -join "`n") -Encoding utf8NoBOM
Write-Output "wrote $dest ($($npm.Count) npm packages, $($rust.Count) crates, $($needed.Count) license ids)"
