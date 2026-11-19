# toastset.ps1 — the native Windows installer for Rusk.
#
# Run from PowerShell on Windows 10 or 11:
#
#   powershell -ExecutionPolicy Bypass -File toastset.ps1
#
# Or fetch and run it in one line:
#
#   irm https://raw.githubusercontent.com/Rusk-programing-language/Rusk/main/toastset.ps1 | iex
#
# This is the same installer the POSIX `toastset` script delegates to on
# Windows. It keeps the original PowerShell flow: TLS 1.2, the latest release
# from the GitHub API, curl.exe for the tarball, tar.exe to unpack it, a user
# PATH update, and a question before creating the Startup-folder shortcut.
#
# Options may use either POSIX or PowerShell spelling:
#   --version 1.2.1 / -Version 1.2.1
#   --target TARGET / -Target TARGET
#   --home DIR / -Home DIR
#   --bin-dir DIR / -BinDir DIR
#   --no-verify / -NoVerify
#   --no-modify-path / -NoModifyPath
#   --no-startup / -NoStartup
#   --uninstall / -Uninstall
#
# Environment: TOASTSET_VERSION, TOASTSET_TARGET, TOASTSET_HOME,
# TOASTSET_BIN_DIR, TOASTSET_NO_MODIFY_PATH=1, TOASTSET_NO_STARTUP=1,
# TOASTSET_DOWNLOAD_BASE, and TOASTSET_API_URL.

# No param block: both POSIX (--version) and PowerShell (-Version) spellings
# arrive in $args, so the small parser below owns the syntax exactly.
$RawArgs = $args

[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$ErrorActionPreference = 'Stop'
$Repo = 'Rusk-programing-language/Rusk'
$BaseUrl = if ($env:TOASTSET_DOWNLOAD_BASE) { $env:TOASTSET_DOWNLOAD_BASE } else { "https://github.com/$Repo/releases/download" }
$ApiUrl = if ($env:TOASTSET_API_URL) { $env:TOASTSET_API_URL } else { "https://api.github.com/repos/$Repo/releases/latest" }

function Say-Info {
    param([string]$Message)
    Write-Host "toastset: $Message"
}

function Say-Warn {
    param([string]$Message)
    Write-Host "toastset: warning: $Message"
}

function Stop-Install {
    param([string]$Message)
    Write-Host "toastset: error: $Message"
    exit 1
}

function Show-Help {
    Write-Host @'
toastset — install the Rusk compiler and toolchain on Windows

USAGE:
  toastset.ps1 [OPTIONS]

OPTIONS:
  --version VERSION   install a specific release (for example 1.2.1)
  --target TARGET     install a specific target (default: x86_64-pc-windows-msvc)
  --home DIR          toolchain home (default: %LOCALAPPDATA%\Rusk)
  --bin-dir DIR       install rusk.exe into DIR
  --no-verify         skip SHA-256 verification
  --no-modify-path    do not change the Windows user PATH
  --no-startup        do not ask about the Windows Startup shortcut
  --uninstall         remove a previous installation
  -h, --help          show this help

PowerShell spellings without the leading -- work too, for example
-Version 1.2.1 and -NoStartup. curl.exe and tar.exe ship with Windows 10
version 1803 and newer.
'@
}

$Version = ''
$Target = ''
$ToolchainHome = ''
$BinDir = ''
$NoVerify = $false
$NoModifyPath = $false
$NoStartup = $false
$Uninstall = $false
$Help = $false

for ($i = 0; $i -lt $RawArgs.Count; $i++) {
    $arg = [string]$RawArgs[$i]
    switch -Regex ($arg) {
        '^(--version|-Version|-v)$' {
            if (++$i -ge $RawArgs.Count) { Stop-Install '--version needs a value' }
            $Version = [string]$RawArgs[$i]
        }
        '^(--target|-Target)$' {
            if (++$i -ge $RawArgs.Count) { Stop-Install '--target needs a value' }
            $Target = [string]$RawArgs[$i]
        }
        '^(--home|-Home)$' {
            if (++$i -ge $RawArgs.Count) { Stop-Install '--home needs a value' }
            $ToolchainHome = [string]$RawArgs[$i]
        }
        '^(--bin-dir|-BinDir)$' {
            if (++$i -ge $RawArgs.Count) { Stop-Install '--bin-dir needs a value' }
            $BinDir = [string]$RawArgs[$i]
        }
        '^(--no-verify|-NoVerify)$' { $NoVerify = $true }
        '^(--no-modify-path|-NoModifyPath)$' { $NoModifyPath = $true }
        '^(--no-startup|-NoStartup)$' { $NoStartup = $true }
        '^(--uninstall|-Uninstall)$' { $Uninstall = $true }
        '^(--help|-Help|-h)$' { $Help = $true }
        default { Stop-Install "I don't know '$arg' — run 'toastset.ps1 --help'" }
    }
}

if ($Help) {
    Show-Help
    exit 0
}

if (-not $Version) { $Version = $env:TOASTSET_VERSION }
if (-not $Target) { $Target = $env:TOASTSET_TARGET }
if (-not $ToolchainHome) { $ToolchainHome = $env:TOASTSET_HOME }
if (-not $BinDir) { $BinDir = $env:TOASTSET_BIN_DIR }
if ($env:TOASTSET_NO_MODIFY_PATH -eq '1') { $NoModifyPath = $true }
if ($env:TOASTSET_NO_STARTUP -eq '1') { $NoStartup = $true }

if (-not $Target) { $Target = 'x86_64-pc-windows-msvc' }
switch ($Target.ToLowerInvariant()) {
    'windows' { $Target = 'x86_64-pc-windows-msvc' }
    'windows-x86_64' { $Target = 'x86_64-pc-windows-msvc' }
    'win64' { $Target = 'x86_64-pc-windows-msvc' }
    'pe' { $Target = 'x86_64-pc-windows-msvc' }
}
if ($Target -ne 'x86_64-pc-windows-msvc') {
    Stop-Install "the native installer only supports x86_64-pc-windows-msvc (got '$Target'); use the POSIX toastset for other targets"
}

if (-not $ToolchainHome) { $ToolchainHome = Join-Path $env:LOCALAPPDATA 'Rusk' }
if (-not $BinDir) { $BinDir = $ToolchainHome }
$Exe = Join-Path $BinDir 'rusk.exe'
$VersionFile = Join-Path $BinDir 'version'

function Remove-StartupShortcut {
    try {
        $folder = [Environment]::GetFolderPath('Startup')
        if (-not $folder -or -not (Test-Path -LiteralPath $folder)) { return }
        $link = Join-Path $folder 'Rusk.lnk'
        if (Test-Path -LiteralPath $link) {
            Remove-Item -LiteralPath $link -Force
            Say-Info 'removed the Rusk shortcut from the Windows Startup folder'
        }
    } catch {
        Say-Warn "I couldn't remove the Rusk Startup shortcut ($($_.Exception.Message))"
    }
}

if ($Uninstall) {
    Remove-StartupShortcut
    if (Test-Path -LiteralPath $Exe) { Remove-Item -LiteralPath $Exe -Force }
    if (Test-Path -LiteralPath $VersionFile) { Remove-Item -LiteralPath $VersionFile -Force }
    Say-Info "removed Rusk from $BinDir"
    exit 0
}

$Tag = ''
if ($Version) {
    $Tag = if ($Version.StartsWith('v')) { $Version } else { "v$Version" }
} else {
    Say-Info 'looking up the latest Rusk release'
    try {
        $release = Invoke-RestMethod -Uri $ApiUrl -UseBasicParsing
        $Tag = [string]$release.tag_name
    } catch {
        Stop-Install "I couldn't reach GitHub to find the latest release; pass --version"
    }
    if (-not $Tag) { Stop-Install "I couldn't read the latest release from GitHub; pass --version" }
    if (-not $Tag.StartsWith('v')) { $Tag = "v$Tag" }
}
$Version = $Tag.TrimStart('v')

$Asset = "rusk-$Tag-$Target.tar.gz"
$Url = "$BaseUrl/$Tag/$Asset"
$Tarball = Join-Path $env:TEMP $Asset
$Sums = Join-Path $env:TEMP "toastset-$Tag-SHA256SUMS.txt"

Say-Info "downloading Rusk $Version for $Target"
& curl.exe -fsSL $Url -o $Tarball
if ($LASTEXITCODE -ne 0) { Stop-Install "I couldn't download $Url" }

if (-not $NoVerify) {
    & curl.exe -fsSL "$BaseUrl/$Tag/SHA256SUMS" -o $Sums
    if ($LASTEXITCODE -eq 0 -and (Test-Path -LiteralPath $Sums)) {
        $line = Get-Content -LiteralPath $Sums | Where-Object { $_ -match "\s$([regex]::Escape($Asset))$" } | Select-Object -First 1
        if ($line) {
            $expected = ($line -split '\s+')[0].ToLowerInvariant()
            $actual = (Get-FileHash -LiteralPath $Tarball -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($actual -ne $expected) { Stop-Install "the download's checksum doesn't match" }
            Say-Info 'checksum verified'
        } else {
            Say-Warn "no checksum for $Asset; continuing"
        }
    } else {
        Say-Warn 'SHA256SUMS is not available; continuing without verification'
    }
    if (Test-Path -LiteralPath $Sums) { Remove-Item -LiteralPath $Sums -Force }
}

$PreviousVersion = ''
if (Test-Path -LiteralPath $VersionFile) { $PreviousVersion = (Get-Content -LiteralPath $VersionFile -TotalCount 1).Trim() }
$Existing = Test-Path -LiteralPath $Exe

New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
& tar.exe -xzf $Tarball -C $BinDir --strip-components 1
if ($LASTEXITCODE -ne 0) { Stop-Install "I couldn't unpack $Asset" }
Remove-Item -LiteralPath $Tarball -Force
if (-not (Test-Path -LiteralPath $Exe)) { Stop-Install "the archive didn't contain rusk.exe" }
Set-Content -LiteralPath $VersionFile -Value $Version -NoNewline:$false

if (-not $NoModifyPath) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not $userPath) { $userPath = '' }
    $parts = @($userPath -split ';' | Where-Object { $_ })
    if ($parts -contains $BinDir) {
        Say-Info 'Rusk is already on the Windows user PATH'
    } else {
        if ($userPath) { $newPath = $userPath.TrimEnd([char]';') + ';' + $BinDir } else { $newPath = $BinDir }
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        Say-Info "added $BinDir to the Windows user PATH"
    }
    if ($env:Path -notlike "*$BinDir*") { $env:Path = "$env:Path;$BinDir" }
}

if (-not $NoStartup -and $env:TOASTSET_NO_STARTUP -ne '1') {
    $answer = ''
    try {
        $answer = Read-Host 'Add Rusk to the Windows Startup folder so the prompt opens at login? [y/N]'
    } catch {
        Say-Warn 'I could not ask about the Startup shortcut; skipping it'
    }
    if ($answer -match '^(y|yes)$') {
        try {
            $folder = [Environment]::GetFolderPath('Startup')
            if (-not $folder -or -not (Test-Path -LiteralPath $folder)) {
                Say-Warn "I couldn't find your Windows Startup folder; no shortcut was created"
            } else {
                $link = Join-Path $folder 'Rusk.lnk'
                $wsh = New-Object -ComObject WScript.Shell
                $shortcut = $wsh.CreateShortcut($link)
                $shortcut.TargetPath = $Exe
                $shortcut.WorkingDirectory = $BinDir
                $shortcut.Description = 'Rusk compiler prompt'
                $shortcut.IconLocation = "$Exe,0"
                $shortcut.Save()
                if (Test-Path -LiteralPath $link) {
                    Say-Info 'added a Rusk shortcut to the Windows Startup folder'
                } else {
                    Say-Warn 'I couldn't create the Startup shortcut'
                }
            }
        } catch {
            Say-Warn "I couldn't create the Startup shortcut ($($_.Exception.Message))"
        }
    } else {
        Say-Info 'skipped the Startup shortcut'
    }
}

if ($Existing) {
    if ($PreviousVersion -and $PreviousVersion -ne $Version) {
        Say-Info "reinstalled rusk $Version to $Exe (was $PreviousVersion)"
    } else {
        Say-Info "reinstalled rusk $Version to $Exe"
    }
} else {
    Say-Info "installed rusk $Version to $Exe"
}

Write-Host ''
Write-Host 'Try it:'
Write-Host '  rusk version'
& $Exe version
