import { spawn } from 'node:child_process'
import { z } from 'zod'

const health = z.object({ ok: z.literal(true) })

export async function startRustBackend(
  binary: string,
  databaseUrl: string,
  fixtureOrigin: string | { hybridOrigin: string; port?: string; native?: boolean },
  authSecret: string,
  workspaceOrigin = typeof fixtureOrigin === 'string' ? fixtureOrigin : fixtureOrigin.hybridOrigin,
  telegramWorkerSecret?: string,
) {
  const hybrid = typeof fixtureOrigin !== 'string'
  const child = spawn(binary, [], {
    env: {
      ...process.env,
      PORT: hybrid ? (fixtureOrigin.port ?? '0') : '0',
      BETTER_AUTH_URL: hybrid ? workspaceOrigin : undefined,
      ...(hybrid && { ASSET_DIR: 'dist' }),
      WORKSPACE_ORIGIN: workspaceOrigin,
      DATABASE_URL: databaseUrl,
      BETTER_AUTH_SECRET: authSecret,
      ...(telegramWorkerSecret && { TELEGRAM_WORKER_SECRET: telegramWorkerSecret }),
    },
    stdio: ['ignore', 'pipe', 'inherit'],
  })
  const exited = new Promise<void>((resolve) => child.once('close', () => resolve()))
  async function close() {
    if (child.pid && child.exitCode === null && child.signalCode === null) child.kill()
    await exited
  }
  const deadline = AbortSignal.timeout(10000)
  try {
    const ready = async () => {
      let output = ''
      for await (const chunk of child.stdout.setEncoding('utf8')) {
        if (typeof chunk !== 'string') continue
        output += chunk
        const match = /^Listening on (http:\/\/127\.0\.0\.1:\d+)\r?$/m.exec(output)
        if (match) return match[1]
      }
      throw new Error('Rust backend exited before publishing its address')
    }
    const origin = await Promise.race([
      ready(),
      new Promise<never>((_, reject) => child.once('error', reject)),
      new Promise<never>((_, reject) => {
        deadline.addEventListener('abort', () => reject(deadline.reason), { once: true })
      }),
    ])
    const response = await fetch(`${origin}/health`, { signal: deadline })
    if (!response.ok) throw new Error(`Rust backend health check returned ${response.status}`)
    health.parse(await response.json())
    return { origin, child, exited, close }
  } catch (error) {
    await close()
    throw error
  }
}
