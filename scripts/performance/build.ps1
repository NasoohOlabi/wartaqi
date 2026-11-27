param(
    [Parameter(Mandatory)][ValidatePattern('^[a-z0-9][a-z0-9:._/-]+$')][string]$Tag,
    [switch]$UseCachedRuntime
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path "$PSScriptRoot/../..").Path
Set-Location $repoRoot
if (!$UseCachedRuntime) {
    docker build -t $Tag .
    if ($LASTEXITCODE) { throw 'The production Dockerfile did not build.' }
    return
}

$artifactRoot = Join-Path $repoRoot '.artifacts/performance'
$cacheInput = Join-Path $artifactRoot 'cargo-registry.tar.gz'
if (!(Test-Path -LiteralPath $cacheInput)) { throw 'Cached Cargo registry input is missing.' }
if ((Get-FileHash -LiteralPath bun.lock).Hash -ne '0769C750DBC917707DBC031227DAD40739A75CC7E0CFB9132156ED2732D2767A') {
    throw 'The cached runtime has only been checked against the original Bun lockfile.'
}

$stageName = 'image-' + [guid]::NewGuid().ToString('N')
$stagePath = Join-Path $artifactRoot $stageName
New-Item -ItemType Directory -Path $stagePath | Out-Null
bunx tsc -b
if ($LASTEXITCODE) { throw 'The frontend typecheck failed.' }
bunx vite build --outDir "$stagePath/dist"
if ($LASTEXITCODE) { throw 'The frontend build failed.' }
bun scripts/precompress-assets.mjs "$stagePath/dist"
if ($LASTEXITCODE) { throw 'Asset precompression failed.' }
$buildContainerArgs = @(
    'run', '--rm', '--network', 'none',
    '--mount', "type=bind,source=$repoRoot,target=/source",
    '--mount', 'type=volume,source=wartaq-perf-rust-target,target=/target',
    '-e', 'CARGO_TARGET_DIR=/target', '-e', "WARTAQ_STAGE=/source/.artifacts/performance/$stageName",
    'rust:1.93-bookworm', 'sh', '-ec',
    'tar -xzf /source/.artifacts/performance/cargo-registry.tar.gz -C /usr/local/cargo; cargo build --release --locked --offline --manifest-path /source/rust-sync/Cargo.toml; cp /target/release/wartaq-workspace-relay "$WARTAQ_STAGE/wartaq-web"'
)
docker @buildContainerArgs
if ($LASTEXITCODE) { throw 'The offline Rust release build failed.' }

Copy-Item -Recurse -Path server,shared,functions -Destination $stagePath
Copy-Item -Path package.json,bun.lock -Destination $stagePath
@'
FROM wartaqi:routed-web-current
WORKDIR /app
USER root
RUN rm -rf /app/dist /app/server /app/shared /app/functions /app/node_modules/web-vitals
USER bun
COPY --chown=bun:bun package.json bun.lock ./
COPY --chown=bun:bun dist ./dist
COPY --chown=bun:bun server ./server
COPY --chown=bun:bun shared ./shared
COPY --chown=bun:bun functions/notebook-service.ts ./functions/notebook-service.ts
COPY --chmod=755 wartaq-web /usr/local/bin/wartaq-web
'@ | Set-Content -Encoding Ascii -LiteralPath "$stagePath/Dockerfile"
docker build --network none -t $Tag $stagePath
if ($LASTEXITCODE) { throw 'The benchmark Docker image did not build.' }
