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
$cargoRegistry = Join-Path $env:USERPROFILE '.cargo/registry'
if (!(Test-Path -LiteralPath $cargoRegistry)) { throw 'Run cargo fetch --locked --manifest-path rust-sync/Cargo.toml on the host first.' }
$stageName = 'image-' + [guid]::NewGuid().ToString('N')
$stagePath = Join-Path $artifactRoot $stageName
New-Item -ItemType Directory -Path $stagePath | Out-Null
npm run typecheck
if ($LASTEXITCODE) { throw 'The frontend typecheck failed.' }
npm exec -- vite build --outDir "$stagePath/dist"
if ($LASTEXITCODE) { throw 'The frontend build failed.' }
node scripts/precompress-assets.mjs "$stagePath/dist"
if ($LASTEXITCODE) { throw 'Asset precompression failed.' }
$buildContainerArgs = @(
    'run', '--rm', '--network', 'none',
    '--mount', "type=bind,source=$repoRoot,target=/source",
    '--mount', "type=bind,source=$cargoRegistry,target=/usr/local/cargo/registry",
    '--mount', 'type=volume,source=wartaq-perf-rust-target,target=/target',
    '-e', 'CARGO_TARGET_DIR=/target', '-e', "WARTAQ_STAGE=/source/.artifacts/performance/$stageName",
    'rust:1.93-bookworm', 'sh', '-ec',
    'cargo build --release --locked --offline --manifest-path /source/rust-sync/Cargo.toml; cp /target/release/wartaq-workspace-relay "$WARTAQ_STAGE/wartaq-web"'
)
docker @buildContainerArgs
if ($LASTEXITCODE) { throw 'The offline Rust release build failed.' }

@'
FROM debian:bookworm-slim
WORKDIR /app
ENV NODE_ENV=production ASSET_DIR=/app/dist PORT=3000
COPY dist ./dist
COPY --chmod=755 wartaq-web /usr/local/bin/wartaq-web
USER 65532:65532
CMD ["wartaq-web"]
'@ | Set-Content -Encoding Ascii -LiteralPath "$stagePath/Dockerfile"
docker build --network none -t $Tag $stagePath
if ($LASTEXITCODE) { throw 'The benchmark Docker image did not build.' }
