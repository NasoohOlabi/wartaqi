param([ValidatePattern('^[a-z0-9-]+$')][string]$Label = 'storage')

$ErrorActionPreference = 'Stop'
Set-Location (Resolve-Path "$PSScriptRoot/../..").Path
$database = 'wartaq-perf-db'
$prefix = 'wartaq_layout_' + [guid]::NewGuid().ToString('N').Substring(0, 12)
$created = @()

function Invoke-LabDocker {
    & docker @args
    if ($LASTEXITCODE) { throw "Docker command failed ($LASTEXITCODE): $($args[0])" }
}

function Measure-Layout($name, $payload) {
    Invoke-LabDocker exec $database psql -U postgres -d $name -v ON_ERROR_STOP=1 -q -c 'VACUUM (FULL, ANALYZE) public.events'
    $sql = "SELECT json_build_object('bytes',pg_database_size(current_database()),'events',count(*),'digest',md5(string_agg(id::text || ':' || ($payload)::text, ',' ORDER BY sequence))) FROM events"
    Invoke-LabDocker exec $database psql -U postgres -d $name -v ON_ERROR_STOP=1 -qAt -c $sql | ConvertFrom-Json
}

try {
    foreach ($name in @(($prefix + '_baseline'), ($prefix + '_candidate'))) {
        Invoke-LabDocker exec $database createdb -U postgres -T wartaq_perf $name
        $created += $name
    }
    $baseline = Measure-Layout $created[0] 'payload'
    $migration = '/tmp/wartaq-compare-storage.sql'
    foreach ($file in @('neon/023-compact-attendance.sql', 'neon/025-enable-attendance-packing.sql')) {
        Invoke-LabDocker cp $file "$($database):$migration"
        Invoke-LabDocker exec $database psql -U postgres -d $created[1] -v ON_ERROR_STOP=1 -1 -q -f $migration
    }
    $candidate = Measure-Layout $created[1] 'wartaq_event_payload(payload,payload_packed,student_id,enrollment_id)'
    if ($baseline.events -ne $candidate.events -or $baseline.digest -ne $candidate.digest) {
        throw 'The canonical event ledger changed.'
    }
    $report = [pscustomobject]@{
        baseline = $baseline
        candidate = $candidate
        percentSaved = [math]::Round(100 * (1 - $candidate.bytes / $baseline.bytes), 2)
    }
    $report | ConvertTo-Json | Tee-Object -FilePath ".artifacts/performance/$Label-storage.json"
} finally {
    foreach ($name in $created) { Invoke-LabDocker exec $database dropdb -U postgres $name }
}
