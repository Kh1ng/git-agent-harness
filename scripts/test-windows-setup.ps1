# Exercise the real resume flow with isolated files and mocked Windows/WSL commands.
$ErrorActionPreference = 'Stop'
$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'setup-windows.ps1'), [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw ($errors | Out-String) }
foreach ($fn in $ast.FindAll({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $false)) {
    . ([scriptblock]::Create($fn.Extent.Text))
}
$stage = Join-Path ([IO.Path]::GetTempPath()) ('gah-wsl-check-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null
$oldProfile = $env:USERPROFILE
$oldSetup = $env:GAH_SETUP
$oldWslEnv = $env:WSLENV
$env:USERPROFILE = $stage
$script:calls = @()
$script:installed = $false
$script:enabled = $false
$script:uid = '1000'
$script:kernel = '6.6.0-microsoft-standard-WSL2'
$script:setupExit = 0
$script:answer = 'y'
$script:reachable = $true
$script:source = [pscustomobject]@{ IPAddress = '192.168.1.11'; AddressFamily = 2 }
function wsl.exe {
    $script:calls += ,@($args)
    $global:LASTEXITCODE = 0
    if ($args -contains '--status') { if (-not $script:enabled) { $global:LASTEXITCODE = 1 }; return }
    if ($args -contains '--list') { if ($script:installed) { "Ubuntu`r`n" }; return }
    if ($args -contains '--install') { $script:installed = $true; return }
    if ($args -contains 'uname') { $script:kernel; return }
    if ($args -contains 'id') { $script:uid; return }
    if ($args -contains 'bash') { $global:LASTEXITCODE = $script:setupExit }
}
function Read-Host { param($Prompt) $script:answer }
function Start-Process { param($FilePath, $ArgumentList, $Verb, [switch]$Wait, [switch]$PassThru)
    if ($Verb -ne 'RunAs' -or $ArgumentList -contains '--distribution' -or $ArgumentList -notcontains '--no-distribution') { throw 'Elevation must never install a user distribution.' }
    $script:enabled = $true
    [pscustomobject]@{ ExitCode = 3010 }
}
function New-Item { param($Path, $ItemType, [switch]$Force)
    if ($Path -like 'HKCU:*') { return }
    Microsoft.PowerShell.Management\New-Item -Path $Path -ItemType $ItemType -Force:$Force
}
function New-ItemProperty { param($Path, $Name, $Value, $PropertyType, [switch]$Force)
    if ($Name -ne '!GAHSetup' -or $Value -notmatch '-Resume$') { throw 'Missing restart resume registration.' }
    $script:resumeRegistered = $true
}
function Remove-ItemProperty { param($Path, $Name, $ErrorAction) $script:resumeRegistered = $false }
function Test-NetConnection { param($ComputerName, $Port, $InformationLevel, $WarningAction)
    [pscustomobject]@{ TcpTestSucceeded = $script:reachable; SourceAddress = $script:source }
}
function Assert-Rejected([scriptblock]$Action, [string]$Expected) {
    try { & $Action } catch { if ($_.Exception.Message -match $Expected) { return }; throw }
    throw "Expected rejection: $Expected"
}
try {
    $command = 'printf "%s" "a quote and a pipe |"; ~/.cargo/bin/gah setup'
    $pending = Join-Path $stage '.config/gah/windows-setup.json'
    Invoke-GahWindowsSetup 'Ubuntu' $command 'https://central.test' $false
    if (-not $script:resumeRegistered -or -not (Test-Path $pending)) { throw 'Restart lost pending setup.' }
    if ($script:installed -or ($script:calls | Where-Object { $_ -contains 'bash' })) { throw 'Setup ran before restart.' }
    $saved = Get-Content $pending -Raw | ConvertFrom-Json
    if ($saved.command -cne $command) { throw 'Saved command quoting changed.' }

    Invoke-GahWindowsSetup 'ignored' 'ignored' '' $true
    if (-not $script:installed -or $env:GAH_SETUP -cne $command -or $env:WSLENV -notmatch 'GAH_SETUP/u') { throw 'Resume did not install for the caller and forward the literal command.' }
    if (Test-Path $pending) { throw 'Successful resume retained pending setup.' }
    if ($script:resumeRegistered) { throw 'Successful setup retained RunOnce.' }
    $userLaunch = $script:calls | Where-Object { $_.Count -eq 2 -and $_[0] -eq '--distribution' }
    if (-not $userLaunch) { throw 'Resume skipped Linux user initialization.' }

    # A failed native setup keeps the pending request, so the next app click can retry.
    [IO.File]::WriteAllText($pending, ($saved | ConvertTo-Json))
    $script:setupExit = 7
    Assert-Rejected { Invoke-GahWindowsSetup 'Ubuntu' '' '' $true } 'GAH setup failed'
    if (-not (Test-Path $pending)) { throw 'Failure erased the pending setup.' }
    $script:setupExit = 0
    $script:uid = '0'
    Assert-Rejected { Assert-GahWslReady 'Ubuntu' '' } 'non-root'
    $script:uid = '1000'
    $script:kernel = '4.4.0-Microsoft'
    Assert-Rejected { Assert-GahWslReady 'Ubuntu' '' } 'WSL2'
    $script:kernel = '6.6.0-microsoft-standard-WSL2'
    $script:reachable = $false
    Assert-Rejected { Assert-GahWslReady 'Ubuntu' 'https://central.test' } 'unreachable'
    $script:reachable = $true
    $script:source = [pscustomobject]@{ IPAddress = '::1'; AddressFamily = 23 }
    Assert-Rejected { Assert-GahWslReady 'Ubuntu' 'https://central.test' } 'IPv4'
    Assert-Rejected { Assert-GahWslReady 'Ubuntu' 'https://central.test/path' } 'origin'
    Assert-Rejected { Invoke-GahWindowsSetup '-bad' $command '' $false } 'Invalid'
    Remove-Item -LiteralPath $pending
    $script:installed = $false
    $script:answer = 'n'
    Invoke-GahWindowsSetup 'Ubuntu' $command '' $false
    if (Test-Path $pending) { throw 'Declining WSL wrote pending setup.' }
    Write-Host 'WSL setup checks passed: feature elevation, restart/resume, Linux user initialization, literal command, retry, WSL1/root/IPv6/outage rejection, cancellation.'
} finally {
    $env:USERPROFILE = $oldProfile
    $env:GAH_SETUP = $oldSetup
    $env:WSLENV = $oldWslEnv
    Remove-Item -LiteralPath $stage -Recurse -Force
}
