import type { Socket } from 'socket.io-client'

const coalesceMs = 40
const repairMs = 30_000
const connectedRepairMs = 120_000

function startStream(userId: string) {
  const listeners = new Set<() => void>()
  let socket: Socket | undefined
  let stopped = false
  let lastRefreshRequested = 0
  const notify = () => {
    lastRefreshRequested = performance.now()
    for (const listener of listeners) listener()
  }
  const foreground = () => {
    if (document.visibilityState !== 'visible') return
    if (socket && !socket.active) socket.connect()
    notify()
  }
  const online = () => {
    socket?.connect()
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
      socket = io({ path: '/socket.io', auth: { userId }, autoConnect: false })
      socket.on('connect', notify)
      socket.on('workspace:changed', notify)
      socket.connect()
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
      socket?.removeAllListeners()
      socket?.disconnect()
    },
  }
}

let active: ReturnType<typeof startStream> | undefined

export function subscribeWorkspaceChanges(
  userId: string,
  refresh: () => Promise<void> | void,
): () => void {
  if (active?.userId !== userId) {
    active?.stop()
    active = startStream(userId)
  }
  const stream = active
  let timer: number | undefined
  let running = false
  let trailing = false
  const schedule = () => {
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
