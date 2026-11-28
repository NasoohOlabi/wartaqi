import { spawnSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { join } from 'node:path'

const binary = join(
  '.artifacts',
  'tools',
  'gitleaks',
  process.platform === 'win32' ? 'gitleaks.exe' : 'gitleaks',
)
if (!existsSync(binary))
  throw new Error('Secret scanner missing. Run npm run setup:security. Checks fail closed.')
const mode = process.argv[2]
if (mode === 'staged' || mode === 'history') {
  const listing = spawnSync(
    'git',
    mode === 'staged'
      ? ['diff', '--cached', '--name-only', '--diff-filter=ACMR']
      : ['log', '--all', '--format=', '--name-only'],
    { stdio: ['ignore', 'pipe', 'inherit'] },
  )
  if (listing.status !== 0) process.exit(listing.status ?? 1)
  const blocked = new TextDecoder()
    .decode(listing.stdout)
    .split(/\r?\n/)
    .filter(
      (path) =>
        /(^|\/)(\.local|\.env(?:\..*)?|credentials\.json|serviceAccount[^/]*\.json)(\/|$)|\.(pem|key|csv)$/i.test(
          path,
        ) && !path.endsWith('.env.example'),
    )
  if (blocked.length)
    throw new Error(`Private data paths cannot be published: ${[...new Set(blocked)].join(', ')}`)
}
const args =
  mode === 'staged'
    ? ['git', '--staged']
    : mode === 'history'
      ? ['git', '--log-opts=--all']
      : mode === 'files'
        ? ['dir', 'dist']
        : null
if (!args) throw new Error('Use staged, history, or files')
// No verbose findings: scanners must never print matched credentials.
const result = spawnSync(binary, [...args, '--redact', '--no-banner'], {
  stdio: 'inherit',
})
process.exit(result.status ?? 1)
