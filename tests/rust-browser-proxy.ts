import { createServer, type ProxyOptions } from 'vite'

export async function startRustBrowserProxy() {
  let rustProxy: ProxyOptions | undefined
  let bunProxy: ProxyOptions | undefined
  const proxy = await createServer({
    configFile: false,
    server: {
      host: '127.0.0.1',
      port: 0,
      hmr: false,
      watch: null,
      proxy: {
        '^/(socket\\.io/|api/(me|notebook|student/attendance|attendance/sync))(?=\\?|$)': {
          target: 'http://127.0.0.1:1',
          ws: true,
          configure(_proxy, options) {
            rustProxy = options
          },
        },
        '/': {
          target: 'http://127.0.0.1:1',
          configure(_proxy, options) {
            bunProxy = options
          },
        },
      },
    },
  })
  try {
    await proxy.listen()
    const address = proxy.httpServer?.address()
    if (!address || typeof address === 'string' || !rustProxy || !bunProxy) {
      throw new Error('Local browser proxy could not bind')
    }
    const origin = 'http://127.0.0.1:' + address.port
    const rust = rustProxy
    const bun = bunProxy
    return {
      origin,
      connect(bunOrigin: string, rustOrigin: string) {
        rust.target = rustOrigin
        bun.target = bunOrigin
      },
      close: () => proxy.close(),
    }
  } catch (error) {
    await proxy.close()
    throw error
  }
}
