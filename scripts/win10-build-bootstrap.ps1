#Requires -RunAsAdministrator
<#
.SYNOPSIS
  Bootstrap a clean Win10 VM to compile backup-sync-tool.

.DESCRIPTION
  Installs Git for Windows, VS 2022 Build Tools (Desktop C++), and Rust (rustup).
  Creates a dedicated GitHub SSH key (does not overwrite existing keys).
  Optionally clones the repo. Nightly/rust-src are left to build-local.ps1.

.NOTES
  Run as Administrator. Log: Desktop\win10-bootstrap.log

  Paste one-liner (Admin PowerShell; Proxmox HTTP on vmbr1):
    Set-ExecutionPolicy Bypass -Scope Process -Force; irm http://10.10.10.1:8765/win10-build-bootstrap.ps1 | iex
#>

$ErrorActionPreference = 'Stop'

# --- config ---
$RepoSshUrl   = if ($env:REPO_URL) { $env:REPO_URL } else { 'git@github.com:ruibeard/backup-sync-tool.git' }
$RepoHttpsUrl = 'https://github.com/ruibeard/backup-sync-tool.git'
$CloneDir     = if ($env:CLONE_DIR) { $env:CLONE_DIR } else { Join-Path $env:USERPROFILE 'code\backup-sync-tool' }
$SshKeyName   = 'id_ed25519_github_win10'
$SshKeyPath   = Join-Path $env:USERPROFILE ".ssh\$SshKeyName"
$SshPubPath   = "$SshKeyPath.pub"
$WorkDir      = Join-Path $env:TEMP 'win10-build-bootstrap'
$LogPath      = Join-Path ([Environment]::GetFolderPath('Desktop')) 'win10-bootstrap.log'
$VsBuildToolsUrl = 'https://aka.ms/vs/17/release/vs_buildtools.exe'
$GitInstallerUrl = 'https://github.com/git-for-windows/git/releases/download/v2.47.1.windows.1/Git-2.47.1-64-bit.exe'
$RustupUrl       = 'https://win.rustup.rs/x86_64'

function Write-Log {
    param([string]$Message, [string]$Level = 'INFO')
    $line = '[{0}] [{1}] {2}' -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $Level, $Message
    Add-Content -Path $LogPath -Value $line -Encoding UTF8
    switch ($Level) {
        'ERROR' { Write-Host $Message -ForegroundColor Red }
        'WARN'  { Write-Host $Message -ForegroundColor Yellow }
        'OK'    { Write-Host $Message -ForegroundColor Green }
        default { Write-Host $Message }
    }
}

function Test-IsAdmin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($id)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Refresh-SessionPath {
    Write-Log 'Refreshing PATH from Machine + User + common tool locations...'
    $machine = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    $user    = [Environment]::GetEnvironmentVariable('Path', 'User')
    $extra = @(
        'C:\Program Files\Git\cmd',
        'C:\Program Files\Git\bin',
        (Join-Path $env:USERPROFILE '.cargo\bin'),
        'C:\Program Files (x86)\Microsoft Visual Studio\Installer',
        'C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC'
    ) -join ';'
    $env:PATH = (@($machine, $user, $extra) | Where-Object { $_ }) -join ';'

    # Prefer newest MSVC Hostx64\x64 bin on PATH if present
    $msvcRoot = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC'
    if (-not (Test-Path $msvcRoot)) {
        $msvcRoot = 'C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC'
    }
    if (Test-Path $msvcRoot) {
        $hostBin = Get-ChildItem -Path $msvcRoot -Directory -ErrorAction SilentlyContinue |
            Sort-Object Name -Descending |
            ForEach-Object { Join-Path $_.FullName 'bin\Hostx64\x64' } |
            Where-Object { Test-Path $_ } |
            Select-Object -First 1
        if ($hostBin) {
            $env:PATH = "$hostBin;$env:PATH"
        }
    }
}

function Download-File {
    param([string]$Url, [string]$OutFile)
    Write-Log "Downloading $Url"
    Write-Log "  -> $OutFile"
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -Uri $Url -OutFile $OutFile -UseBasicParsing
}

function Ensure-Command {
    param([string]$Name)
    return [bool](Get-Command $Name -ErrorAction SilentlyContinue)
}

# --- start ---
if (-not (Test-IsAdmin)) {
    Write-Host 'ERROR: Run this script as Administrator (right-click PowerShell -> Run as administrator).' -ForegroundColor Red
    exit 1
}

New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null
New-Item -ItemType Directory -Force -Path (Split-Path $LogPath) | Out-Null
'' | Set-Content -Path $LogPath -Encoding UTF8
Write-Log '=== Win10 build bootstrap starting ==='
Write-Log "WorkDir=$WorkDir"
Write-Log "Log=$LogPath"

# -------------------------------------------------------------------------
# 1) Git for Windows
# -------------------------------------------------------------------------
Refresh-SessionPath
if (Ensure-Command 'git') {
    Write-Log "Git already present: $(git --version)" 'OK'
} else {
    Write-Log 'Installing Git for Windows (silent)...'
    $gitExe = Join-Path $WorkDir 'Git-Installer.exe'
    Download-File -Url $GitInstallerUrl -OutFile $gitExe
    $gitArgs = '/VERYSILENT /NORESTART /NOCANCEL /SP- /CLOSEAPPLICATIONS /RESTARTAPPLICATIONS /COMPONENTS="icons,ext\reg\shellhere,assoc,assoc_sh" /o:PathOption=Cmd'
    $p = Start-Process -FilePath $gitExe -ArgumentList $gitArgs -Wait -PassThru
    if ($p.ExitCode -ne 0) {
        Write-Log "Git installer exited with code $($p.ExitCode)" 'ERROR'
        exit 1
    }
    Refresh-SessionPath
    if (-not (Ensure-Command 'git')) {
        Write-Log 'Git installed but git.exe not on PATH. Open a new Admin PowerShell and re-run.' 'ERROR'
        exit 1
    }
    Write-Log "Git installed: $(git --version)" 'OK'
}

# -------------------------------------------------------------------------
# 2) VS 2022 Build Tools — Desktop C++ / VCTools
# -------------------------------------------------------------------------
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$haveVc = $false
if (Test-Path $vswhere) {
    $vsPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null |
        Select-Object -First 1
    if ($vsPath) { $haveVc = $true }
}

if ($haveVc) {
    Write-Log "VS Build Tools / VC Tools already present: $vsPath" 'OK'
} else {
    Write-Log 'Downloading VS 2022 Build Tools bootstrapper (this can take a while)...'
    $vsExe = Join-Path $WorkDir 'vs_buildtools.exe'
    Download-File -Url $VsBuildToolsUrl -OutFile $vsExe

    Write-Log 'Installing VS 2022 Build Tools workload Desktop development with C++ (silent, long)...'
    Write-Log 'Progress: VS installer window may appear; wait until it finishes.'
    $vsArgs = @(
        '--quiet', '--wait', '--norestart', '--nocache',
        '--installPath', 'C:\BuildTools',
        '--add', 'Microsoft.VisualStudio.Workload.VCTools',
        '--includeRecommended'
    )
    $p = Start-Process -FilePath $vsExe -ArgumentList $vsArgs -Wait -PassThru
    # VS installer: 0 = success, 3010 = reboot required
    if ($p.ExitCode -notin 0, 3010) {
        Write-Log "VS Build Tools installer exited with code $($p.ExitCode)" 'ERROR'
        Write-Log 'See %TEMP%\dd_bootstrapper_*.log and Desktop\win10-bootstrap.log' 'ERROR'
        exit 1
    }
    if ($p.ExitCode -eq 3010) {
        Write-Log 'VS installer requested reboot (3010). Continue after reboot if link.exe is missing.' 'WARN'
    }
    Refresh-SessionPath
    Write-Log 'VS 2022 Build Tools install finished.' 'OK'
}

# -------------------------------------------------------------------------
# 3) Rust via rustup (default MSVC host)
# -------------------------------------------------------------------------
Refresh-SessionPath
if (Ensure-Command 'rustc') {
    Write-Log "Rust already present: $(rustc --version)" 'OK'
} else {
    Write-Log 'Installing Rust via rustup-init (-y, default MSVC host)...'
    $rustupExe = Join-Path $WorkDir 'rustup-init.exe'
    Download-File -Url $RustupUrl -OutFile $rustupExe
    $p = Start-Process -FilePath $rustupExe -ArgumentList '-y', '--default-host', 'x86_64-pc-windows-msvc' -Wait -PassThru
    if ($p.ExitCode -ne 0) {
        Write-Log "rustup-init exited with code $($p.ExitCode)" 'ERROR'
        exit 1
    }
    Refresh-SessionPath
    if (-not (Ensure-Command 'rustc')) {
        Write-Log 'rustup finished but rustc not on PATH. Open new PowerShell and check %USERPROFILE%\.cargo\bin' 'ERROR'
        exit 1
    }
    Write-Log "Rust installed: $(rustc --version)" 'OK'
}
Write-Log 'Skipping nightly/rust-src here — build-local.ps1 installs those.' 'OK'

# -------------------------------------------------------------------------
# 4) SSH key for GitHub (dedicated name — never overwrite)
# -------------------------------------------------------------------------
$sshDir = Join-Path $env:USERPROFILE '.ssh'
New-Item -ItemType Directory -Force -Path $sshDir | Out-Null

if (Test-Path $SshKeyPath) {
    Write-Log "SSH key already exists (leaving intact): $SshKeyPath" 'OK'
} else {
    Write-Log "Generating new ed25519 key: $SshKeyPath"
    # Empty passphrase for build-VM convenience
    & ssh-keygen -t ed25519 -f $SshKeyPath -N '""' -C 'win10-build-proxmox'
    if ($LASTEXITCODE -ne 0) {
        Write-Log 'ssh-keygen failed' 'ERROR'
        exit 1
    }
    Write-Log 'SSH key generated.' 'OK'
}

# SSH config: Host github.com -> this key
$sshConfig = Join-Path $sshDir 'config'
$configBlock = @"
Host github.com
  HostName github.com
  User git
  IdentityFile $SshKeyPath
  IdentitiesOnly yes
"@

$needWrite = $true
if (Test-Path $sshConfig) {
    $existing = Get-Content $sshConfig -Raw -ErrorAction SilentlyContinue
    if ($existing -and $existing -match [regex]::Escape($SshKeyName)) {
        Write-Log 'SSH config already references win10 GitHub key.' 'OK'
        $needWrite = $false
    } elseif ($existing -and $existing -match '(?m)^\s*Host\s+github\.com\s*$') {
        Write-Log 'SSH config has Host github.com but not our key — appending IdentityFile note block.' 'WARN'
        Add-Content -Path $sshConfig -Value "`n# win10-build-bootstrap`nHost github.com-win10`n  HostName github.com`n  User git`n  IdentityFile $SshKeyPath`n  IdentitiesOnly yes`n"
        $needWrite = $false
        Write-Log 'Also use: git clone git@github.com-win10:ruibeard/backup-sync-tool.git' 'WARN'
    }
}
if ($needWrite) {
    if (Test-Path $sshConfig) {
        Add-Content -Path $sshConfig -Value "`n# win10-build-bootstrap`n$configBlock"
    } else {
        Set-Content -Path $sshConfig -Value $configBlock -Encoding UTF8
    }
    Write-Log "Wrote SSH config: $sshConfig" 'OK'
}

Write-Host ''
Write-Host '========== ADD THIS PUBLIC KEY TO GITHUB ==========' -ForegroundColor Cyan
Write-Host 'GitHub -> Settings -> SSH and GPG keys -> New SSH key' -ForegroundColor Cyan
Write-Host ''
if (Test-Path $SshPubPath) {
    $pub = Get-Content $SshPubPath -Raw
    Write-Host $pub.Trim() -ForegroundColor Yellow
    Write-Log "Public key path: $SshPubPath"
    Write-Log ("Public key: " + $pub.Trim())
} else {
    Write-Log "Public key missing: $SshPubPath" 'ERROR'
}
Write-Host '====================================================' -ForegroundColor Cyan
Write-Host ''

# -------------------------------------------------------------------------
# 5) Optional clone
# -------------------------------------------------------------------------
$doClone = $true
if ($env:SKIP_CLONE -eq '1') { $doClone = $false }

if ($doClone) {
    if (Test-Path (Join-Path $CloneDir '.git')) {
        Write-Log "Repo already cloned at $CloneDir" 'OK'
    } else {
        New-Item -ItemType Directory -Force -Path (Split-Path $CloneDir) | Out-Null
        Write-Log "Cloning $RepoSshUrl -> $CloneDir"
        Write-Log '(Requires GitHub SSH key to be added first if repo is private.)'
        Refresh-SessionPath
        $cloneOk = $false
        try {
            & git clone $RepoSshUrl $CloneDir
            if ($LASTEXITCODE -eq 0) { $cloneOk = $true }
        } catch {
            Write-Log "SSH clone failed: $_" 'WARN'
        }
        if (-not $cloneOk) {
            Write-Log "SSH clone failed — trying HTTPS: $RepoHttpsUrl" 'WARN'
            & git clone $RepoHttpsUrl $CloneDir
            if ($LASTEXITCODE -ne 0) {
                Write-Log 'Clone failed. Add SSH key to GitHub, then: git clone git@github.com:ruibeard/backup-sync-tool.git' 'WARN'
            } else {
                Write-Log "Cloned via HTTPS to $CloneDir" 'OK'
            }
        } else {
            Write-Log "Cloned via SSH to $CloneDir" 'OK'
        }
    }
} else {
    Write-Log 'SKIP_CLONE=1 — not cloning.' 'OK'
}

# -------------------------------------------------------------------------
# Done
# -------------------------------------------------------------------------
Refresh-SessionPath
Write-Log '=== Bootstrap finished ===' 'OK'
Write-Host ''
Write-Host 'NEXT STEPS' -ForegroundColor Green
Write-Host '1. Copy the yellow public key above into GitHub (SSH keys).'
Write-Host "   Pubkey file: $SshPubPath"
Write-Host '2. Close this window and open a NEW PowerShell (Admin optional for build).'
Write-Host "3. cd $CloneDir"
Write-Host '4. .\build-local.ps1'
Write-Host ''
Write-Host "Log: $LogPath"
Write-Host ''
