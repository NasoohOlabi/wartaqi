[CmdletBinding()]
param(
  [string]$ModelOrigin = 'http://127.0.0.1:8081',
  [string]$Question
)

$ErrorActionPreference = 'Stop'
$model = $null
if (-not [Uri]::TryCreate($ModelOrigin, [UriKind]::Absolute, [ref]$model) -or
    $model.Scheme -ne 'http' -or $model.Host -notin @('127.0.0.1', 'localhost') -or
    $model.UserInfo -or $model.AbsolutePath -ne '/' -or $model.Query -or $model.Fragment) {
  throw 'ModelOrigin must be a plain HTTP loopback origin.'
}
$modelOriginValue = $model.GetLeftPart([UriPartial]::Authority)
$models = Invoke-RestMethod -Uri "$modelOriginValue/v1/models" -TimeoutSec 5
if ('prism-ml/bonsai-27b' -notin @($models.data.id)) {
  throw 'LM Studio does not list prism-ml/bonsai-27b.'
}

$root = Split-Path -Parent $PSScriptRoot
$containerName = 'wartaq-agent-test-' + [guid]::NewGuid().ToString('N')
$reportPath = Join-Path ([IO.Path]::GetTempPath()) "$containerName.json"
$containerId = $null
$variables = @('DATABASE_URL_UNPOOLED', 'TEST_RAILWAY_DATABASE_URL', 'TEST_LM_STUDIO_ORIGIN', 'TEST_TELEGRAM_AGENT_QUESTION')
$previous = @{}
foreach ($name in $variables) {
  $item = Get-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue
  $previous[$name] = if ($null -eq $item) { $null } else { $item.Value }
}
Push-Location $root
try {
  $containerId = (& docker run --detach --rm --name $containerName --publish '127.0.0.1::5432' --env POSTGRES_HOST_AUTH_METHOD=trust --env POSTGRES_DB=notebook_test postgres:17)
  if ($LASTEXITCODE -ne 0 -or $containerId -notmatch '^[a-f0-9]{64}$') {
    throw 'Could not create disposable PostgreSQL container.'
  }
  $ready = $false
  for ($attempt = 0; $attempt -lt 60; $attempt++) {
    & docker exec $containerId pg_isready -U postgres -d notebook_test *> $null
    if ($LASTEXITCODE -eq 0) { $ready = $true; break }
    Start-Sleep -Seconds 1
  }
  if (-not $ready) { throw 'Disposable PostgreSQL did not become ready.' }
  $binding = (& docker port $containerId '5432/tcp')
  if ($LASTEXITCODE -ne 0 -or $binding -notmatch '^127\.0\.0\.1:(\d+)$') {
    throw 'Expected exactly one loopback PostgreSQL port binding.'
  }
  $port = [int]$Matches[1]
  foreach ($migration in @('railway/001-auth.sql', 'railway/002-mcp-oauth.sql')) {
    Get-Content -LiteralPath (Join-Path $root $migration) -Raw |
      & docker exec --interactive $containerId psql -U postgres -d notebook_test -v ON_ERROR_STOP=1
    if ($LASTEXITCODE -ne 0) { throw "Migration failed: $migration" }
  }
  $env:DATABASE_URL_UNPOOLED = "postgres://postgres@127.0.0.1:$port/notebook_test"
  $env:TEST_RAILWAY_DATABASE_URL = $env:DATABASE_URL_UNPOOLED
  $env:TEST_LM_STUDIO_ORIGIN = $modelOriginValue
  [Environment]::SetEnvironmentVariable('TEST_TELEGRAM_AGENT_QUESTION', $Question, 'Process')
  & npm run db:migrate
  if ($LASTEXITCODE -ne 0) { throw 'Application migrations failed.' }

  # The JSON report distinguishes a passing test from a suite skipped by its environment gate.
  $errorPreference = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  try {
    $output = & npm test -- tests/telegram-agent-live-integration.test.ts --reporter=default --reporter=json "--outputFile.json=$reportPath" 2>&1
    $testExitCode = $LASTEXITCODE
  } finally {
    $ErrorActionPreference = $errorPreference
  }
  $output | ForEach-Object {
    $line = $_.ToString()
    if ($line -and $line -ne 'System.Management.Automation.RemoteException') { Write-Host $line }
  }
  if ($testExitCode -ne 0 -or -not (Test-Path -LiteralPath $reportPath)) {
    throw 'Headless integration failed or did not produce a test report.'
  }
  $summary = Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json
  if ($summary.numTotalTests -ne 1 -or $summary.numPassedTests -ne 1) {
    throw 'Headless integration failed or did not run exactly one test.'
  }
  Write-Host 'Headless Telegram agent integration passed.'
} finally {
  Remove-Item -LiteralPath $reportPath -ErrorAction SilentlyContinue
  foreach ($name in $variables) {
    [Environment]::SetEnvironmentVariable($name, $previous[$name], 'Process')
  }
  $cleanupFailed = $false
  if ($containerId -match '^[a-f0-9]{64}$') {
    & docker rm --force $containerId | Out-Null
    $cleanupFailed = $LASTEXITCODE -ne 0
  }
  Pop-Location
  if ($cleanupFailed) { throw "Could not remove disposable container $containerName." }
}
