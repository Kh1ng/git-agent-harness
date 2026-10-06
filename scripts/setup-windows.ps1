#Requires -Version 5.1
<#
.SYNOPSIS
Prepare WSL for the desktop setup button and resume after Windows sign-in.
.DESCRIPTION
The pending request contains a distribution, central origin, and credential-free
setup command. Elevation enables WSL; the distribution belongs to the caller.
#>
[CmdletBinding()]
param(
    [ValidatePattern('^[a-zA-Z0-9][a-zA-Z0-9._-]*$')][string]$Distribution = 'Ubuntu',
    [string]$SetupCommand = $env:GAH_SETUP,
    [string]$CentralUrl = '',
    [switch]$Resume
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-WslSuccess([string]$Action) {
    if ($LASTEXITCODE -ne 0) { throw "$Action failed (exit $LASTEXITCODE)." }
}

# Windows PowerShell 5.1 does not escape quotes in native arguments, so a
# script travels in the environment and the one quoted argument is escaped here.
function Invoke-GahWslScript([string]$Name, [string]$Script) {
    $PSNativeCommandArgumentPassing = 'Legacy'
    $env:GAH_SCRIPT = $Script -replace "`r", ''
    if ($env:WSLENV -notmatch '(^|:)GAH_SCRIPT/u(:|$)') {
        $env:WSLENV = if ($env:WSLENV) { "$($env:WSLENV):GAH_SCRIPT/u" } else { 'GAH_SCRIPT/u' }
    }
    & wsl.exe --distribution $Name --exec bash -lc 'eval \"$GAH_SCRIPT\"'
}

function Get-GahWslDistributions {
    if (-not (Get-Command wsl.exe -ErrorAction SilentlyContinue)) { return @() }
    $listed = (& wsl.exe --list --quiet 2>$null | Out-String).Replace([string][char]0, '')
    if ($LASTEXITCODE -ne 0) { return @() }
    return @($listed -split "`r?`n" | ForEach-Object { $_.Trim() } | Where-Object { $_ })
}

function Assert-GahWslReady([string]$Name, [string]$Address) {
    $kernel = & wsl.exe --distribution $Name --exec uname -r
    Assert-WslSuccess 'WSL startup'
    if (($kernel | Out-String) -notmatch 'WSL2') { throw "The worker requires WSL2. Run: wsl --set-version $Name 2" }
    $uid = & wsl.exe --distribution $Name --exec id -u
    Assert-WslSuccess 'WSL user check'
    if (($uid | Out-String).Trim() -eq '0') { throw 'Set a non-root default WSL user before setup. Agent credentials belong to that user.' }
    if ($Address) {
        $uri = [uri]$Address
        if ($uri.Scheme -notin @('http', 'https') -or $uri.UserInfo -or $uri.Query -or $uri.Fragment -or $uri.AbsolutePath -ne '/') { throw 'Central address must be an HTTP(S) origin.' }
        $connection = Test-NetConnection -ComputerName $uri.DnsSafeHost -Port $uri.Port -InformationLevel Detailed -WarningAction SilentlyContinue
        if (-not $connection.TcpTestSucceeded) { throw 'The central node is unreachable from Windows.' }
        if (([Net.IPAddress]::Parse($connection.SourceAddress.IPAddress)).AddressFamily -ne [Net.Sockets.AddressFamily]::InterNetwork) { throw 'The WSL worker requires an IPv4 route to central.' }
    }
}

function Enable-GahWslSystemd([string]$Name) {
    & wsl.exe --distribution $Name --exec systemctl --user show-environment *> $null
    if ($LASTEXITCODE -eq 0) { return }
    Write-Host "GAH needs systemd. Enabling it requires your Linux sudo password and restarts $Name."
    if ((Read-Host 'Stop this distribution and enable systemd? [y/N]') -notmatch '^[yY]$') { throw 'Systemd preparation cancelled.' }
    $enable = @'
sudo sh -c '
set -eu
config=/etc/wsl.conf
[ -e "$config" ] || touch "$config"
tmp=$(mktemp)
trap '\''rm -f "$tmp"'\'' EXIT
awk '\''
  /^\[boot\][[:space:]]*$/ { boot=1; seen=1; print; print "systemd=true"; next }
  /^\[/ { boot=0 }
  boot && /^[[:space:]]*systemd[[:space:]]*=/ { next }
  { print }
  END { if (!seen) print "\n[boot]\nsystemd=true" }
'\'' "$config" > "$tmp"
cat "$tmp" > "$config"
'
'@
    Invoke-GahWslScript $Name $enable
    Assert-WslSuccess 'Systemd configuration'
    & wsl.exe --terminate $Name
    Assert-WslSuccess 'WSL restart'
    & wsl.exe --distribution $Name --exec systemctl --user show-environment *> $null
    Assert-WslSuccess 'WSL systemd startup'
}

# One orchestration path serves first launch, RunOnce, and retries from the app.
function Invoke-GahWindowsSetup([string]$Name, [string]$Command, [string]$Address, [bool]$Continue) {
    $directory = Join-Path $env:USERPROFILE '.config\gah'
    $pending = Join-Path $directory 'windows-setup.json'
    $runOnce = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\RunOnce'
    $initializeUser = $false
    if ($Continue) {
        $saved = Get-Content -LiteralPath $pending -Raw | ConvertFrom-Json
        $Name = $saved.distribution
        $Command = $saved.command
        $Address = $saved.central_url
        $initializeUser = $true
    } elseif (Test-Path -LiteralPath $pending) {
        # RunOnce is consumed at sign-in even when setup is interrupted. A
        # later app click must still offer user initialization for the pending
        # distribution, while retaining the app's current command and origin.
        $saved = Get-Content -LiteralPath $pending -Raw | ConvertFrom-Json
        $initializeUser = $saved.distribution -ceq $Name
    }
    if ($Name -cnotmatch '^[a-zA-Z0-9][a-zA-Z0-9._-]*$' -or -not $Command) { throw 'Invalid or missing desktop setup request.' }
    if ($Name -notin @(Get-GahWslDistributions)) {
        Write-Host "GAH needs WSL2 and $Name. Windows may request a restart."
        if (-not $Continue -and (Read-Host 'Enable WSL and continue setup? [y/N]') -notmatch '^[yY]$') { return }
        New-Item -ItemType Directory -Path $directory -Force | Out-Null
        [IO.File]::WriteAllText($pending, (@{ distribution = $Name; command = $Command; central_url = $Address } | ConvertTo-Json), (New-Object Text.UTF8Encoding($false)))
        New-Item -Path $runOnce -Force | Out-Null
        New-ItemProperty -Path $runOnce -Name '!GAHSetup' -Value "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`" -Resume" -PropertyType String -Force | Out-Null
        & wsl.exe --status *> $null
        if ($LASTEXITCODE -ne 0) {
            # Elevate only machine-wide features. Installing a distribution as a
            # different UAC administrator would register it to the wrong user.
            $enabled = Start-Process -FilePath 'wsl.exe' -ArgumentList @('--install', '--no-distribution', '--no-launch') -Verb RunAs -Wait -PassThru
            if ($enabled.ExitCode -notin @(0, 3010)) { throw "WSL preparation failed (exit $($enabled.ExitCode)). Select setup again to retry." }
            Write-Host 'Restart Windows, then sign in. GAH setup will reopen automatically.'
            return
        }
        & wsl.exe --set-default-version 2
        Assert-WslSuccess 'WSL2 default'
        & wsl.exe --install --distribution $Name --no-launch
        if ($LASTEXITCODE -eq 3010) {
            Write-Host 'Restart Windows, then sign in. GAH setup will reopen automatically.'
            return
        }
        Assert-WslSuccess 'Distribution installation'
        if ($Name -notin @(Get-GahWslDistributions)) { throw 'The distribution is not registered. Select setup again to retry.' }
        $initializeUser = $true
    }
    if ($initializeUser) {
        Write-Host "Open $Name, create your Linux user when prompted, then type exit to continue GAH setup."
        & wsl.exe --distribution $Name
        Assert-WslSuccess 'Linux user initialization'
    }
    Assert-GahWslReady $Name $Address
    Enable-GahWslSystemd $Name
    Invoke-GahWslScript $Name $Command
    Assert-WslSuccess 'GAH setup'
    Remove-Item -LiteralPath $pending -Force -ErrorAction SilentlyContinue
    Remove-ItemProperty -Path $runOnce -Name '!GAHSetup' -ErrorAction SilentlyContinue
    Write-Host 'GAH setup finished. Return to the app and check setup again.'
}

try {
    Invoke-GahWindowsSetup $Distribution $SetupCommand $CentralUrl $Resume.IsPresent
} catch {
    Write-Error $_
} finally {
    [void](Read-Host 'Press Enter to close')
}
