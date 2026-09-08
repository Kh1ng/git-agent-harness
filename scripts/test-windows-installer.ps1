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
    foreach ($invalid in @('{broken', '[]', '[{}]', 'null', '"text"')) {
        Set-Content -LiteralPath $config -Value $invalid
        $before = [IO.File]::ReadAllText($config)
        $rejected = $false
        try { Save-DesktopConnection $config 'https://new.test' 'Ubuntu' } catch { $rejected = $true }
        if (-not $rejected -or [IO.File]::ReadAllText($config) -ne $before) { throw "Invalid existing settings must be preserved and rejected: $invalid" }
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

# Exercise artifact validation and download selection only. Never run the installer body.
foreach ($name in @('Get-TestArtifactFiles', 'Download-SetupFile')) {
    $definition = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name }, $true)
    if (-not $definition) { throw "Missing installer helper: $name" }
    . ([scriptblock]::Create($definition.Extent.Text))
}
function Write-TestManifest([string]$Directory, [string]$Kind, [string]$Revision, [string[]]$Names) {
    $files = @{}
    foreach ($name in $Names) { $files[$name] = (Get-FileHash -LiteralPath (Join-Path $Directory $name) -Algorithm SHA256).Hash.ToLowerInvariant() }
    @{ schema_version = 1; revision = $Revision; files = $files } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $Directory "$Kind-artifact.json")
}
function Assert-TestArtifactRejected([scriptblock]$Action, [string]$Reason) {
    $rejected = $false
    try { & $Action | Out-Null } catch { $rejected = $true }
    if (-not $rejected) { throw "Invalid test artifacts were accepted: $Reason" }
}
$bundle = Join-Path ([IO.Path]::GetTempPath()) ('gah-artifact-check-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $bundle | Out-Null
try {
    $desktopFiles = @('install-windows.ps1', 'GAH Worker_0.1.1_x64-setup.exe')
    $workerFiles = @('install-windows.ps1', 'gah', 'source.tar.gz', 'install-wsl.sh')
    foreach ($name in ($desktopFiles + $workerFiles | Select-Object -Unique)) { Set-Content -LiteralPath (Join-Path $bundle $name) -Value "test bytes for $name" }
    $installerPath = Join-Path $bundle 'install-windows.ps1'
    $revision = 'a' * 40
    Write-TestManifest $bundle desktop $revision $desktopFiles
    Write-TestManifest $bundle worker $revision $workerFiles
    $testArtifactFiles = Get-TestArtifactFiles $bundle both $installerPath
    if ($testArtifactFiles.Count -ne 5) { throw 'Both artifacts must resolve exactly five installer routes.' }
    $TestArtifactDirectory = $bundle
    $copied = Join-Path $bundle 'copied-gah'
    Download-SetupFile 'release/linux-cli' $copied
    if ([IO.File]::ReadAllText($copied) -ne [IO.File]::ReadAllText((Join-Path $bundle 'gah'))) { throw 'Test mode did not copy the verified artifact.' }
    Assert-TestArtifactRejected { Download-SetupFile 'unknown' $copied } 'unknown route'

    Write-TestManifest $bundle worker ('b' * 40) $workerFiles
    Assert-TestArtifactRejected { Get-TestArtifactFiles $bundle both $installerPath } 'mixed revisions'
    # Desktop-only validation does not require a worker manifest.
    Remove-Item -LiteralPath (Join-Path $bundle 'worker-artifact.json')
    [void](Get-TestArtifactFiles $bundle desktop $installerPath)
    Assert-TestArtifactRejected { Get-TestArtifactFiles $bundle worker $installerPath } 'missing manifest'
    Write-TestManifest $bundle worker $revision $workerFiles
    $manifestPath = Join-Path $bundle 'worker-artifact.json'
    $validManifest = [IO.File]::ReadAllText($manifestPath)
    foreach ($invalid in @('[]', '[{}]', '{broken', '{}', '{"schema_version":1,"revision":"invalid","files":{}}')) {
        Set-Content -LiteralPath $manifestPath -Value $invalid
        Assert-TestArtifactRejected { Get-TestArtifactFiles $bundle worker $installerPath } 'malformed manifest'
    }
    Set-Content -LiteralPath $manifestPath -Value $validManifest
    $malicious = $validManifest.Replace('"gah"', '"../gah"')
    Set-Content -LiteralPath $manifestPath -Value $malicious
    Assert-TestArtifactRejected { Get-TestArtifactFiles $bundle worker $installerPath } 'path traversal'
    Set-Content -LiteralPath $manifestPath -Value $validManifest
    Set-Content -LiteralPath (Join-Path $bundle 'gah') -Value 'changed bytes'
    Assert-TestArtifactRejected { Get-TestArtifactFiles $bundle worker $installerPath } 'checksum mismatch'
    Write-TestManifest $bundle worker $revision $workerFiles
    Assert-TestArtifactRejected { Get-TestArtifactFiles $bundle worker $path } 'executing installer from another bundle'

    # Online mode keeps its original authenticated URL and never reads local artifacts.
    $TestArtifactDirectory = ''
    $CentralUrl = 'https://central.test'
    $CoordinatorToken = 'test-coordinator-token'
    function Invoke-WebRequest { param([switch]$UseBasicParsing, $Uri, $Headers, $OutFile)
        if ($Uri -ne 'https://central.test/api/settings/nodes/release/linux-cli' -or $Headers.Authorization -ne 'Bearer test-coordinator-token' -or $OutFile -ne $copied) { throw 'Online setup request changed.' }
        $script:downloadObserved = $true
    }
    $script:downloadObserved = $false
    Download-SetupFile 'release/linux-cli' $copied
    if (-not $script:downloadObserved) { throw 'Online setup did not use its normal download helper.' }
    Remove-Item Function:\Invoke-WebRequest
} finally { Remove-Item -LiteralPath $bundle -Recurse -Force }
Write-Host 'Artifact checks passed: matching revisions, checksums, malformed bundles, local copies, unchanged online downloads.'
