$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Set-Location -LiteralPath $repoRoot

$source = Get-Content -LiteralPath (Join-Path $repoRoot 'demo/api.ts') -Raw
$emails = [regex]::Matches($source, 'email: "([^"]+)"') | ForEach-Object { $_.Groups[1].Value }
if ($emails.Count -lt 6 -or @($emails | Where-Object { $_ -notmatch '^[a-z0-9._+-]+@example\.(com|org|net)$' }).Count -gt 0) {
  throw 'The demo must contain at least six reserved-domain fictional emails and no personal email.'
}

pnpm exec vite build --configLoader runner --mode demo
if ($LASTEXITCODE -ne 0) { throw 'The isolated demo build failed.' }

$server = Start-Process -FilePath (Get-Command node).Source -ArgumentList @(
  'node_modules/vite/bin/vite.js', 'preview', '--configLoader', 'runner',
  '--host', '127.0.0.1', '--port', '1420', '--strictPort'
) -WorkingDirectory $repoRoot -WindowStyle Hidden -PassThru
try {
  $ready = $false
  for ($attempt = 0; $attempt -lt 30; $attempt++) {
    try {
      $response = Invoke-WebRequest -Uri 'http://127.0.0.1:1420/' -UseBasicParsing -TimeoutSec 1
      if ($response.StatusCode -eq 200) { $ready = $true; break }
    } catch { Start-Sleep -Milliseconds 300 }
  }
  if (-not $ready) { throw 'The isolated demo preview did not start.' }

  $env:npm_config_cache = Join-Path $repoRoot 'src-tauri/target/npm-cache'
  npx --yes playwright screenshot --channel=msedge --lang=en-US --timezone=Asia/Shanghai `
    --viewport-size='1440,850' --wait-for-selector='article.account-card:nth-child(6)' `
    --wait-for-timeout=700 --full-page 'http://127.0.0.1:1420/' 'assets/readme-demo.png'
  if ($LASTEXITCODE -ne 0) { throw 'The demo screenshot failed.' }
  Write-Output 'Review assets/readme-demo.png visually before committing it.'
} finally {
  Stop-Process -Id $server.Id -ErrorAction SilentlyContinue
}
