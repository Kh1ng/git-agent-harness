#Requires -Version 5.1
<#
.SYNOPSIS
    Install or upgrade the GAH Worker desktop app on Windows.

.DESCRIPTION
    Downloads the latest NSIS installer from the private GitHub repository
    and runs it silently. A GitHub token with repo read access is required
    because the repository is private.

.PARAMETER Token
    GitHub personal access token (classic) with at least read:packages or
    repo scope. If omitted the script looks for the GITHUB_TOKEN environment
    variable, then prompts interactively.

.PARAMETER Version
    Release tag to install. Defaults to the latest release that contains a
    Windows .exe asset.

.EXAMPLE
    # One-liner from a shell where $env:GITHUB_TOKEN is set:
    irm "https://raw.githubusercontent.com/Kh1ng/git-agent-harness/main/scripts/install-windows.ps1" | iex

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
    Authorization = "Bearer $Token"
    Accept        = 'application/vnd.github+json'
    'X-GitHub-Api-Version' = '2022-11-28'
}

Write-Host "Fetching release info from github.com/$Repo ..."

$releasesUrl = "https://api.github.com/repos/$Repo/releases"
if ($Version) {
    $releasesUrl = "https://api.github.com/repos/$Repo/releases/tags/$Version"
    $release = Invoke-RestMethod -Uri $releasesUrl -Headers $headers
} else {
    $releases = Invoke-RestMethod -Uri $releasesUrl -Headers $headers
    $release  = $releases | Where-Object { $_.assets | Where-Object { $_.name -like '*.exe' } } |
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

# GitHub release asset downloads require a redirect to the S3 URL; use the
# asset download API endpoint, which returns the binary directly when the
# Accept header requests the raw octet stream.
Invoke-WebRequest `
    -Uri "https://api.github.com/repos/$Repo/releases/assets/$($asset.id)" `
    -Headers @{
        Authorization = "Bearer $Token"
        Accept        = 'application/octet-stream'
        'X-GitHub-Api-Version' = '2022-11-28'
    } `
    -OutFile $installer

Write-Host "Running installer (silent) ..."
$proc = Start-Process -FilePath $installer -ArgumentList '/S' -PassThru -Wait
if ($proc.ExitCode -ne 0) {
    Write-Error "Installer exited with code $($proc.ExitCode)."
    exit $proc.ExitCode
}

Write-Host "GAH Worker installed. Launch it from the Start menu or run 'gah-desktop'."
