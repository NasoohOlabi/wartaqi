import { expect, test } from 'vitest-runner'
import { reviewAudit } from '../scripts/audit'

const url = 'https://github.com/advisories/GHSA-fc5q-39xc-vjpf'
const beforeExpiry = Date.parse('2026-11-17T00:00:00Z')
const picomatch = [{ url, severity: 'high' }]

test('a clean audit needs no exception, including after its expiry', () => {
  expect(reviewAudit({}, Date.parse('2026-12-02T00:00:00Z'))).toBe(0)
})

test('only the exact reviewed picomatch advisory is eligible for the production check', () => {
  expect(reviewAudit({ picomatch }, beforeExpiry)).toBe(1)
  for (const report of [
    { another: picomatch },
    { picomatch: [{ url: 'https://example.org/' }] },
    { unrelated: [{ url: 'https://example.org/' }], picomatch },
  ])
    expect(() => reviewAudit(report, beforeExpiry)).toThrow('Unreviewed')
})

test('the exception fails at its expiry boundary', () => {
  expect(() => reviewAudit({ picomatch }, Date.parse('2026-12-01T00:00:00Z'))).toThrow('expired')
})

test('malformed or incomplete audit reports fail closed', () => {
  for (const value of [null, [], { picomatch: [{}] }, { picomatch: [] }, { error: 'unavailable' }])
    expect(() => reviewAudit(value, beforeExpiry)).toThrow()
})
