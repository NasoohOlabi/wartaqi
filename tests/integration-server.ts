import { fork } from 'node:child_process'
import { randomBytes } from 'node:crypto'
import { once } from 'node:events'
import { get } from 'node:http'
import { z } from 'zod'
import { listenFetch } from '../server/http'
import { startRustBackend } from './rust-backend-server'

export function navigationRedirect(url: URL, cookie?: string) {
  return new Promise<string | undefined>((resolve, reject) => {
    // Node fetch identifies itself as a CORS fetch, which returns OAuth redirect JSON.
    get(
      url,
      {
        headers: {
          accept: 'text/html',
          'sec-fetch-mode': 'navigate',
          ...(cookie && { cookie }),
        },
        signal: AbortSignal.timeout(5000),
      },
      (response) => {
        response.resume()
        resolve(response.headers.location)
      },
    ).once('error', reject)
  })
}

export async function startIntegrationServer(
  databaseUrl: string,
  authSecret = randomBytes(32).toString('hex'),
  telegramWorkerSecret?: string,
) {
  if (process.env.TEST_RUST_WEB_BINARY) {
    const slot = await listenFetch(() => new Response(null, { status: 404 }), {
      hostname: '127.0.0.1',
      port: 0,
    })
    const port = new URL(slot.origin).port
    await slot.close()
    return startRustBackend(
      process.env.TEST_RUST_WEB_BINARY,
      databaseUrl,
      { publicOrigin: slot.origin, port },
      authSecret,
      slot.origin,
      telegramWorkerSecret,
    )
  }
  return startNodeIntegrationServer(databaseUrl, authSecret, telegramWorkerSecret)
}

export async function startNodeIntegrationServer(
  databaseUrl: string,
  authSecret = randomBytes(32).toString('hex'),
  telegramWorkerSecret?: string,
) {
  const child = fork(import.meta.filename, [], {
    execArgv: ['--import', 'tsx'],
    env: {
      ...process.env,
      DATABASE_URL: databaseUrl,
      BETTER_AUTH_SECRET: authSecret,
      ...(telegramWorkerSecret && { TELEGRAM_WORKER_SECRET: telegramWorkerSecret }),
    },
    stdio: ['ignore', 'ignore', 'inherit', 'ipc'],
  })
  const exited = new Promise<number | null>((resolve) =>
    child.once('close', (code) => resolve(code)),
  )
  async function close() {
    if (child.connected)
      child.send('close', (error) => {
        if (error) child.kill()
      })
    const kill = setTimeout(() => child.kill(), 3000)
    try {
      await exited
    } finally {
      clearTimeout(kill)
    }
  }

  const deadline = AbortSignal.timeout(10000)
  try {
    const [address] = await Promise.race([
      once(child, 'message', { signal: deadline }),
      exited.then((code) => {
        throw new Error(`Integration server exited before startup with code ${code}`)
      }),
    ])
    const origin = z
      .string()
      .regex(/^http:\/\/127\.0\.0\.1:\d+$/)
      .parse(address)
    const response = await fetch(`${origin}/health`, { signal: deadline })
    if (!response.ok) throw new Error(`Integration server health check returned ${response.status}`)
    return { origin, close }
  } catch (error) {
    await close()
    throw error
  }
}

if (import.meta.main) {
  const handler = Promise.withResolvers<typeof import('../server/index').handle>()
  const binding = await listenFetch(async (request) => (await handler.promise)(request), {
    hostname: '127.0.0.1',
    port: 0,
  })
  process.env.PORT = new URL(binding.origin).port
  process.env.BETTER_AUTH_URL = binding.origin
  const api = await import('../server/index')
  const workspace = api.workspaceServer(binding.server)
  handler.resolve(api.handle)
  process.on('message', (message) => {
    if (message === 'close')
      void workspace
        .close()
        .then(() => binding.close())
        .then(() => api.closeHttpResources())
        .then(
          () => process.exit(0),
          () => process.exit(1),
        )
  })
  process.send?.(binding.origin)
}
