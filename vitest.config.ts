// The npm alias keeps Better Auth's optional Vitest peer out of production installs.
import { defineConfig } from 'vitest-runner/config'

export default defineConfig({
  test: {
    include: ['tests/**/*.test.ts'],
    // Several integration suites share a disposable database and fixed fixture values.
    fileParallelism: false,
  },
})
