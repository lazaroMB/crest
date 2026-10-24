# Installs crest on Windows for the current user, as an app: the release's
# setup (packaging/windows/crest.iss) run silently puts crest.exe in
# %LOCALAPPDATA%\Programs\crest, adds a Start menu entry, the folder to the
# user PATH and crest to Settings > Apps (with its uninstaller). No
# administrator rights.
#
#   irm https://raw.githubusercontent.com/lazaroMB/crest/main/scripts/install.ps1 | iex
#   .\install.ps1                     # from an extracted release zip
#   .\install.ps1 -Version v0.14.0    # a given release
#   .\install.ps1 -Setup crest-0.14.0-x86_64-pc-windows-msvc-setup.exe
#   .\install.ps1 -Zip crest-0.14.0-x86_64-pc-windows-msvc.zip
#   .\install.ps1 -Uninstall
#
# Through `irm | iex` there are no parameters: CREST_VERSION picks the
# release (default: the latest). A download is checked against its .sha256
# before anything runs. Releases before v0.14.0 have no setup: their zip
# is installed by copying crest.exe and creating the shortcut here (no
# entry in Settings > Apps), as are -Zip and an extracted zip.

param(
    [string]$Version = $env:CREST_VERSION,
    [string]$Setup = "",
    [string]$Zip = "",
    [switch]$Uninstall
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$Repo = "lazaroMB/crest"
$Target = "x86_64-pc-windows-msvc"
$InstallDir = Join-Path $env:LOCALAPPDATA "Programs\crest"
$Shortcut = Join-Path ([Environment]::GetFolderPath("Programs")) "crest.lnk"
# Written by the setup, next to crest.exe, and its entry in Settings > Apps
# (the AppId of packaging/windows/crest.iss).
$Uninstaller = Join-Path $InstallDir "unins000.exe"
$AppsEntry = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{9C1A9DD2-A2E2-4F50-BA98-5C1E587342BE}_is1"

function Set-UserPath([scriptblock]$Change) {
    $current = [Environment]::GetEnvironmentVariable("Path", "User")
    $entries = @(if ($current) { $current.Split(";") | Where-Object { $_ } })
    $updated = & $Change $entries
    [Environment]::SetEnvironmentVariable("Path", ($updated -join ";"), "User")
}

function Assert-Checksum([string]$File) {
    $sums = "$File.sha256"
    if (-not (Test-Path $sums)) {
        Write-Warning "no $sums next to $File`: it is not verified"
        return
    }
    $expected = ((Get-Content $sums -Raw).Trim() -split "\s+")[0].ToLowerInvariant()
    $actual = (Get-FileHash -Algorithm SHA256 $File).Hash.ToLowerInvariant()
    if ($expected -ne $actual) {
        throw "install.ps1: checksum mismatch for $File (expected $expected, got $actual)"
    }
}

# Downloads <name> and <name>.sha256 of the release into $Dir; $false when
# the release has no such file.
function Get-ReleaseFile([string]$Tag, [string]$Name, [string]$Dir) {
    $base = "https://github.com/$Repo/releases/download/$Tag"
    try {
        Invoke-WebRequest "$base/$Name.sha256" -OutFile (Join-Path $Dir "$Name.sha256")
    } catch {
        return $false
    }
    Write-Output "downloading $base/$Name" | Out-Host
    Invoke-WebRequest "$base/$Name" -OutFile (Join-Path $Dir $Name)
    return $true
}

function Install-Setup([string]$File) {
    Assert-Checksum $File
    $arguments = "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CURRENTUSER", "/TASKS=addtopath"
    $process = Start-Process -FilePath $File -ArgumentList $arguments -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "install.ps1: the setup failed (exit code $($process.ExitCode))" }
}

# crest's config folder, as crest finds it: $env:XDG_CONFIG_HOME\crest
# when it is absolute, else %APPDATA%\crest.
function Get-ConfigDir {
    if ($env:XDG_CONFIG_HOME -and [System.IO.Path]::IsPathRooted($env:XDG_CONFIG_HOME)) {
        return Join-Path $env:XDG_CONFIG_HOME "crest"
    }
    Join-Path $env:APPDATA "crest"
}

# Copies the config\ next to crest.exe (a commented init.lua and the
# example color themes) into the config folder, never replacing a file
# that exists. Zips before v0.15.0 have none: nothing to do.
function Copy-ConfigSeed([string]$From) {
    $seed = Join-Path $From "config"
    if (-not (Test-Path $seed)) { return }
    $config = Get-ConfigDir
    $themes = Join-Path $config "themes"
    New-Item -ItemType Directory -Force -Path $themes | Out-Null
    $init = Join-Path $config "init.lua"
    if (-not (Test-Path $init) -and (Test-Path (Join-Path $seed "init.lua"))) {
        Copy-Item (Join-Path $seed "init.lua") $init
        Write-Output "created $init (every option commented out)"
    }
    $added = 0
    foreach ($theme in Get-ChildItem -Path (Join-Path $seed "themes") -Filter *.lua -ErrorAction SilentlyContinue) {
        $target = Join-Path $themes $theme.Name
        if (-not (Test-Path $target)) {
            Copy-Item $theme.FullName $target
            $added++
        }
    }
    if ($added -gt 0) { Write-Output "added $added color themes to $themes (pick one with /theme)" }
}

# The installs of releases before v0.14.0: crest.exe, the shortcut and the
# PATH entry made by this script.
function Install-Exe([string]$Exe) {
    Copy-ConfigSeed (Split-Path -Parent $Exe)
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item -Force $Exe (Join-Path $InstallDir "crest.exe")
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
}

function Install-Zip([string]$File, [string]$Work) {
    Assert-Checksum $File
    Expand-Archive -Path $File -DestinationPath $Work -Force
    $exe = Get-ChildItem -Path $Work -Recurse -Filter crest.exe | Select-Object -First 1 -ExpandProperty FullName
    if (-not $exe) { throw "install.ps1: no crest.exe in $File" }
    Install-Exe $exe
}

if ($Uninstall) {
    if (Test-Path $Uninstaller) {
        $process = Start-Process -FilePath $Uninstaller -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART" -Wait -PassThru
        if ($process.ExitCode -ne 0) { throw "install.ps1: the uninstaller failed (exit code $($process.ExitCode))" }
        # It runs from a copy in %TEMP% and returns at once: wait for it to
        # remove its entry in Settings > Apps.
        for ($i = 0; $i -lt 60 -and (Test-Path $AppsEntry); $i++) { Start-Sleep -Milliseconds 500 }
        if (Test-Path $AppsEntry) { throw "install.ps1: the uninstaller did not finish" }
    }
    # What an install by this script before v0.14.0 left.
    if (Test-Path $InstallDir) { Remove-Item -Recurse -Force $InstallDir }
    if (Test-Path $Shortcut) { Remove-Item -Force $Shortcut }
    Set-UserPath { param($entries) $entries | Where-Object { $_ -ne $InstallDir } }
    Write-Output "crest removed (your config in $env:APPDATA\crest and data in $env:LOCALAPPDATA\crest are kept)"
    return
}

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("crest-" + [System.Guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $work | Out-Null
try {
    $local = if ($PSScriptRoot) { Join-Path $PSScriptRoot "crest.exe" } else { "" }
    if ($Setup) {
        Install-Setup $Setup
    } elseif ($Zip) {
        Install-Zip $Zip (Join-Path $work "zip")
    } elseif ($local -and (Test-Path $local)) {
        Install-Exe $local
    } else {
        if (-not $Version) {
            $latest = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest"
            $Version = $latest.tag_name
        }
        $tag = if ($Version.StartsWith("v")) { $Version } else { "v$Version" }
        $stem = "crest-$($tag.Substring(1))-$Target"
        if (Get-ReleaseFile $tag "$stem-setup.exe" $work) {
            Install-Setup (Join-Path $work "$stem-setup.exe")
        } elseif (Get-ReleaseFile $tag "$stem.zip" $work) {
            Install-Zip (Join-Path $work "$stem.zip") (Join-Path $work "zip")
        } else {
            throw "install.ps1: release $tag has no Windows build"
        }
    }

    Write-Output "crest installed in $InstallDir"
    Write-Output "start it from the Start menu, or open a new terminal to use the ``crest`` command"
    foreach ($tool in "rg", "fzf", "git") {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
            Write-Warning "$tool is not on PATH: winget install BurntSushi.ripgrep.MSVC junegunn.fzf Git.Git"
        }
    }
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
