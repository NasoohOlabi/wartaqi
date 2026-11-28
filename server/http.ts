import { once } from 'node:events'
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http'
import { Readable } from 'node:stream'
import { pipeline } from 'node:stream/promises'

async function dispatch(
  incoming: IncomingMessage,
  outgoing: ServerResponse,
  handle: (request: Request) => Response | Promise<Response>,
) {
  const method = incoming.method ?? 'GET'
  const target = incoming.url ?? ''
  const host = incoming.headers.host
  if (!host || !target.startsWith('/')) {
    outgoing.writeHead(400).end()
    return
  }
  const headers = new Headers()
  for (let index = 0; index < incoming.rawHeaders.length; index += 2)
    headers.append(incoming.rawHeaders[index], incoming.rawHeaders[index + 1])
  const stop = new AbortController()
  outgoing.once('close', () => {
    if (!outgoing.writableFinished) stop.abort()
  })
  const hasBody =
    incoming.headers['transfer-encoding'] || Number(incoming.headers['content-length']) > 0
  const body =
    hasBody && method !== 'GET' && method !== 'HEAD' ? Readable.toWeb(incoming) : undefined
  if (body !== undefined && !(body instanceof ReadableStream))
    throw new Error('Invalid request stream')
  const init: RequestInit & { duplex?: 'half' } = {
    method,
    headers,
    signal: stop.signal,
    ...(body && { body, duplex: 'half' }),
  }
  const response = await handle(new Request('http://' + host + target, init))
  outgoing.statusCode = response.status
  for (const [name, value] of response.headers) {
    if (name !== 'set-cookie') outgoing.setHeader(name, value)
  }
  const cookies = response.headers.getSetCookie()
  if (cookies.length) outgoing.setHeader('Set-Cookie', cookies)
  if (response.body && method !== 'HEAD') {
    await pipeline(Readable.from(response.body, { objectMode: false }), outgoing, {
      signal: stop.signal,
    })
  } else {
    if (response.body) await response.body.cancel()
    outgoing.end()
  }
}

export async function listenFetch(
  handle: (request: Request) => Response | Promise<Response>,
  { port, hostname }: { port: number; hostname: string },
) {
  const server = createServer((incoming, outgoing) => {
    void dispatch(incoming, outgoing, handle).catch(() => {
      if (outgoing.destroyed) return
      if (outgoing.headersSent) outgoing.destroy()
      else outgoing.writeHead(500).end()
    })
  })
  server.listen(port, hostname)
  await once(server, 'listening')
  const address = server.address()
  if (!address || typeof address === 'string') throw new Error('Missing HTTP listener address')
  return {
    server,
    origin: 'http://' + hostname + ':' + address.port,
    async close() {
      if (!server.listening) return
      const closing = new Promise<void>((resolve, reject) =>
        server.close((cause) => (cause ? reject(cause) : resolve())),
      )
      server.closeAllConnections()
      await closing
    },
  }
}
