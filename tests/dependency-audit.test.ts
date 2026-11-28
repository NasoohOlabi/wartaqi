import { expect, test } from 'vitest-runner'
import { reviewAudit } from '../scripts/audit'

const url = 'https://github.com/advisories/GHSA-fc5q-39xc-vjpf'
const beforeExpiry = Date.parse('2026-11-17T00:00:00Z')
const picomatch = { name: 'picomatch', via: [{ name: 'picomatch', url }] }

function report(vulnerabilities: Record<string, unknown>) {
  return {
    auditReportVersion: 2,
    vulnerabilities,
    metadata: { vulnerabilities: { total: Object.keys(vulnerabilities).length } },
  }
}

test('a clean audit needs no exception, including after its expiry', () => {
  expect(reviewAudit(report({}), Date.parse('2026-12-02T00:00:00Z'))).toBe(0)
})

test('only the exact reviewed picomatch advisory is eligible for the production check', () => {
  expect(reviewAudit(report({ picomatch }), beforeExpiry)).toBe(1)
  expect(
    reviewAudit(report({ vite: { name: 'vite', via: ['picomatch'] }, picomatch }), beforeExpiry),
  ).toBe(2)
  for (const vulnerabilities of [
    { another: picomatch },
    { picomatch: { name: 'picomatch', via: [{ name: 'picomatch', url: 'https://example.org/' }] } },
    { vite: { name: 'vite', via: ['picomatch'] } },
    { unrelated: { name: 'unrelated', via: ['unrelated'] }, picomatch },
  ])
    expect(() => reviewAudit(report(vulnerabilities), beforeExpiry)).toThrow('Unreviewed')
})

test('the exception fails at its expiry boundary', () => {
  expect(() => reviewAudit(report({ picomatch }), Date.parse('2026-12-01T00:00:00Z'))).toThrow(
    'expired',
  )
})

test('malformed or incomplete audit reports fail closed', () => {
  for (const value of [
    null,
    [],
    report({ picomatch: {} }),
    { error: 'unavailable' },
    { ...report({}), vulnerabilities: { picomatch } },
  ])
    expect(() => reviewAudit(value, beforeExpiry)).toThrow()
})
