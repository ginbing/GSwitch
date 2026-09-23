$ErrorActionPreference = 'Stop'
$version = (Get-Content src-tauri/tauri.conf.json | ConvertFrom-Json).version
$installer = "src-tauri/target/release/bundle/nsis/GSwitch_${version}_x64-setup.exe"
if (-not (Test-Path -LiteralPath $installer)) { throw "Missing NSIS installer: $installer" }
$install = Join-Path $env:RUNNER_TEMP 'gswitch-release-smoke'
if (Test-Path -LiteralPath $install) { throw "Smoke install path already exists: $install" }
$process = Start-Process -FilePath (Resolve-Path -LiteralPath $installer) -ArgumentList '/S', "/D=$install" -Wait -PassThru -WindowStyle Hidden
if ($process.ExitCode -ne 0) { throw "NSIS install exited $($process.ExitCode)" }
$app = Get-ChildItem -LiteralPath $install -Recurse -File -Filter 'gswitch.exe' | Select-Object -First 1
if (-not $app) { throw 'NSIS install did not contain gswitch.exe' }
if (-not $app.VersionInfo.ProductVersion.StartsWith($version)) { throw "Installed app version does not match $version" }
$uninstaller = Get-ChildItem -LiteralPath $install -Recurse -File -Filter '*uninstall*.exe' | Select-Object -First 1
if (-not $uninstaller) { throw 'NSIS install did not contain an uninstaller' }
$process = Start-Process -FilePath $uninstaller.FullName -ArgumentList '/S' -Wait -PassThru -WindowStyle Hidden
if ($process.ExitCode -ne 0) { throw "NSIS uninstall exited $($process.ExitCode)" }
if (Test-Path -LiteralPath $app.FullName) { throw 'NSIS uninstall left the application executable' }
Write-Output "NSIS install, version, and uninstall passed for $version"
