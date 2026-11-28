import type { Socket } from 'socket.io-client'

const coalesceMs = 40
const repairMs = 30_000
const connectedRepairMs = 120_000
const websocketWaitMs = 2_000

function startStream(userId: string) {
  const listeners = new Set<(hint?: unknown) => void>()
  let socket: Socket | undefined
  let watchdog: number | undefined
  let connect = () => socket?.connect()
  let stopped = false
  let lastRefreshRequested = 0
  const notify = (hint?: unknown) => {
    if (hint !== 'ledger') lastRefreshRequested = performance.now()
    for (const listener of listeners) listener(hint)
  }
  const foreground = () => {
    if (document.visibilityState !== 'visible') return
    if (socket && !socket.active) connect()
    notify()
  }
  const online = () => {
    connect()
    notify()
  }
  window.addEventListener('focus', foreground)
  window.addEventListener('online', online)
  document.addEventListener('visibilitychange', foreground)
  const timer = window.setInterval(() => {
    if (socket?.connected && performance.now() - lastRefreshRequested < connectedRepairMs) return
    foreground()
  }, repairMs)
  void import('socket.io-client')
    .then(({ io }) => {
      if (stopped) return
      const start = (pollingFirst: boolean) => {
        const client = io({
          path: '/socket.io',
          auth: { userId },
          autoConnect: false,
          forceNew: true,
          transports: pollingFirst ? ['polling', 'websocket'] : ['websocket', 'polling'],
          tryAllTransports: true,
        })
        socket = client
        const clearWatchdog = () => window.clearTimeout(watchdog)
        const armWatchdog = () => {
          if (pollingFirst) return
          clearWatchdog()
          watchdog = window.setTimeout(() => {
            client.removeAllListeners()
            client.io.removeAllListeners()
            client.disconnect()
            start(true)
          }, websocketWaitMs)
        }
        client.io.on('open', clearWatchdog)
        client.io.on('error', clearWatchdog)
        client.io.on('reconnect_attempt', armWatchdog)
        client.on('connect', notify)
        client.on('workspace:changed', notify)
        connect = () => {
          if (!client.active) armWatchdog()
          client.connect()
        }
        connect()
      }
      start(false)
    })
    .catch(() => {
      // Foreground and periodic repair also work without the socket transport.
    })
  return {
    userId,
    listeners,
    stop() {
      if (stopped) return
      stopped = true
      listeners.clear()
      window.removeEventListener('focus', foreground)
      window.removeEventListener('online', online)
      document.removeEventListener('visibilitychange', foreground)
      window.clearInterval(timer)
      window.clearTimeout(watchdog)
      socket?.removeAllListeners()
      socket?.disconnect()
    },
  }
}

let active: ReturnType<typeof startStream> | undefined

export function subscribeWorkspaceChanges(
  userId: string,
  refresh: () => Promise<void> | void,
  options?: { skipLedger?: boolean },
): () => void {
  if (active?.userId !== userId) {
    active?.stop()
    active = startStream(userId)
  }
  const stream = active
  let timer: number | undefined
  let running = false
  let trailing = false
  const schedule = (hint?: unknown) => {
    if (hint === 'ledger' && options?.skipLedger) return
    if (!stream.listeners.has(schedule)) return
    if (running) {
      trailing = true
      return
    }
    if (timer !== undefined) return
    timer = window.setTimeout(async () => {
      timer = undefined
      if (!stream.listeners.has(schedule)) return
      running = true
      try {
        await refresh()
      } catch {
        // Consumers own read errors and may keep rendering their last authorized result.
      } finally {
        running = false
        if (trailing) {
          trailing = false
          schedule()
        }
      }
    }, coalesceMs)
  }
  stream.listeners.add(schedule)
  schedule()
  return () => {
    stream.listeners.delete(schedule)
    window.clearTimeout(timer)
    if (stream.listeners.size === 0 && active === stream) {
      stream.stop()
      active = undefined
    }
  }
}
