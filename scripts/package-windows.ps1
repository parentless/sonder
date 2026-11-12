param(
    [ValidateSet('debug', 'release')][string]$Profile = 'release',
    [ValidatePattern('^[a-z0-9][a-z0-9-]{0,63}$')][string]$Label
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    $metadata = cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
    $binary = Join-Path $metadata.target_directory "x86_64-pc-windows-msvc/$Profile/cadence.exe"
    if (!(Test-Path -LiteralPath $binary)) {
        throw "Build first: cargo build --release --locked --target x86_64-pc-windows-msvc"
    }
    # Releases publish exactly one file, Sonder.exe, plus its checksum. Debug and
    # labelled builds get their own names so they never replace it.
    $name = 'Sonder'
    if ($Profile -eq 'debug') { $name += '-debug' }
    if ($Label) { $name += "-$Label" }
    New-Item -ItemType Directory -Path 'dist' -Force | Out-Null
    $file = "dist/$name.exe"
    Copy-Item -LiteralPath $binary -Destination $file -Force
    $hash = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $name.exe`n" | Set-Content -LiteralPath "$file.sha256" -Encoding ascii -NoNewline
    Write-Host "Packaged $file"
} finally { Pop-Location }
