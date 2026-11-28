import { createServer, type ProxyOptions } from 'vite'
import { startRustBackend } from './rust-backend-server'

export async function startRustBrowserProxy(
  binary: string,
  databaseUrl: string,
  authSecret: string,
) {
  let rustProxy: ProxyOptions | undefined
  const proxy = await createServer({
    configFile: false,
    server: {
      host: '127.0.0.1',
      port: 0,
      hmr: false,
      watch: null,
      proxy: {
        '/': {
          target: 'http://127.0.0.1:1',
          ws: true,
          configure(_proxy, options) {
            rustProxy = options
          },
        },
      },
    },
  })
  try {
    await proxy.listen()
    const address = proxy.httpServer?.address()
    if (!address || typeof address === 'string' || !rustProxy) {
      throw new Error('Local browser proxy could not bind')
    }
    const origin = 'http://127.0.0.1:' + address.port
    const rust = await startRustBackend(binary, databaseUrl, { publicOrigin: origin }, authSecret)
    rustProxy.target = rust.origin
    return {
      origin,
      async close() {
        await Promise.all([proxy.close(), rust.close()])
      },
    }
  } catch (error) {
    await proxy.close()
    throw error
  }
}
