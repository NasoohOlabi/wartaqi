import { spawnSync } from 'node:child_process'
import { z } from 'zod'

const advisoryUrl = 'https://github.com/advisories/GHSA-fc5q-39xc-vjpf'
const expires = Date.parse('2026-12-01T00:00:00Z')
const finding = z.union([z.string().min(1), z.object({ name: z.string(), url: z.url() })])
const reportSchema = z.object({
  auditReportVersion: z.literal(2),
  vulnerabilities: z.record(
    z.string(),
    z.object({ name: z.string(), via: z.array(finding).min(1) }),
  ),
  metadata: z.object({ vulnerabilities: z.object({ total: z.number().int().nonnegative() }) }),
})

export function reviewAudit(report: unknown, now = Date.now()) {
  const { vulnerabilities, metadata } = reportSchema.parse(report)
  const packages = Object.keys(vulnerabilities)
  const unreviewed = () =>
    new Error('Unreviewed dependency advisory; see the full audit report above')
  if (packages.length !== metadata.vulnerabilities.total) throw new Error('Incomplete audit report')
  if (!packages.length) return 0

  const reviewed = new Set<string>()
  while (reviewed.size < packages.length) {
    const before = reviewed.size
    for (const name of packages) {
      const entry = vulnerabilities[name]
      if (entry.name !== name) throw unreviewed()
      if (
        entry.via.every((item) =>
          typeof item === 'string'
            ? reviewed.has(item)
            : name === 'picomatch' && item.name === name && item.url === advisoryUrl,
        )
      ) {
        reviewed.add(name)
      }
    }
    // npm can list dependants as vulnerabilities via a package name rather than an advisory URL.
    if (reviewed.size === before) throw unreviewed()
  }
  if (now >= expires)
    throw new Error('The development-only picomatch exception expired; review SECURITY.md')
  return packages.length
}

if (import.meta.main) {
  const cli = process.env.npm_execpath
  if (!cli) throw new Error('Run npm run security:audit')
  const audit = spawnSync(process.execPath, [cli, 'audit', '--json'], {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'inherit'],
  })
  if (audit.error) throw audit.error
  const report: unknown = JSON.parse(audit.stdout)
  console.info(JSON.stringify(report, null, 2))
  const findings = reviewAudit(report)
  if (audit.status !== (findings ? 1 : 0)) throw new Error('Unexpected dependency audit result')
  if (findings) {
    console.warn(
      'Verified local picomatch backport; registry advisory above remains excepted until 2026-12-01. Production has no exception. See SECURITY.md.',
    )
    const production = spawnSync(process.execPath, [cli, 'audit', '--omit=dev'], {
      stdio: 'inherit',
    })
    if (production.status !== 0) throw new Error('Production dependency audit failed')
  }
}
