param(
    [Parameter(Mandatory)][string]$Image,
    [Parameter(Mandatory)][ValidatePattern('^[a-z0-9-]+$')][string]$Label,
    [Parameter(Mandatory)][ValidateSet('hplus', 'hplus-warm', 'hplus-admin', 'hplus-dashboard', 'lan', 'lan-preparation', 'stress')][string]$Mode,
    [ValidateSet('small', 'large')][string]$Fixture = 'small',
    [string[]]$Migrations = @(),
    [switch]$ReclaimEvents,
    [switch]$IsolateDatabase,
    [int]$AppPort = 48617,
    [int]$ProxyPort = 48618
)

$ErrorActionPreference = 'Stop'
Set-Location (Resolve-Path "$PSScriptRoot/../..").Path
$templateDatabase = 'wartaq-perf-db'
$labDatabase = if ($IsolateDatabase) { 'wartaq-perf-run-db' } else { $templateDatabase }
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

$launchKey = [guid]::NewGuid().ToString('N') + [guid]::NewGuid().ToString('N')
$ownsDatabase = $false
$dumpPath = ".artifacts/performance/$Label-fixture.dump"
try {
    if ($IsolateDatabase) {
        Invoke-LabDocker run --detach --rm --name $labDatabase --network wartaq-perf-lab --cpus 1.0 --memory 512m `
            -e 'POSTGRES_HOST_AUTH_METHOD=trust' -e "POSTGRES_DB=$copy" postgres:17
        $ownsDatabase = $true
        for ($attempt = 0; $attempt -lt 30; $attempt++) {
            docker exec $labDatabase pg_isready -h 127.0.0.1 -U postgres -d $copy -q
            if (!$LASTEXITCODE) { break }
            Start-Sleep -Seconds 1
        }
        if ($LASTEXITCODE) { throw 'The isolated database did not start.' }
        Invoke-LabDocker exec $templateDatabase pg_dump -U postgres -Fc -f /tmp/wartaq-fixture.dump $template
        Invoke-LabDocker cp "$($templateDatabase):/tmp/wartaq-fixture.dump" $dumpPath
        Invoke-LabDocker cp $dumpPath "$($labDatabase):/tmp/wartaq-fixture.dump"
        Invoke-LabDocker exec $labDatabase pg_restore -U postgres -d $copy --exit-on-error /tmp/wartaq-fixture.dump
    } else {
        Invoke-LabDocker exec $labDatabase createdb -U postgres -T $template $copy
        $ownsDatabase = $true
    }
    foreach ($migration in $Migrations) {
        Invoke-LabDocker cp (Resolve-Path -LiteralPath $migration).Path "$($labDatabase):/tmp/wartaq-run-migration.sql"
        Invoke-LabDocker exec $labDatabase psql -U postgres -d $copy -v ON_ERROR_STOP=1 -q -f /tmp/wartaq-run-migration.sql
    }
    if ($ReclaimEvents) {
        Invoke-LabDocker exec $labDatabase psql -U postgres -d $copy -v ON_ERROR_STOP=1 -q -c 'VACUUM (FULL, ANALYZE) public.events'
    }
    $runArgs = @(
        'run', '--detach', '--rm', '--name', $labWeb, '--network', 'wartaq-perf-lab',
        '--cpus', '1.0', '--memory', '512m', '-p', "127.0.0.1:$($AppPort):3000",
        '-e', "DATABASE_URL=postgres://postgres@$($labDatabase):5432/$copy",
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
        if ($ownsDatabase) {
            if ($IsolateDatabase) { Invoke-LabDocker stop $labDatabase }
            else { Invoke-LabDocker exec $labDatabase dropdb -U postgres $copy }
        }
        if (Test-Path -LiteralPath $dumpPath) { Remove-Item -LiteralPath $dumpPath }
    }
}
