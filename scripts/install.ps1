# Installs crest on Windows for the current user: crest.exe in
# %LOCALAPPDATA%\Programs\crest, that folder in the user PATH and a Start
# menu shortcut. No administrator rights.
#
#   irm https://raw.githubusercontent.com/lazaroMB/crest/main/scripts/install.ps1 | iex
#   .\install.ps1                     # from an extracted release zip
#   .\install.ps1 -Version v0.12.0    # a given release
#   .\install.ps1 -Zip crest-0.12.0-x86_64-pc-windows-msvc.zip
#   .\install.ps1 -Uninstall
#
# Through `irm | iex` there are no parameters: CREST_VERSION picks the
# release (default: the latest). A downloaded zip is checked against its
# .sha256 before anything is installed.

param(
    [string]$Version = $env:CREST_VERSION,
    [string]$Zip = "",
    [switch]$Uninstall
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$Repo = "lazaroMB/crest"
$Target = "x86_64-pc-windows-msvc"
$InstallDir = Join-Path $env:LOCALAPPDATA "Programs\crest"
$Shortcut = Join-Path ([Environment]::GetFolderPath("Programs")) "crest.lnk"

function Set-UserPath([scriptblock]$Change) {
    $current = [Environment]::GetEnvironmentVariable("Path", "User")
    $entries = @(if ($current) { $current.Split(";") | Where-Object { $_ } })
    $updated = & $Change $entries
    [Environment]::SetEnvironmentVariable("Path", ($updated -join ";"), "User")
}

if ($Uninstall) {
    if (Test-Path $InstallDir) { Remove-Item -Recurse -Force $InstallDir }
    if (Test-Path $Shortcut) { Remove-Item -Force $Shortcut }
    Set-UserPath { param($entries) $entries | Where-Object { $_ -ne $InstallDir } }
    Write-Output "crest removed (your config in $env:APPDATA\crest and data in $env:LOCALAPPDATA\crest are kept)"
    return
}

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("crest-" + [System.Guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $work | Out-Null
try {
    # 1. Find crest.exe: next to this script, in -Zip, or in a download.
    $local = if ($PSScriptRoot) { Join-Path $PSScriptRoot "crest.exe" } else { "" }
    if (-not $Zip -and $local -and (Test-Path $local)) {
        $exe = $local
    } else {
        if (-not $Zip) {
            if (-not $Version) {
                $latest = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest"
                $Version = $latest.tag_name
            }
            $tag = if ($Version.StartsWith("v")) { $Version } else { "v$Version" }
            $name = "crest-$($tag.Substring(1))-$Target.zip"
            $base = "https://github.com/$Repo/releases/download/$tag"
            $Zip = Join-Path $work $name
            Write-Output "downloading $base/$name"
            Invoke-WebRequest "$base/$name" -OutFile $Zip
            Invoke-WebRequest "$base/$name.sha256" -OutFile "$Zip.sha256"
        }
        $sums = "$Zip.sha256"
        if (Test-Path $sums) {
            $expected = ((Get-Content $sums -Raw).Trim() -split "\s+")[0].ToLowerInvariant()
            $actual = (Get-FileHash -Algorithm SHA256 $Zip).Hash.ToLowerInvariant()
            if ($expected -ne $actual) {
                throw "install.ps1: checksum mismatch for $Zip (expected $expected, got $actual)"
            }
        } else {
            Write-Warning "no $sums next to the zip: the download is not verified"
        }
        Expand-Archive -Path $Zip -DestinationPath $work -Force
        $exe = Get-ChildItem -Path $work -Recurse -Filter crest.exe | Select-Object -First 1 -ExpandProperty FullName
        if (-not $exe) { throw "install.ps1: no crest.exe in $Zip" }
    }

    # 2. Install it, put it on the user PATH, add the Start menu shortcut.
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item -Force $exe (Join-Path $InstallDir "crest.exe")
    Set-UserPath {
        param($entries)
        if ($entries -contains $InstallDir) { $entries } else { @($entries) + $InstallDir }
    }
    $shell = New-Object -ComObject WScript.Shell
    $link = $shell.CreateShortcut($Shortcut)
    $link.TargetPath = Join-Path $InstallDir "crest.exe"
    $link.WorkingDirectory = $env:USERPROFILE
    $link.Description = "crest, a coding agent"
    $link.Save()

    Write-Output "crest installed in $InstallDir"
    Write-Output "open a new terminal to use the ``crest`` command, or start it from the Start menu"
    foreach ($tool in "rg", "fzf", "git") {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
            Write-Warning "$tool is not on PATH; see README.txt for how to install it"
        }
    }
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
