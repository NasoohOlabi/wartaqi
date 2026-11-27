import { closeHttpResources, handle, publicOrigin } from './index'

if (import.meta.main) {
  const publicUrl = new URL(publicOrigin)
  const server = Bun.serve({
    hostname: '127.0.0.1',
    port: 0,
    fetch(request) {
      if (request.headers.get('host')?.toLowerCase() !== publicUrl.host.toLowerCase())
        return new Response('Forbidden', { status: 403 })
      const incoming = new URL(request.url)
      const url = new URL(publicOrigin)
      url.pathname = incoming.pathname
      url.search = incoming.search
      return handle(new Request(url, request))
    },
  })
  console.log(`WARTAQ_COMPAT_READY=${server.port}`)
  let shuttingDown = false
  const shutdown = async () => {
    if (shuttingDown) return
    shuttingDown = true
    await server.stop(true)
    await closeHttpResources()
  }
  process.once('SIGINT', () => void shutdown())
  process.once('SIGTERM', () => void shutdown())
}
