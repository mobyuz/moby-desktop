<#
.SYNOPSIS
  Proves that a built Moby binary does or does not contain the development-only code.

.DESCRIPTION
  The robot simulator (src-tauri/src/robot/sim.rs) and the PC microphone path
  (src-tauri/src/voice/mic.rs) each hold a marker string that exists in a binary only if that
  file was compiled in. A release build must contain neither (ADR 0006 and 0007).

  Default: fail if either marker is found. This is what CI runs on the release build.
  -ExpectPresent: fail if a marker is missing. Run it on a debug or test binary to prove the
  check can see the markers at all (a check that can never fail proves nothing).

.EXAMPLE
  cargo build --release --locked        # in src-tauri
  pwsh scripts/check-release-excludes-dev.ps1 src-tauri/target/release/moby-desktop.exe

.EXAMPLE
  pwsh scripts/check-release-excludes-dev.ps1 -ExpectPresent src-tauri/target/debug/deps/moby_desktop_lib-<hash>.exe
#>
param(
  [Parameter(Mandatory = $true, Position = 0)][string]$Binary,
  [switch]$ExpectPresent
)
$ErrorActionPreference = 'Stop'
if (-not (Test-Path -LiteralPath $Binary)) { throw "No such file: $Binary" }
$markers = [ordered]@{
  'simulator'     = 'moby-simulator-compiled-7f3a91c2'
  'pc microphone' = 'moby-pc-microphone-compiled-3c9e51d7'
}
# Latin-1 maps every byte to one character, so a byte search is a string search.
$text = [Text.Encoding]::GetEncoding(28591).GetString([IO.File]::ReadAllBytes((Resolve-Path -LiteralPath $Binary)))
$bad = 0
foreach ($name in $markers.Keys) {
  $found = $text.Contains($markers[$name])
  $ok = if ($ExpectPresent) { $found } else { -not $found }
  Write-Host ("{0,-14} marker {1}: {2}" -f $name, $(if ($found) { 'present' } else { 'absent' }), $(if ($ok) { 'ok' } else { 'FAIL' }))
  if (-not $ok) { $bad++ }
}
if ($bad -gt 0) { exit 1 }
