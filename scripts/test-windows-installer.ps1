# Parse both the installer and its embedded scheduled-task script without running either.
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
Write-Host 'Windows installer and logon task PowerShell syntax checks passed.'
