import { expect, test } from 'vitest-runner'
import { spawnSync } from 'node:child_process'

test('build tools use the patched ReDoS-safe glob parser', () => {
  const result = spawnSync(process.execPath, ['scripts/check-picomatch.cjs'], {
    encoding: 'utf8',
    timeout: 2000,
  })
  expect(result.stderr).toBe('')
  expect(result.status).toBe(0)
})
