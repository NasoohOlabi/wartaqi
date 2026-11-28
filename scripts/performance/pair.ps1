param(
    [string]$ControlImage = 'wartaq-perf:candidate',
    [string]$CandidateImage = 'wartaq-perf:candidate',
    [ValidatePattern('^[a-z0-9-]+$')][string]$Label = 'runtime-aa',
    [switch]$KeepRunning
)

$ErrorActionPreference = 'Stop'
Set-Location (Resolve-Path "$PSScriptRoot/../..").Path
$database = 'wartaq-perf-db'
$targets = @(
    @{ name = 'wartaq-pair-control'; db = 'wartaq_pair_control'; port = 48717; image = $ControlImage },
    @{ name = 'wartaq-pair-candidate'; db = 'wartaq_pair_candidate'; port = 48727; image = $CandidateImage }
)
$created = @()
$started = @()

function Invoke-PairDocker {
    & docker @args
    if ($LASTEXITCODE) { throw "Docker command failed ($($args[0])) with code $LASTEXITCODE." }
}

foreach ($target in $targets) {
    if (docker ps -a --filter "name=^/$($target.name)$" --format '{{.Names}}') {
        throw "A previous local pair is still running ($($target.name))."
    }
}

try {
    foreach ($target in $targets) {
        $origin = "http://127.0.0.1:$($target.port)"
        Invoke-PairDocker exec $database createdb -U postgres -T wartaq_perf_small $target.db
        $created += $target
        $secret = [guid]::NewGuid().ToString('N') + [guid]::NewGuid().ToString('N')
        Invoke-PairDocker run --detach --rm --name $target.name --network wartaq-perf-lab --cpus 1.0 --memory 512m `
            -p "127.0.0.1:$($target.port):3000" -e "BETTER_AUTH_URL=$origin" -e "WORKSPACE_ORIGIN=$origin" `
            -e "DATABASE_URL=postgres://postgres@wartaq-perf-db:5432/$($target.db)" -e "BETTER_AUTH_SECRET=$secret" $target.image
        $started += $target
        curl.exe --retry 12 --retry-delay 1 --retry-all-errors -fsS "$origin/health"
        if ($LASTEXITCODE) { throw "Health check failed for $($target.name)." }
    }
    node scripts/performance/pair.mjs $Label $targets[0].port $targets[1].port
    if ($LASTEXITCODE) { throw 'The two containers differed or failed the fixture contract.' }
} finally {
    if (!$KeepRunning) {
        foreach ($target in $started) { Invoke-PairDocker stop $target.name }
        foreach ($target in $created) { Invoke-PairDocker exec $database dropdb -U postgres $target.db }
    }
}
