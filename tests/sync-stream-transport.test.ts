import { expect, test } from 'vitest-runner'
import { createServer } from 'node:http'
import { setTimeout as sleep } from 'node:timers/promises'
import { Server } from 'socket.io'
import { subscribeWorkspaceChanges } from '../src/syncStream'

async function until(check: () => boolean, timeout = 6000) {
  const deadline = Date.now() + timeout
  while (!check()) {
    if (Date.now() > deadline) throw new Error('Timed out waiting for sync')
    await sleep(20)
  }
}

test('workspace transport delivers, reconnects, falls back from a blackhole, and respects denial', async () => {
  const http = createServer()
  const server = new Server(http)
  let websocketMode: 'healthy' | 'blackhole' | 'reject' = 'healthy'
  let upgrades = 0
  let polling = 0
  let connections = 0
  const upgradeHandlers = http.listeners('upgrade')
  http.removeAllListeners('upgrade')
  http.on('upgrade', (request, socket, head) => {
    upgrades++
    if (websocketMode === 'blackhole') return
    if (websocketMode === 'reject') return socket.destroy()
    for (const handler of upgradeHandlers) handler.call(http, request, socket, head)
  })
  const requestHandlers = http.listeners('request')
  http.removeAllListeners('request')
  http.on('request', (request, response) => {
    if (request.url?.includes('transport=polling')) polling++
    for (const handler of requestHandlers) handler.call(http, request, response)
  })
  server.use((socket, next) =>
    next(socket.handshake.auth.userId === 'denied' ? new Error('Unauthorized') : undefined),
  )
  server.on('connection', () => connections++)
  await new Promise<void>((resolve) => http.listen(0, '127.0.0.1', resolve))
  const address = http.address()
  if (!address || typeof address === 'string') throw new Error('Missing server address')
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, 'window')
  const previousDocument = Object.getOwnPropertyDescriptor(globalThis, 'document')
  const previousLocation = Object.getOwnPropertyDescriptor(globalThis, 'location')
  const browser = new EventTarget()
  Object.assign(browser, { setTimeout, clearTimeout, setInterval, clearInterval })
  const document = Object.assign(new EventTarget(), { visibilityState: 'visible' })
  Object.defineProperty(globalThis, 'window', { configurable: true, value: browser })
  Object.defineProperty(globalThis, 'document', { configurable: true, value: document })
  Object.defineProperty(globalThis, 'location', {
    configurable: true,
    value: new URL(`http://127.0.0.1:${address.port}`),
  })
  let unsubscribe: (() => void) | undefined
  try {
    let refreshes = 0
    unsubscribe = subscribeWorkspaceChanges('allowed', () => {
      refreshes++
    })
    await until(() => connections === 1 && refreshes >= 1)
    expect(upgrades).toBe(1)
    expect(polling).toBe(0)
    const before = refreshes
    server.emit('workspace:changed')
    await until(() => refreshes > before)
    const beforeReconnect = refreshes
    for (const client of server.sockets.sockets.values()) client.conn.close()
    await until(() => connections === 2 && refreshes > beforeReconnect)
    unsubscribe()
    unsubscribe = undefined

    websocketMode = 'blackhole'
    const beforePolling = polling
    const start = performance.now()
    unsubscribe = subscribeWorkspaceChanges('allowed', () => {
      refreshes++
    })
    await until(() => connections === 3)
    expect(performance.now() - start).toBeGreaterThanOrEqual(1900)
    expect(performance.now() - start).toBeLessThan(4000)
    expect(polling).toBeGreaterThan(beforePolling)
    const beforeHint = refreshes
    server.emit('workspace:changed')
    await until(() => refreshes > beforeHint)
    unsubscribe()
    unsubscribe = undefined

    websocketMode = 'reject'
    const rejectedStart = performance.now()
    unsubscribe = subscribeWorkspaceChanges('allowed', () => {
      refreshes++
    })
    await until(() => connections === 4)
    expect(performance.now() - rejectedStart).toBeLessThan(2000)
    unsubscribe()
    unsubscribe = undefined

    websocketMode = 'healthy'
    unsubscribe = subscribeWorkspaceChanges('allowed', () => {
      refreshes++
    })
    await until(() => connections === 5)
    websocketMode = 'blackhole'
    const reconnectStart = performance.now()
    const refreshesBeforeFallback = refreshes
    for (const client of server.sockets.sockets.values()) client.conn.close()
    await until(() => connections === 6 && refreshes > refreshesBeforeFallback)
    expect(performance.now() - reconnectStart).toBeLessThan(4500)
    unsubscribe()
    unsubscribe = undefined

    websocketMode = 'healthy'
    await sleep(300)
    const deniedPolling = polling
    const deniedUpgrades = upgrades
    unsubscribe = subscribeWorkspaceChanges('denied', () => {
      refreshes++
    })
    await until(() => upgrades > deniedUpgrades)
    await sleep(2300)
    expect(polling).toBe(deniedPolling)
    expect(connections).toBe(6)
  } finally {
    unsubscribe?.()
    server.close()
    http.closeAllConnections()
    for (const [key, descriptor] of [
      ['window', previousWindow],
      ['document', previousDocument],
      ['location', previousLocation],
    ] as const) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor)
      else Reflect.deleteProperty(globalThis, key)
    }
  }
}, 20000)
