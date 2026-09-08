#Requires -Version 5.1
<#
.SYNOPSIS
Install the native GAH desktop, a WSL2 headless worker, or both.
.DESCRIPTION
Settings > Add a Node supplies CentralUrl and CoordinatorToken. Worker setup
requires an elevated PowerShell session and an initialized Ubuntu WSL2 user.
Without CentralUrl this retains the standalone GitHub desktop installer.
TestArtifactDirectory accepts verified unpublished bundles from one Desktop workflow run.
#>
[CmdletBinding()]
param(
    [string]$Token = $env:GITHUB_TOKEN,
    [string]$Version = '',
    [string]$CentralUrl = '',
    [string]$CoordinatorToken = '',
    [ValidateSet('desktop', 'worker', 'both')][string]$Role = 'desktop',
    [string]$WslDistribution = 'Ubuntu',
    [string]$TestArtifactDirectory = ''
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

function Assert-NativeSuccess([string]$Action) {
    if ($LASTEXITCODE -ne 0) { throw "$Action failed (exit $LASTEXITCODE)." }
}
function Download-SetupFile([string]$Name, [string]$Destination) {
    if ($TestArtifactDirectory) {
        if (-not $testArtifactFiles.ContainsKey($Name)) { throw "No verified test artifact for $Name." }
        Copy-Item -LiteralPath $testArtifactFiles[$Name] -Destination $Destination
    } else {
        Invoke-WebRequest -UseBasicParsing -Uri "$CentralUrl/api/settings/nodes/$Name" -Headers @{ Authorization = "Bearer $CoordinatorToken" } -OutFile $Destination
    }
}

# Validate the entire selected bundle before installing an executable or writing settings.
function Get-TestArtifactFiles([string]$Directory, [string]$InstallRole, [string]$InstallerPath) {
    $result = @{}
    $revision = $null
    $installerHash = (Get-FileHash -LiteralPath $InstallerPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $kinds = if ($InstallRole -eq 'both') { @('desktop', 'worker') } elseif ($InstallRole -eq 'worker') { @('worker') } else { @('desktop') }
    foreach ($kind in $kinds) {
        $json = [IO.File]::ReadAllText((Join-Path $Directory "$kind-artifact.json"))
        if (-not $json.TrimStart().StartsWith('{')) { throw 'Artifact manifest must be a JSON object.' }
        $manifest = ConvertFrom-Json -InputObject $json
        if ($manifest.schema_version -ne 1 -or $manifest.revision -cnotmatch '^[a-f0-9]{40}$') { throw 'Invalid artifact manifest version or revision.' }
        if ($revision -and $revision -cne $manifest.revision) { throw 'Desktop and worker artifact revisions do not match.' }
        $revision = $manifest.revision
        $names = @($manifest.files.PSObject.Properties.Name)
        $expectedCount = if ($kind -eq 'desktop') { 2 } else { 4 }
        if ($names.Count -ne $expectedCount -or 'install-windows.ps1' -cnotin $names) { throw 'Artifact manifest has an unexpected file list.' }
        foreach ($entry in $manifest.files.PSObject.Properties) {
            $name = $entry.Name
            $route = if ($name -ceq 'install-windows.ps1') { 'install.ps1' }
                elseif ($kind -eq 'desktop' -and $name -cmatch '^[a-zA-Z0-9 ._-]+_\d+\.\d+\.\d+_x64-setup\.exe$') { 'release/desktop' }
                elseif ($kind -eq 'worker' -and $name -ceq 'gah') { 'release/linux-cli' }
                elseif ($kind -eq 'worker' -and $name -cin @('source.tar.gz', 'install-wsl.sh')) { $name }
                else { throw "Unexpected artifact filename: $name" }
            if ($entry.Value -isnot [string] -or $entry.Value -cnotmatch '^[a-f0-9]{64}$') { throw "Invalid checksum for $name." }
            $file = Join-Path $Directory $name
            $hash = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($hash -cne $entry.Value) { throw "Artifact checksum mismatch: $name" }
            if ($name -ceq 'install-windows.ps1' -and $hash -cne $installerHash) { throw 'Run the install-windows.ps1 from the selected test artifacts.' }
            $result[$route] = $file
        }
    }
    Write-Host "Verified unpublished test artifacts from $revision"
    return $result
}

# Installer reruns change the connection, not the operator's desktop preferences.
function Save-DesktopConnection([string]$Path, [string]$Address, [string]$Distribution) {
    $settings = if (Test-Path -LiteralPath $Path) {
        $json = [IO.File]::ReadAllText($Path)
        # Windows PowerShell unwraps JSON arrays on the pipeline. Reject non-object
        # roots before parsing so a one-object array cannot masquerade as settings.
        if (-not $json.TrimStart().StartsWith('{')) { throw "Desktop settings at $Path must be a JSON object." }
        ConvertFrom-Json -InputObject $json
    } else { [pscustomobject]@{} }
    if ($settings -isnot [pscustomobject]) { throw "Desktop settings at $Path must be a JSON object. Repair the file before rerunning setup." }
    $settings | Add-Member -NotePropertyName central_url -NotePropertyValue $Address -Force
    $settings | Add-Member -NotePropertyName wsl_distribution -NotePropertyValue $Distribution -Force
    [IO.File]::WriteAllText($Path, ($settings | ConvertTo-Json -Depth 20), (New-Object Text.UTF8Encoding($false)))
}

$testArtifactFiles = if ($TestArtifactDirectory) { Get-TestArtifactFiles $TestArtifactDirectory $Role $PSCommandPath } else { @{} }

if ($CentralUrl) {
    $uri = [uri]$CentralUrl
    if ($uri.Scheme -notin @('http', 'https') -or $uri.UserInfo -or $uri.Query -or $uri.Fragment -or $uri.AbsolutePath -ne '/') { throw 'CentralUrl must be an HTTP(S) origin without credentials or a path.' }
    if (-not $CoordinatorToken) { throw 'CoordinatorToken is required for central-node installation.' }
    $CentralUrl = $CentralUrl.TrimEnd('/')
} elseif ($Role -ne 'desktop') { throw 'Worker installation requires the command from Settings > Add a Node.' }
if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64') { throw 'This installer currently requires x64 Windows.' }
if ($Role -ne 'desktop') {
    $principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Run this setup command in PowerShell as administrator. Worker setup configures a LAN port and a logon task.' }
    if ($WslDistribution.StartsWith('-') -or $WslDistribution -match '[\r\n]') { throw 'Invalid WSL distribution name.' }
    if (-not (Get-Command wsl.exe -ErrorAction SilentlyContinue)) { throw 'WSL is unavailable. Install WSL2, restart Windows, initialize Ubuntu, then rerun this command.' }
    $distros = (& wsl.exe --list --quiet | Out-String).Replace([string][char]0, '').Trim() -split "`r?`n"
    if ($WslDistribution -notin ($distros | ForEach-Object { $_.Trim() })) {
        & wsl.exe --install --distribution $WslDistribution --no-launch
        Assert-NativeSuccess 'WSL installation'
        throw "WSL was installed but needs initialization. Restart if prompted, open $WslDistribution and create your Linux user, then rerun this setup command."
    }
    $kernel = & wsl.exe --distribution $WslDistribution --exec uname -r
    Assert-NativeSuccess 'WSL startup'
    if (($kernel | Out-String) -notmatch 'WSL2') { throw "The worker requires WSL2. Run: wsl --set-version $WslDistribution 2" }
    $uid = & wsl.exe --distribution $WslDistribution --exec id -u
    Assert-NativeSuccess 'WSL user check'
    if (($uid | Out-String).Trim() -eq '0') { throw 'Set a non-root default WSL user, then rerun setup. Agent credentials belong to that user.' }
}

$stage = Join-Path $env:TEMP ('gah-setup-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null
try {
    if ($Role -ne 'worker') {
        $installer = Join-Path $stage 'gah_x64-setup.exe'
        if ($CentralUrl -or $TestArtifactDirectory) { Download-SetupFile 'release/desktop' $installer }
        else {
            if (-not $Token -and (Get-Command gh -ErrorAction SilentlyContinue)) { $Token = (& gh auth token | Out-String).Trim() }
            if (-not $Token) { throw 'Set GITHUB_TOKEN or run gh auth login to read the private release.' }
            $headers = @{ Authorization = "Bearer $Token"; Accept = 'application/vnd.github+json'; 'X-GitHub-Api-Version' = '2022-11-28' }
            $api = 'https://api.github.com/repos/Kh1ng/git-agent-harness/releases'
            $releases = if ($Version) { @(Invoke-RestMethod -Uri "$api/tags/$Version" -Headers $headers) } else { @(Invoke-RestMethod -Uri $api -Headers $headers) }
            $asset = $releases | Where-Object { -not $_.draft -and -not $_.prerelease } | ForEach-Object { $_.assets } | Where-Object { $_.name -match '_(\d+\.\d+\.\d+)_x64-setup\.exe$' -and [version]$Matches[1] -ge [version]'0.1.1' } | Select-Object -First 1
            if (-not $asset) { throw 'No Tauri desktop 0.1.1+ x64 NSIS setup asset exists in the selected releases.' }
            $headers.Accept = 'application/octet-stream'
            Invoke-WebRequest -UseBasicParsing -Uri "$api/assets/$($asset.id)" -Headers $headers -OutFile $installer
        }
        $proc = Start-Process -FilePath $installer -ArgumentList '/S' -PassThru -Wait
        if ($proc.ExitCode -ne 0) { throw "Desktop installer failed (exit $($proc.ExitCode))." }
    }
    $configDir = Join-Path $env:USERPROFILE '.config\gah'
    New-Item -ItemType Directory -Path $configDir -Force | Out-Null
    $utf8 = New-Object Text.UTF8Encoding($false)
    if ($CentralUrl) {
        Save-DesktopConnection (Join-Path $configDir 'desktop.json') $CentralUrl $WslDistribution
    }
    if ($Role -ne 'desktop') {
        $connection = Test-NetConnection -ComputerName ([uri]$CentralUrl).DnsSafeHost -Port ([uri]$CentralUrl).Port -InformationLevel Detailed -WarningAction SilentlyContinue
        if (-not $connection.TcpTestSucceeded) { throw 'The central node is unreachable from Windows.' }
        $workerIp = $connection.SourceAddress.IPAddress
        $centralIp = $connection.RemoteAddress.IPAddressToString
        if ([Net.IPAddress]::Parse($workerIp).AddressFamily -ne [Net.Sockets.AddressFamily]::InterNetwork) { throw 'WSL LAN forwarding currently requires an IPv4 route to the central node.' }
        Download-SetupFile 'source.tar.gz' (Join-Path $stage 'source.tar.gz')
        Download-SetupFile 'release/linux-cli' (Join-Path $stage 'gah')
        Download-SetupFile 'install-wsl.sh' (Join-Path $stage 'install-wsl.sh')
        # Limit access before writing the coordinator credential to the staging directory.
        & icacls.exe $stage /inheritance:r /grant:r "$($env:USERNAME):(OI)(CI)F" 'SYSTEM:(OI)(CI)F' | Out-Null
        Assert-NativeSuccess 'Setup directory permissions'
        [IO.File]::WriteAllText((Join-Path $stage 'settings.json'), (@{ central_url = $CentralUrl; token = $CoordinatorToken; advertised_url = "http://${workerIp}:3774"; display_name = $env:COMPUTERNAME } | ConvertTo-Json), $utf8)
        $wslStage = (& wsl.exe --distribution $WslDistribution --exec wslpath -a $stage | Out-String).Trim()
        Assert-NativeSuccess 'WSL path conversion'
        & wsl.exe --distribution $WslDistribution --exec bash "$wslStage/install-wsl.sh" $wslStage
        Assert-NativeSuccess 'Headless worker installation'

        # A user service owns the worker. The logon task keeps WSL alive and refreshes its NAT address.
        # The app never owns or terminates this service.
        $startupDir = Join-Path $env:ProgramFiles 'GAH Worker Startup'
        New-Item -ItemType Directory -Path $startupDir -Force | Out-Null
        $startup = Join-Path $startupDir 'start-wsl-worker.ps1'
        $startupSettings = @{ distribution = $WslDistribution; worker_ip = $workerIp; central_ip = $centralIp } | ConvertTo-Json -Compress
        $startupScript = @'
$ErrorActionPreference = 'Stop'
$settings = ConvertFrom-Json '__SETTINGS__'
$ips = (& wsl.exe --distribution $settings.distribution --exec hostname -I | Out-String).Trim() -split '\s+'
if ($LASTEXITCODE -ne 0) { throw 'Cannot start WSL.' }
$ip = $ips | Where-Object { $_ -match '^\d+\.\d+\.\d+\.\d+$' } | Select-Object -First 1
if (-not $ip) { throw 'Cannot find the WSL IPv4 address.' }
& netsh.exe interface portproxy add v4tov4 listenaddress=$($settings.worker_ip) listenport=3774 connectaddress=$ip connectport=3774
if ($LASTEXITCODE -ne 0) { throw 'Cannot configure the WSL worker port.' }
& wsl.exe --distribution $settings.distribution --exec systemctl --user start gah-worker.service
if ($LASTEXITCODE -ne 0) { throw 'Cannot start gah-worker.service.' }
& wsl.exe --distribution $settings.distribution --exec sleep infinity
'@
        [IO.File]::WriteAllText($startup, $startupScript.Replace('__SETTINGS__', $startupSettings.Replace("'", "''")), $utf8)
        if (-not (Get-NetFirewallRule -Name 'GAH-WSL-Worker' -ErrorAction SilentlyContinue)) {
            New-NetFirewallRule -Name 'GAH-WSL-Worker' -DisplayName 'GAH WSL Worker (central node only)' -Direction Inbound -Action Allow -Protocol TCP -LocalPort 3774 -LocalAddress $workerIp -RemoteAddress $centralIp | Out-Null
        } else {
            Get-NetFirewallRule -Name 'GAH-WSL-Worker' | Get-NetFirewallAddressFilter | Set-NetFirewallAddressFilter -LocalAddress $workerIp -RemoteAddress $centralIp | Out-Null
        }
        $action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$startup`""
        $trigger = New-ScheduledTaskTrigger -AtLogOn -User ([Security.Principal.WindowsIdentity]::GetCurrent().Name)
        $taskSettings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
        $taskPrincipal = New-ScheduledTaskPrincipal -UserId ([Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Highest
        Register-ScheduledTask -TaskName 'GAH WSL Worker' -Action $action -Trigger $trigger -Settings $taskSettings -Principal $taskPrincipal -Force | Out-Null
        Stop-ScheduledTask -TaskName 'GAH WSL Worker' -ErrorAction SilentlyContinue
        Start-ScheduledTask -TaskName 'GAH WSL Worker'
        $identityJson = (& wsl.exe --distribution $WslDistribution --exec curl -fsS http://127.0.0.1:3774/health | Out-String)
        Assert-NativeSuccess 'Worker health check'
        $identity = $identityJson | ConvertFrom-Json
        $forwardingReady = $false
        for ($attempt = 0; $attempt -lt 20; $attempt++) {
            try {
                $forwarded = Invoke-RestMethod -Uri "http://${workerIp}:3774/health" -TimeoutSec 2
                if ($forwarded.node_id -eq $identity.node_id) { $forwardingReady = $true; break }
            } catch { }
            Start-Sleep -Milliseconds 500
        }
        if (-not $forwardingReady) { throw 'The WSL service is installed, but Windows forwarding is not ready. Check the GAH WSL Worker task and port 3774, then rerun setup.' }
        & wsl.exe --distribution $WslDistribution --exec bash -lc '"$HOME/.local/share/gah/worker/register.sh"'
        Assert-NativeSuccess 'Node registration'
        Write-Host 'Worker registered. It starts at Windows logon and continues after the desktop app quits.'
        Write-Host 'Needs setup: authenticate repository/agent CLIs inside WSL, add a repository profile, then register its profile name. Installation does not imply dispatch readiness.'
        Write-Host "Open $WslDistribution and run: ~/.local/share/gah/worker/register.sh PROFILE_NAME"
        Write-Host 'Use a stable LAN/VPN address. Rerun setup if the Windows address changes.'
    }
    if ($Role -ne 'worker') { Write-Host 'GAH desktop installed. Open GAH Worker from the Start menu.' }
} finally {
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
}
