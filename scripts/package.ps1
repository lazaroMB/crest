# Builds the crest release zip for Windows from an already compiled binary.
#
#   cargo build --release --locked --target x86_64-pc-windows-msvc -p crest
#   pwsh scripts/package.ps1 -Target x86_64-pc-windows-msvc -Version 0.12.0
#
# Produces <Out>/crest-<version>-<target>.zip and its checksum <file>.sha256
# in the `sha256sum -c` format (lowercase hash, two spaces, LF), like
# scripts/package.sh does for Linux and macOS. The zip holds a directory with
# crest.exe, install.ps1 (scripts/install.ps1), README.txt with the runtime
# requirements, LICENSE and THIRD-PARTY.md.

param(
    [Parameter(Mandatory = $true)][string]$Target,
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$Out = "dist"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "target" }
$exe = Join-Path $targetDir "$Target/release/crest.exe"
if (-not (Test-Path $exe)) {
    throw "package.ps1: $exe does not exist; build it first"
}

$name = "crest-$Version-$Target"
$staging = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
$dir = Join-Path $staging $name
New-Item -ItemType Directory -Force -Path $dir | Out-Null
New-Item -ItemType Directory -Force -Path $Out | Out-Null

Copy-Item $exe $dir
Copy-Item scripts/install.ps1 $dir
Copy-Item LICENSE, THIRD-PARTY.md $dir

$readme = @"
crest $Version ($Target)

Installation: run install.ps1 from this folder (no administrator rights):

    powershell -ExecutionPolicy Bypass -File .\install.ps1

It puts crest in %LOCALAPPDATA%\Programs\crest, adds that folder to your
user PATH and creates a Start menu shortcut; install.ps1 -Uninstall removes
them. Or run crest.exe from anywhere.

crest keeps its config in %APPDATA%\crest (init.lua, plugins) and its data
in %LOCALAPPDATA%\crest; XDG_CONFIG_HOME and XDG_DATA_HOME override them.
Keep ANTHROPIC_API_KEY in the Windows Credential Manager (/login in crest)
or in your user environment variables.

Runtime requirements
--------------------
- ripgrep (rg) 13.0 or newer: winget install BurntSushi.ripgrep.MSVC
- fzf 0.36 or newer: winget install junegunn.fzf
- git: winget install Git.Git
- Windows 10 1809 or newer (DirectX 11); curl.exe ships with Windows.
SQLite and TLS (rustls) are built into the binary.

/! commands and plan checks run in PowerShell (pwsh when installed, else
Windows PowerShell); app.opt.shell changes it.
"@
# LF line endings, as the Unix README.
[System.IO.File]::WriteAllText((Join-Path $dir "README.txt"), ($readme -replace "`r`n", "`n"))

$zip = "$name.zip"
$zipPath = Join-Path $Out $zip
if (Test-Path $zipPath) { Remove-Item $zipPath }
Compress-Archive -Path $dir -DestinationPath $zipPath
Remove-Item -Recurse -Force $staging

$hash = (Get-FileHash -Algorithm SHA256 $zipPath).Hash.ToLowerInvariant()
[System.IO.File]::WriteAllText((Join-Path $Out "$zip.sha256"), "$hash  $zip`n")

Write-Output $zipPath
