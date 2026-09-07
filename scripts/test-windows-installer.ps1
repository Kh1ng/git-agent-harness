# Check parsing and isolated installer decisions without downloads, services, or user configuration.
$ErrorActionPreference = 'Stop'
$tokens = $null
$parseErrors = $null
$path = Join-Path $PSScriptRoot 'install-windows.ps1'
$ast = [System.Management.Automation.Language.Parser]::ParseFile($path, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw ($parseErrors | Out-String) }
$assignment = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.AssignmentStatementAst] -and $node.Left.Extent.Text -eq '$startupScript' }, $true)
if (-not $assignment) { throw 'Missing worker startup script.' }
$startup = $assignment.Right.Find({ param($node) $node -is [System.Management.Automation.Language.StringConstantExpressionAst] }, $true).Value.Replace('__SETTINGS__', '{"distribution":"Ubuntu","worker_ip":"192.168.1.11","central_ip":"192.168.1.10"}')
[void][System.Management.Automation.Language.Parser]::ParseInput($startup, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw ($parseErrors | Out-String) }

# Load only the connection writer; executing the installer would mutate this machine.
$writer = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Save-DesktopConnection' }, $true)
if (-not $writer) { throw 'Missing desktop connection writer.' }
. ([scriptblock]::Create($writer.Extent.Text))
$stage = Join-Path ([IO.Path]::GetTempPath()) ('gah-installer-check-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null
try {
    $config = Join-Path $stage 'desktop.json'
    Save-DesktopConnection $config 'http://192.168.1.10:3773' 'Ubuntu'
    $initial = Get-Content -LiteralPath $config -Raw | ConvertFrom-Json
    if ($initial.central_url -ne 'http://192.168.1.10:3773' -or $initial.wsl_distribution -ne 'Ubuntu') { throw 'Fresh connection settings were not saved.' }
    Set-Content -LiteralPath $config -Value '{"central_url":"http://old.test","wsl_distribution":"Old","presence":{"dock":true,"launch_window":false,"tray":false},"future_preference":"keep"}'
    Save-DesktopConnection $config 'https://new.test' "Ubuntu 'quoted'"
    $updated = Get-Content -LiteralPath $config -Raw | ConvertFrom-Json
    if ($updated.central_url -ne 'https://new.test' -or $updated.wsl_distribution -ne "Ubuntu 'quoted'") { throw 'Connection was not updated.' }
    if (-not $updated.presence.dock -or $updated.presence.launch_window -or $updated.presence.tray -or $updated.future_preference -ne 'keep') { throw 'Installer rerun erased desktop preferences.' }
    foreach ($invalid in @('{broken', '[]', 'null', '"text"')) {
        Set-Content -LiteralPath $config -Value $invalid
        $before = [IO.File]::ReadAllText($config)
        $rejected = $false
        try { Save-DesktopConnection $config 'https://new.test' 'Ubuntu' } catch { $rejected = $true }
        if (-not $rejected -or [IO.File]::ReadAllText($config) -ne $before) { throw 'Invalid existing settings must be preserved and rejected.' }
    }
} finally { Remove-Item -LiteralPath $stage -Recurse -Force }

# Evaluate the real asset selector with release fixtures, without calling GitHub.
$selector = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.AssignmentStatementAst] -and $node.Left.Extent.Text -eq '$asset' }, $true)
if (-not $selector) { throw 'Missing desktop release selector.' }
$releases = @(
    [pscustomobject]@{ draft = $true; prerelease = $false; assets = @([pscustomobject]@{ name = 'GAH.Worker_9.0.0_x64-setup.exe'; id = 1 }) },
    [pscustomobject]@{ draft = $false; prerelease = $true; assets = @([pscustomobject]@{ name = 'GAH.Worker_8.0.0_x64-setup.exe'; id = 2 }) },
    [pscustomobject]@{ draft = $false; prerelease = $false; assets = @(
        [pscustomobject]@{ name = 'gah.exe'; id = 3 },
        [pscustomobject]@{ name = 'GAH.Worker_0.1.0_x64-setup.exe'; id = 4 },
        [pscustomobject]@{ name = 'GAH.Worker_0.1.1_arm64-setup.exe'; id = 5 },
        [pscustomobject]@{ name = 'GAH.Worker_0.1.1_x64-setup.exe'; id = 6 }
    ) }
)
$selected = & ([scriptblock]::Create($selector.Right.Extent.Text))
if ($selected.id -ne 6) { throw 'Release selector must choose the supported stable Tauri NSIS asset.' }
Write-Host 'Windows installer checks passed: syntax, settings preservation, stable supported NSIS selection.'
