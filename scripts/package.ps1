# Builds the crest release zip for Windows from an already compiled binary.
#
#   cargo build --release --locked --target x86_64-pc-windows-msvc -p crest
#   pwsh scripts/package.ps1 -Target x86_64-pc-windows-msvc -Version 0.12.0
#
# Produces <Out>/crest-<version>-<target>.zip and its checksum <file>.sha256
# in the `sha256sum -c` format (lowercase hash, two spaces, LF), like
# scripts/package.sh does for Linux and macOS. The zip holds, at its root,
# crest.exe, install.ps1 (scripts/install.ps1), README.txt with the runtime
# requirements, LICENSE and THIRD-PARTY.md.
#
# With -Installer it also builds <Out>/crest-<version>-<target>-setup.exe
# (and its .sha256) from the same files with Inno Setup 6
# (packaging/windows/crest.iss; ISCC.exe from the Inno Setup folder or PATH).

param(
    [Parameter(Mandatory = $true)][string]$Target,
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$Out = "dist",
    [switch]$Installer
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
# What install.ps1 and the setup copy into the config folder when it is
# missing there.
$themes = Join-Path $dir "config\themes"
New-Item -ItemType Directory -Force -Path $themes | Out-Null
Copy-Item assets/init.lua (Join-Path $dir "config\init.lua")
Copy-Item examples/themes/*.lua $themes

$readme = @"
crest $Version ($Target)

Installation: run install.ps1 from this folder (no administrator rights):

    powershell -ExecutionPolicy Bypass -File .\install.ps1

It puts crest in %LOCALAPPDATA%\Programs\crest, adds that folder to your
user PATH and creates a Start menu shortcut; install.ps1 -Uninstall removes
them. Or run crest.exe from anywhere.

crest keeps its config in %APPDATA%\crest (init.lua, plugins, themes) and
its data in %LOCALAPPDATA%\crest; XDG_CONFIG_HOME and XDG_DATA_HOME override
them. The installer copies config\ (a commented init.lua and the example
color themes) there, skipping every file that already exists.
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
# The files at the root of the zip, no folder: the portable build is just
# crest.exe and its notes.
Compress-Archive -Path (Join-Path $dir "*") -DestinationPath $zipPath

function Write-Checksum([string]$File) {
    $hash = (Get-FileHash -Algorithm SHA256 $File).Hash.ToLowerInvariant()
    $leaf = Split-Path -Leaf $File
    [System.IO.File]::WriteAllText("$File.sha256", "$hash  $leaf`n")
}
Write-Checksum $zipPath

if ($Installer) {
    $iscc = @(
        (Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"),
        (Join-Path $env:ProgramFiles "Inno Setup 6\ISCC.exe"),
        (Get-Command ISCC.exe -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source)
    ) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
    if (-not $iscc) { throw "package.ps1: ISCC.exe (Inno Setup 6) not found" }
    # VERSIONINFO wants four numbers: 1.2.3-rc.1 -> 1.2.3.0.
    $numbers = @($Version -split '[.+-]' | Select-Object -First 3 | ForEach-Object {
        $n = 0; if ([int]::TryParse($_, [ref]$n)) { $n } else { 0 } })
    while ($numbers.Count -lt 4) { $numbers += 0 }
    $setupName = "$name-setup"
    & $iscc /Qp "/DAppVersion=$Version" "/DAppVersionNumbers=$($numbers -join '.')" `
        "/DSourceDir=$dir" "/DOutDir=$((Resolve-Path $Out).Path)" "/DOutputName=$setupName" `
        (Join-Path $root "packaging/windows/crest.iss")
    if ($LASTEXITCODE -ne 0) { throw "package.ps1: ISCC failed ($LASTEXITCODE)" }
    Write-Checksum (Join-Path $Out "$setupName.exe")
}

Remove-Item -Recurse -Force $staging
Write-Output $zipPath
