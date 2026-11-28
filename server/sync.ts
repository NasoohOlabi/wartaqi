import { setTimeout as sleep } from 'node:timers/promises'
import type { IncomingMessage, Server as HttpServer, ServerResponse } from 'node:http'
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
  origin,
  server,
}: {
  authorize: (headers: Headers, userId: string) => Promise<boolean>
  databaseUrl: string
  origin: string
  server: HttpServer
}) {
  const io = new Server<Record<never, never>, WorkspaceEvents>(server, {
    path: socketPath,
    serveClient: false,
    maxHttpBufferSize: 4096,
  })
  io.engine.use((request: IncomingMessage, response: ServerResponse, next: () => void) => {
    // Apply the origin check to the handshake and requests carrying an existing SID.
    const from = request.headers.origin
    const site = request.headers['sec-fetch-site']
    if ((from && from !== origin) || (site && site !== 'same-origin')) {
      response.writeHead(403).end('Forbidden')
      return
    }
    if (!request.headers.cookie) {
      response.writeHead(403, { 'content-type': 'application/json' }).end('{"code":4,"message":{}}')
      return
    }
    next()
  })
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
  return {
    async close() {
      await Promise.all([stopListening(), io.close()])
    },
  }
}
