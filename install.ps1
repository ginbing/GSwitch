# GSwitch Windows installer helper.
# This script downloads the NSIS installer from the latest public GitHub Release.

[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$Repository = 'ginbing/GSwitch'
$ReleasesPage = "https://github.com/$Repository/releases"
$ReleaseApi = "https://api.github.com/repos/$Repository/releases/latest"
$GitHubHeaders = @{
  Accept = 'application/vnd.github+json'
  'User-Agent' = 'GSwitch-installer'
  'X-GitHub-Api-Version' = '2022-11-28'
}

function Select-GSwitchInstallerAsset {
  [CmdletBinding()]
  param(
    [object[]]$Assets
  )

  $matches = @(
    $Assets | Where-Object {
      $_.name -is [string] -and $_.name -match '(?i)-setup\.exe$'
    }
  )

  if ($matches.Count -ne 1) {
    $available = @($Assets | ForEach-Object { [string]$_.name }) -join ', '
    if ([string]::IsNullOrWhiteSpace($available)) {
      $available = 'none'
    }

    throw "Expected exactly one Windows NSIS setup.exe asset; found $($matches.Count). Available assets: $available"
  }

  return $matches[0]
}

function Install-GSwitch {
  [CmdletBinding()]
  param()

  try {
    $release = Invoke-RestMethod -Uri $ReleaseApi -Headers $GitHubHeaders
  }
  catch {
    throw "Could not find the latest public GSwitch release. Try again later or download the Windows installer from $ReleasesPage."
  }

  $asset = Select-GSwitchInstallerAsset -Assets @($release.assets)
  $downloadUrl = [string]$asset.browser_download_url
  if ([string]::IsNullOrWhiteSpace($downloadUrl)) {
    throw 'The selected GSwitch installer does not have a download URL.'
  }

  $installerPath = Join-Path ([System.IO.Path]::GetTempPath()) (
    "GSwitch-$([System.Guid]::NewGuid().ToString('N'))-setup.exe"
  )

  try {
    try {
      Invoke-WebRequest -Uri $downloadUrl -Headers $GitHubHeaders -OutFile $installerPath
    }
    catch {
      throw "Could not download the GSwitch installer. Try again or download it from $ReleasesPage."
    }

    $installer = Start-Process -FilePath $installerPath -Wait -PassThru
    if ($installer.ExitCode -ne 0) {
      throw "The GSwitch installer failed with exit code $($installer.ExitCode)."
    }
  }
  finally {
    if (Test-Path -LiteralPath $installerPath) {
      Remove-Item -LiteralPath $installerPath -Force
    }
  }
}

if ($MyInvocation.InvocationName -ne '.') {
  Install-GSwitch
}
