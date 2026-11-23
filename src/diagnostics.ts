import { useEffect, useRef } from 'react'
import { onCLS, onINP, onLCP } from 'web-vitals'
import {
  diagnosticsActionSchema,
  diagnosticsBatchSchema,
  diagnosticsEventSchema,
} from '../shared/diagnostics'
import type {
  DiagnosticsAction,
  DiagnosticsEvent,
  DiagnosticsOutcome,
  DiagnosticsScreen,
} from '../shared/diagnostics'
import { accountFetch } from './accountRequest'
import { startSessionReplay } from './sessionReplay'

let actor: string | undefined
let journeyId = ''
let step = 0
let screen: DiagnosticsScreen | null = null
let recordedScreen: DiagnosticsScreen | null = null
let queue: DiagnosticsEvent[] = []
let sending = false
let offline = false
let capture = 0

type EventInput<T = DiagnosticsEvent> = T extends DiagnosticsEvent
  ? Omit<T, 'id' | 'journeyId' | 'step' | 'screen'>
  : never

function add(event: EventInput) {
  if (!actor || !screen) return
  const parsed = diagnosticsEventSchema.safeParse({
    ...event,
    id: crypto.randomUUID(),
    journeyId,
    step: ++step,
    screen,
  })
  if (parsed.success) {
    if (queue.length === 120) queue.shift()
    queue.push(parsed.data)
  }
}

async function flush(keepalive = false) {
  if (!actor || offline || sending || !queue.length) return
  const userId = actor
  const batch = queue.slice(0, 30)
  sending = true
  try {
    const response = await accountFetch(
      '/api/diagnostics',
      userId,
      diagnosticsBatchSchema.parse({ events: batch }),
      keepalive,
    )
    if ((response.ok || [400, 401, 403, 413].includes(response.status)) && actor === userId) {
      const ids = new Set(batch.map((event) => event.id))
      queue = queue.filter((event) => !ids.has(event.id))
    }
  } catch {
    // Diagnostics never affect application behavior.
  } finally {
    sending = false
  }
}

export function recordDiagnosticAction(
  userId: string,
  action: DiagnosticsAction,
  outcome: DiagnosticsOutcome,
): void {
  if (actor !== userId) return
  add({ kind: 'action', action, outcome })
}

export function flushDiagnostics(userId: string) {
  if (actor === userId) void flush(true)
}

export function useDiagnostics(
  userId: string,
  currentScreen: DiagnosticsScreen | null,
  isOffline: boolean,
): void {
  const replay = useRef<ReturnType<typeof startSessionReplay> | null>(null)
  useEffect(() => {
    const capturing = ++capture
    if (actor !== userId) {
      actor = userId
      journeyId = crypto.randomUUID()
      step = 0
      queue = []
      recordedScreen = null
    }
    replay.current = startSessionReplay(userId, journeyId)
    const error = () => add({ kind: 'error', code: 'runtime' })
    const rejection = () => add({ kind: 'error', code: 'promise' })
    const online = () => {
      offline = false
      void flush()
    }
    const hidden = () => {
      if (document.visibilityState === 'hidden') void flush(true)
    }
    const activation = (event: Event) => {
      if (!(event.target instanceof Element)) return
      const control = event.target.closest('[data-diagnostic-action]')
      if (!control || (event.type === 'click' && control.matches('input,select'))) return
      const action = diagnosticsActionSchema.safeParse(
        control.getAttribute('data-diagnostic-action'),
      )
      if (action.success) recordDiagnosticAction(userId, action.data, 'activated')
    }
    window.addEventListener('error', error)
    window.addEventListener('unhandledrejection', rejection)
    window.addEventListener('online', online)
    document.addEventListener('visibilitychange', hidden)
    document.addEventListener('click', activation, true)
    document.addEventListener('change', activation, true)
    const timer = window.setInterval(() => void flush(), 15000)
    return () => {
      replay.current?.stop()
      replay.current = null
      window.clearInterval(timer)
      window.removeEventListener('error', error)
      window.removeEventListener('unhandledrejection', rejection)
      window.removeEventListener('online', online)
      document.removeEventListener('visibilitychange', hidden)
      document.removeEventListener('click', activation, true)
      document.removeEventListener('change', activation, true)
      queueMicrotask(() => {
        if (capture !== capturing) return
        void flush(true)
        actor = undefined
        queue = []
        screen = null
      })
    }
  }, [userId])
  useEffect(() => {
    screen = currentScreen
    replay.current?.setScreen(currentScreen)
    if (currentScreen && recordedScreen !== currentScreen) add({ kind: 'screen' })
    recordedScreen = currentScreen
    if (!currentScreen) void flush()
  }, [userId, currentScreen])
  useEffect(() => {
    offline = isOffline
    if (!offline) void flush()
  }, [isOffline])
}

function vital(metric: 'LCP' | 'INP' | 'CLS', value: number) {
  add({ kind: 'vital', metric, value })
}
onLCP(({ value }) => vital('LCP', value))
onINP(({ value }) => vital('INP', value))
onCLS(({ value }) => vital('CLS', value))
