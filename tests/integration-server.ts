import { randomBytes } from 'node:crypto'
import type { WebSocketData } from '@socket.io/bun-engine'
import { z } from 'zod'

export async function startIntegrationServer(
  databaseUrl: string,
  authSecret = randomBytes(32).toString('hex'),
) {
  const ready = Promise.withResolvers<unknown>()
  const child = Bun.spawn([process.execPath, 'run', import.meta.path], {
    env: {
      ...process.env,
      DATABASE_URL: databaseUrl,
      BETTER_AUTH_SECRET: authSecret,
    },
    stdout: 'ignore',
    stderr: 'inherit',
    ipc: ready.resolve,
  })
  const deadline = AbortSignal.timeout(10000)
  deadline.addEventListener('abort', () => ready.reject(deadline.reason), { once: true })

  try {
    const address = await Promise.race([
      ready.promise,
      child.exited.then((code) => {
        throw new Error(`Integration server exited before startup with code ${code}`)
      }),
    ])
    const origin = z
      .string()
      .regex(/^http:\/\/127\.0\.0\.1:\d+$/)
      .parse(address)
    const response = await fetch(`${origin}/health`, { signal: deadline })
    if (!response.ok) throw new Error(`Integration server health check returned ${response.status}`)
    return { origin, child }
  } catch (error) {
    child.kill()
    throw error
  }
}

if (import.meta.main) {
  type Workspace = ReturnType<typeof import('../server/index').workspaceServer>
  const handler = Promise.withResolvers<Workspace>()
  let workspace: Workspace | undefined
  const server = Bun.serve<WebSocketData>({
    hostname: '127.0.0.1',
    port: 0,
    idleTimeout: 50,
    fetch: async (request, instance) => (await handler.promise).serve.fetch(request, instance),
    websocket: {
      open: (socket) => workspace?.serve.websocket.open(socket),
      message: (socket, message) => workspace?.serve.websocket.message(socket, message),
      close: (socket, code, reason) => workspace?.serve.websocket.close(socket, code, reason),
    },
  })
  process.env.PORT = String(server.port)
  process.env.BETTER_AUTH_URL = process.env.TEST_WORKSPACE_ORIGIN ?? server.url.origin
  workspace = (await import('../server/index')).workspaceServer()
  handler.resolve(workspace)
  process.on('message', (message) => {
    if (message === 'close') {
      void Promise.all([workspace?.close(), server.stop(true)]).finally(() => process.exit())
    }
  })
  process.send?.(server.url.origin)
}
