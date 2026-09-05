#Requires -Version 5.1
<#
.SYNOPSIS
    Install or upgrade the GAH Worker desktop app on Windows.

.DESCRIPTION
    Downloads the latest NSIS installer from the private GitHub repository
    and runs it silently. A GitHub token with repo read access is required
    because the repository is private.

.PARAMETER Token
    GitHub personal access token (classic) with repo scope. If omitted the
    script reads $env:GITHUB_TOKEN, then prompts interactively.

.PARAMETER Version
    Release tag to install. Defaults to the latest release that contains a
    Windows .exe asset.

.EXAMPLE
    # One-liner (token in env):
    $env:GITHUB_TOKEN="ghp_xxxxxxxxxxxx"; irm "https://raw.githubusercontent.com/Kh1ng/git-agent-harness/main/scripts/install-windows.ps1" | iex

.EXAMPLE
    # Supply the token explicitly:
    & .\install-windows.ps1 -Token ghp_xxxxxxxxxxxx
#>
[CmdletBinding()]
param(
    [string]$Token   = $env:GITHUB_TOKEN,
    [string]$Version = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$Repo = 'Kh1ng/git-agent-harness'

if (-not $Token) {
    $Token = Read-Host -Prompt 'GitHub token (repo read access required)'
}

$headers = @{
    Authorization          = "Bearer $Token"
    Accept                 = 'application/vnd.github+json'
    'X-GitHub-Api-Version' = '2022-11-28'
}

Write-Host "Fetching release info from github.com/$Repo ..."

if ($Version) {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/tags/$Version" -Headers $headers
} else {
    $releases = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases" -Headers $headers
    $release  = $releases |
        Where-Object { $_.assets | Where-Object { $_.name -like '*.exe' } } |
        Select-Object -First 1
}

if (-not $release) {
    Write-Error "No Windows release found. Run the 'Release — Windows' workflow first."
    exit 1
}

$asset = $release.assets | Where-Object { $_.name -like '*.exe' } | Select-Object -First 1
if (-not $asset) {
    Write-Error "Release '$($release.tag_name)' has no .exe asset."
    exit 1
}

$installer = Join-Path $env:TEMP $asset.name
Write-Host "Downloading $($asset.name) from release $($release.tag_name) ..."

Invoke-WebRequest `
    -Uri "https://api.github.com/repos/$Repo/releases/assets/$($asset.id)" `
    -Headers @{
        Authorization          = "Bearer $Token"
        Accept                 = 'application/octet-stream'
        'X-GitHub-Api-Version' = '2022-11-28'
    } `
    -OutFile $installer

Write-Host "Running installer (silent) ..."
$proc = Start-Process -FilePath $installer -ArgumentList '/S' -PassThru -Wait
if ($proc.ExitCode -ne 0) {
    Write-Error "Installer exited with code $($proc.ExitCode)."
    exit $proc.ExitCode
}

Write-Host "GAH Worker installed. Launch it from the Start menu."
