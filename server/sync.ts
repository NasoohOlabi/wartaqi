import { setTimeout as sleep } from 'node:timers/promises'
import { Server as BunEngine } from '@socket.io/bun-engine'
import { Client } from 'pg'
import { Server } from 'socket.io'
import { z } from 'zod'

const socketPath = '/socket.io/'
const channel = 'wartaq_workspace_changed'
const handshakeSchema = z.strictObject({ userId: z.string().min(1).max(128) })

type WorkspaceEvents = { 'workspace:changed': (hint?: 'ledger') => void }

function listenForChanges(databaseUrl: string, invalidate: (hint?: 'ledger') => void) {
  const stop = new AbortController()
  let retryDelay = 250

  async function listen() {
    while (!stop.signal.aborted) {
      const client = new Client({
        connectionString: databaseUrl,
        application_name: 'wartaq-workspace-listener',
        connectionTimeoutMillis: 10000,
        query_timeout: 10000,
        keepAlive: true,
        keepAliveInitialDelayMillis: 10000,
      })
      const disconnected = Promise.withResolvers<void>()
      const disconnect = () => disconnected.resolve()
      stop.signal.addEventListener('abort', disconnect, { once: true })
      client.on('error', disconnect)
      client.once('end', disconnect)
      client.on('notification', (notification) => {
        if (notification.channel === channel)
          invalidate(notification.payload === 'ledger' ? 'ledger' : undefined)
      })
      try {
        await client.connect()
        if (stop.signal.aborted) return
        await client.query(`LISTEN ${channel}`)
        if (stop.signal.aborted) return
        retryDelay = 250
        // A commit might have landed while this connection was offline.
        invalidate()
        await disconnected.promise
      } catch {
        // HTTP remains available while the invalidation connection recovers.
      } finally {
        stop.signal.removeEventListener('abort', disconnect)
        await client.end().catch(() => undefined)
      }
      if (!stop.signal.aborted) {
        console.warn('Workspace notification listener disconnected')
        await sleep(retryDelay, undefined, { signal: stop.signal }).catch(() => undefined)
        retryDelay = Math.min(2 * retryDelay, 10000)
      }
    }
  }

  const listening = listen()
  return async () => {
    stop.abort()
    await listening
  }
}

export function createWorkspaceSync({
  authorize,
  databaseUrl,
  handle,
  origin,
}: {
  authorize: (headers: Headers, userId: string) => Promise<boolean>
  databaseUrl: string
  handle: (request: Request) => Promise<Response>
  origin: string
}) {
  const engine = new BunEngine({
    path: socketPath,
    maxHttpBufferSize: 4096,
    async allowRequest(request) {
      if (!request.headers.has('cookie')) throw new Error('Unauthorized')
    },
  })
  const io = new Server<Record<never, never>, WorkspaceEvents>({ serveClient: false })
  io.bind(engine)
  io.use(async (socket, next) => {
    try {
      const auth = handshakeSchema.safeParse(socket.handshake.auth)
      const { cookie } = socket.handshake.headers
      if (
        auth.success &&
        typeof cookie === 'string' &&
        (await authorize(new Headers({ cookie }), auth.data.userId))
      ) {
        next()
        return
      }
    } catch {
      // Return a generic failure without disclosing identity or grant details.
    }
    next(new Error('Unauthorized'))
  })
  const stopListening = listenForChanges(databaseUrl, (hint) =>
    hint ? io.emit('workspace:changed', hint) : io.emit('workspace:changed'),
  )
  const { fetch: socketFetch, websocket, idleTimeout } = engine.handler()

  return {
    serve: {
      websocket,
      idleTimeout,
      fetch(request: Request, server: Parameters<typeof socketFetch>[1]) {
        const path = new URL(request.url).pathname
        if (path !== socketPath) return handle(request)
        // A same-origin polling GET can omit Origin. Reject foreign Origin and
        // browser cross-site metadata on upgrades and existing polling SIDs too.
        const from = request.headers.get('origin')
        const site = request.headers.get('sec-fetch-site')
        if ((from && from !== origin) || (site && site !== 'same-origin'))
          return new Response('Forbidden', { status: 403 })
        return socketFetch(request, server)
      },
    },
    async close() {
      await Promise.all([stopListening(), io.close()])
    },
  }
}
