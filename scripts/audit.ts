import { spawnSync } from 'node:child_process'
import { z } from 'zod'

const advisoryUrl = 'https://github.com/advisories/GHSA-fc5q-39xc-vjpf'
const expires = Date.parse('2026-12-01T00:00:00Z')
const reportSchema = z.record(z.string(), z.array(z.looseObject({ url: z.url() })).min(1))

export function reviewAudit(report: unknown, now = Date.now()) {
  const findings = Object.entries(reportSchema.parse(report)).flatMap(([name, advisories]) =>
    advisories.map((advisory) => ({ name, advisory })),
  )
  if (findings.some(({ name, advisory }) => name !== 'picomatch' || advisory.url !== advisoryUrl))
    throw new Error('Unreviewed dependency advisory; see the full audit report above')
  if (findings.length && now >= expires)
    throw new Error('The development-only picomatch exception expired; review SECURITY.md')
  return findings.length
}

if (import.meta.main) {
  const audit = spawnSync('bun', ['audit', '--json'], {
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
    const production = spawnSync('bun', ['audit', '--prod'], {
      stdio: 'inherit',
    })
    if (production.error) throw production.error
    if (production.status !== 0) throw new Error('Production dependency audit failed')
  }
}
