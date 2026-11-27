param([int]$ThroughMigration = [int]::MaxValue)

$ErrorActionPreference = 'Stop'
Set-Location (Resolve-Path "$PSScriptRoot/../..").Path
$labDatabase = 'wartaq-perf-db'
$labNetwork = 'wartaq-perf-lab'
$fixtures = @(
    @{ name = 'wartaq_perf_small'; students = 48; days = 24 },
    @{ name = 'wartaq_perf'; students = 480; days = 120 }
)

function Invoke-LabDocker {
    & docker @args
    if ($LASTEXITCODE) { throw "Docker command failed ($LASTEXITCODE): $($args[0])" }
}

if (docker ps -a --filter "name=^/$labDatabase$" --format '{{.Names}}') {
    throw "The benchmark database already exists. Reuse the templates or stop $labDatabase first."
}
if (!(docker network ls --filter "name=^$labNetwork$" --format '{{.Name}}')) {
    Invoke-LabDocker network create $labNetwork
}
$runArgs = @(
    'run', '--detach', '--rm', '--name', $labDatabase, '--network', $labNetwork,
    '--cpus', '1.0', '--memory', '512m', '-e', 'POSTGRES_HOST_AUTH_METHOD=trust', 'postgres:17'
)
Invoke-LabDocker @runArgs

for ($attempt = 0; $attempt -lt 30; $attempt++) {
    docker exec $labDatabase pg_isready -U postgres -q
    if (!$LASTEXITCODE) { break }
    Start-Sleep -Seconds 1
}
if ($LASTEXITCODE) { throw 'The synthetic PostgreSQL container never became ready.' }

$artifactRoot = '.artifacts/performance'
New-Item -ItemType Directory -Force -Path $artifactRoot | Out-Null
$env:WARTAQ_PERF_PASSWORD = [guid]::NewGuid().ToString('N') + 'Perf!'
Set-Content -NoNewline -LiteralPath "$artifactRoot/fixture-password.local" -Value $env:WARTAQ_PERF_PASSWORD
$hash = (bun --eval "import { hashPassword } from 'better-auth/crypto'; console.log(await hashPassword(process.env.WARTAQ_PERF_PASSWORD))").Trim()
if ($LASTEXITCODE -or !$hash) { throw 'Failed to generate a disposable password hash.' }
Remove-Item Env:WARTAQ_PERF_PASSWORD

$migrationFiles = @(Get-ChildItem railway -Filter '*.sql' | Sort-Object Name) +
    @(Get-ChildItem neon -Filter '*.sql' | Where-Object { [int]$_.BaseName.Split('-')[0] -le $ThroughMigration } | Sort-Object Name)
Invoke-LabDocker cp scripts/performance/fixture.sql "$($labDatabase):/tmp/wartaq-fixture.sql"
foreach ($fixture in $fixtures) {
    Invoke-LabDocker exec $labDatabase createdb -U postgres $fixture.name
    foreach ($migration in $migrationFiles) {
        Invoke-LabDocker cp $migration.FullName "$($labDatabase):/tmp/wartaq-migration.sql"
        Invoke-LabDocker exec $labDatabase psql -U postgres -d $fixture.name -v ON_ERROR_STOP=1 -q -f /tmp/wartaq-migration.sql
    }
    $seedArgs = @(
        'exec', $labDatabase, 'psql', '-U', 'postgres', '-d', $fixture.name, '-q',
        '-v', "students=$($fixture.students)", '-v', "days=$($fixture.days)",
        '-v', "password_hash=$hash", '-f', '/tmp/wartaq-fixture.sql'
    )
    Invoke-LabDocker @seedArgs
}
