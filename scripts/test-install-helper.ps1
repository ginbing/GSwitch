# Safe boundary checks: real hashing/temporary-file cleanup, simulated network
# and installer launch. No installer bytes are downloaded or executed.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '../install.ps1')

$script:FixtureBytes = [System.Text.Encoding]::UTF8.GetBytes('GSwitch synthetic installer payload')
$hasher = [System.Security.Cryptography.SHA256]::Create()
try { $script:FixtureHash = ([BitConverter]::ToString($hasher.ComputeHash($script:FixtureBytes))).Replace('-', '') }
finally { $hasher.Dispose() }

function Assert-Condition($Condition, [string]$Message) {
  if (-not $Condition) { throw $Message }
}

function Invoke-RestMethod {
  param($Uri, $Headers)
  Assert-Condition ($Uri -eq 'https://api.github.com/repos/ginbing/GSwitch/releases/latest') 'Unexpected metadata endpoint'
  if ($script:Case.Mode -eq 'metadata-failure') { throw 'Synthetic network failure' }
  $asset = [pscustomobject]@{ name = 'GSwitch_0.0.0_x64-setup.exe'; browser_download_url = 'https://example.invalid/synthetic-setup.exe' }
  if ($script:Case.ContainsKey('Digest')) { $asset | Add-Member NoteProperty digest $script:Case.Digest }
  $assets = @($asset)
  if ($script:Case.Mode -eq 'multiple-assets') { $assets = @($asset, $asset) }
  return [pscustomobject]@{ assets = $assets }
}

function Invoke-WebRequest {
  param($Uri, $Headers, $OutFile)
  Assert-Condition ($Uri -eq 'https://example.invalid/synthetic-setup.exe') 'Unexpected download endpoint'
  $script:Downloads++
  $script:DownloadedPath = $OutFile
  Assert-Condition ([System.IO.Path]::GetFullPath($OutFile).StartsWith([System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()), [System.StringComparison]::OrdinalIgnoreCase)) 'Download escaped temporary storage'
  [System.IO.File]::WriteAllBytes($OutFile, $script:FixtureBytes)
  if ($script:Case.Mode -eq 'download-failure') { throw 'Synthetic partial download failure' }
}

function Get-FileHash {
  param($LiteralPath, $Algorithm)
  $script:HashReads++
  Assert-Condition ($Algorithm -eq 'SHA256') 'Wrong hash algorithm'
  if ($script:Case.Mode -eq 'hash-failure') { throw 'Synthetic unreadable file' }
  Microsoft.PowerShell.Utility\Get-FileHash -LiteralPath $LiteralPath -Algorithm $Algorithm
}

function Start-Process {
  param($FilePath, [switch]$Wait, [switch]$PassThru)
  $script:Launches++
  Assert-Condition ($Wait -and $PassThru) 'Installer lifecycle changed'
  Assert-Condition ($script:HashReads -eq 1) 'Installer launched before verification'
  Assert-Condition ($FilePath -eq $script:DownloadedPath) 'Installer path changed after verification'
  Assert-Condition ((Microsoft.PowerShell.Utility\Get-FileHash -LiteralPath $FilePath -Algorithm SHA256).Hash -eq $script:FixtureHash) 'Launched different bytes'
  if ($script:Case.Mode -eq 'launch-failure') { throw 'Synthetic launch failure' }
  $exitCode = if ($script:Case.Mode -eq 'installer-failure') { 7 } else { 0 }
  [pscustomobject]@{ ExitCode = $exitCode }
}

$cases = @(
  @{ Mode = 'matching'; Digest = "sha256:$script:FixtureHash"; Launches = 1; Success = $true },
  @{ Mode = 'matching-lowercase'; Digest = "sha256:$($script:FixtureHash.ToLowerInvariant())"; Launches = 1; Success = $true },
  @{ Mode = 'mismatch'; Digest = ('sha256:' + ('0' * 64)); Launches = 0 },
  @{ Mode = 'missing'; Launches = 0 },
  @{ Mode = 'null'; Digest = $null; Launches = 0 },
  @{ Mode = 'numeric'; Digest = 42; Launches = 0 },
  @{ Mode = 'unsupported'; Digest = ('sha512:' + ('a' * 64)); Launches = 0 },
  @{ Mode = 'short'; Digest = 'sha256:abc'; Launches = 0 },
  @{ Mode = 'bad-hex'; Digest = ('sha256:' + ('g' * 64)); Launches = 0 },
  @{ Mode = 'trailing-newline'; Digest = "sha256:$script:FixtureHash`n"; Launches = 0 },
  @{ Mode = 'metadata-failure'; Launches = 0 },
  @{ Mode = 'multiple-assets'; Digest = "sha256:$script:FixtureHash"; Launches = 0 },
  @{ Mode = 'download-failure'; Digest = "sha256:$script:FixtureHash"; Launches = 0 },
  @{ Mode = 'hash-failure'; Digest = "sha256:$script:FixtureHash"; Launches = 0 },
  @{ Mode = 'launch-failure'; Digest = "sha256:$script:FixtureHash"; Launches = 1 },
  @{ Mode = 'installer-failure'; Digest = "sha256:$script:FixtureHash"; Launches = 1 }
)

foreach ($script:Case in $cases) {
  $script:Downloads = 0
  $script:Launches = 0
  $script:HashReads = 0
  $script:DownloadedPath = $null
  $succeeded = $false
  $failure = $null
  try {
    try { Install-GSwitch; $succeeded = $true } catch { $failure = $_ }
    $expectedSuccess = $script:Case.ContainsKey('Success') -and $script:Case.Success
    Assert-Condition ($succeeded -eq $expectedSuccess) "Wrong result for $($script:Case.Mode): $failure"
    Assert-Condition ($script:Launches -eq $script:Case.Launches) "Wrong launch count for $($script:Case.Mode)"
    $expectedDownloads = if (@('matching', 'matching-lowercase', 'mismatch', 'download-failure', 'hash-failure', 'launch-failure', 'installer-failure') -contains $script:Case.Mode) { 1 } else { 0 }
    Assert-Condition ($script:Downloads -eq $expectedDownloads) "Wrong download count for $($script:Case.Mode)"
    if ($script:DownloadedPath) {
      Assert-Condition (-not (Test-Path -LiteralPath $script:DownloadedPath)) "Temporary file remains for $($script:Case.Mode)"
    }
    Write-Output "PASS $($script:Case.Mode)"
  }
  finally {
    if ($script:DownloadedPath -and (Test-Path -LiteralPath $script:DownloadedPath)) {
      Remove-Item -LiteralPath $script:DownloadedPath -Force
    }
  }
}
Write-Output "Passed $($cases.Count) installer integrity cases; no installer was launched."
