param(
    [Parameter(Mandatory)][string]$Image,
    [Parameter(Mandatory)][ValidatePattern('^[a-z0-9-]+$')][string]$Label,
    [Parameter(Mandatory)][ValidateSet('hplus', 'lan', 'stress')][string]$Mode,
    [ValidateSet('small', 'large')][string]$Fixture = 'small',
    [string[]]$Migrations = @(),
    [int]$AppPort = 48617,
    [int]$ProxyPort = 48618
)

$ErrorActionPreference = 'Stop'
Set-Location (Resolve-Path "$PSScriptRoot/../..").Path
$labDatabase = 'wartaq-perf-db'
$labWeb = 'wartaq-perf-web'
$shortLabel = $Label.Replace('-', '_')
$copy = "wartaq_run_$shortLabel"
$template = if ($Fixture -eq 'small') { 'wartaq_perf_small' } else { 'wartaq_perf' }

function Invoke-LabDocker {
    & docker @args
    if ($LASTEXITCODE) { throw "Docker command failed ($LASTEXITCODE): $($args[0])" }
}

if (!(Test-Path -LiteralPath '.artifacts/performance/fixture-password.local')) {
    throw 'Prepare the synthetic fixture and its password file first.'
}

if (docker ps -a --filter "name=^/$labWeb$" --format '{{.Names}}') {
    throw "Stop the previous task-owned $labWeb container before starting a repeat."
}

Invoke-LabDocker exec $labDatabase createdb -U postgres -T $template $copy
$launchKey = [guid]::NewGuid().ToString('N') + [guid]::NewGuid().ToString('N')
try {
    foreach ($migration in $Migrations) {
        Invoke-LabDocker cp (Resolve-Path -LiteralPath $migration).Path "$($labDatabase):/tmp/wartaq-run-migration.sql"
        Invoke-LabDocker exec $labDatabase psql -U postgres -d $copy -v ON_ERROR_STOP=1 -q -f /tmp/wartaq-run-migration.sql
    }
    $runArgs = @(
        'run', '--detach', '--rm', '--name', $labWeb, '--network', 'wartaq-perf-lab',
        '--cpus', '1.0', '--memory', '512m', '-p', "127.0.0.1:$($AppPort):3000",
        '-e', "DATABASE_URL=postgres://postgres@wartaq-perf-db:5432/$copy",
        '-e', "BETTER_AUTH_URL=http://127.0.0.1:$ProxyPort",
        '-e', "BETTER_AUTH_SECRET=$launchKey", $Image
    )
    Invoke-LabDocker @runArgs
    curl.exe --retry 10 --retry-delay 1 --retry-all-errors -fsS "http://127.0.0.1:$AppPort/health"
    if ($LASTEXITCODE) { throw 'The application failed its local Docker health check.' }
    $env:WARTAQ_PERF_WEB = $labWeb
    $env:WARTAQ_PERF_DB = $labDatabase
    if ($Mode -eq 'stress') {
        node scripts/performance/stress.mjs $Label $ProxyPort $AppPort
    } else {
        node scripts/performance/journey.mjs $Label $Mode $ProxyPort $AppPort
    }
    if ($LASTEXITCODE) { throw "Benchmark $Label failed ($LASTEXITCODE)." }
} finally {
    try {
        if (docker ps --filter "name=^/$labWeb$" --format '{{.Names}}') {
            Invoke-LabDocker stop $labWeb
        }
    } finally {
        Invoke-LabDocker exec $labDatabase dropdb -U postgres $copy
    }
}
