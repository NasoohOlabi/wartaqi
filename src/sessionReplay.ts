import {
  replayLabels,
  replayLabelSchema,
  replayMaxBoxes,
  replayMaxDimension,
  replayMaxFrames,
  replayMaxBodyBytes,
  replayMaxFramesPerBatch,
  replayPath,
} from '../shared/sessionReplay'
import type { ReplayBox, ReplayFrame } from '../shared/sessionReplay'
import type { DiagnosticsScreen } from '../shared/diagnostics'
import { accountFetch } from './accountRequest'

const labels = new Map(
  Object.entries(replayLabels).flatMap(([key, values]) =>
    values.map((value) => [value, replayLabelSchema.parse(key)] as const),
  ),
)

function solidColor(value: string) {
  const channels = /^rgb\((\d+),\s*(\d+),\s*(\d+)\)$/.exec(value)
  if (channels)
    return (
      '#' +
      channels
        .slice(1)
        .map((channel) => Number(channel).toString(16).padStart(2, '0'))
        .join('')
    )
}

export function startSessionReplay(userId: string, journeyId: string) {
  let screen: DiagnosticsScreen | null = null
  let step = 0
  let queue: ReplayFrame[] = []
  let sending = false
  let stopped = false
  let retry = 0
  let timer = 0
  let trailing = 0
  let lastView = ''
  const started = performance.now()
  const enabled =
    import.meta.env.MODE !== 'verification' ||
    document.documentElement.dataset.sessionReplay === 'test'
  const limit = (n: number) => Math.max(0, Math.min(replayMaxDimension, Math.round(n)))
  const label = (element: Element) => {
    const text = (
      element.getAttribute('aria-label') ||
      element.getAttribute('title') ||
      element.textContent ||
      ''
    ).trim()
    return labels.get(text)
  }
  function boxes(width: number, height: number): ReplayBox[] {
    const result: ReplayBox[] = []
    const elements = document.querySelectorAll(
      'main, header, nav, aside, section, article, button, a, input, select, textarea, summary, img, h1, h2, h3, p, td, th',
    )
    for (const element of elements) {
      if (result.length >= replayMaxBoxes) break
      const rect = element.getBoundingClientRect()
      if (rect.right <= 0 || rect.bottom <= 0 || rect.left >= width || rect.top >= height) continue
      const style = getComputedStyle(element)
      if (style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0')
        continue
      const x = limit(Math.max(0, rect.left)),
        y = limit(Math.max(0, rect.top))
      const w = limit(Math.min(width, rect.right)) - x,
        h = limit(Math.min(height, rect.bottom)) - y
      if (!w || !h) continue
      const kind = element.matches('input,select,textarea')
        ? 'field'
        : element.matches('button,a,summary')
          ? 'control'
          : element.matches('img')
            ? 'image'
            : element.matches('h1,h2,h3,p,td,th')
              ? 'line'
              : 'surface'
      const fill = solidColor(style.backgroundColor)
      if (kind === 'surface' && !fill) continue
      const safeLabel = kind === 'control' || kind === 'line' ? label(element) : undefined
      result.push({
        x,
        y,
        w,
        h,
        kind,
        ...(fill && { fill }),
        ...(safeLabel && { label: safeLabel }),
      })
    }
    return result
  }
  async function flush(keepalive = false) {
    if (!enabled || sending || !queue.length) return
    const frames = queue.slice(0, replayMaxFramesPerBatch)
    while (
      frames.length > 1 &&
      new TextEncoder().encode(JSON.stringify({ journeyId, frames })).length > replayMaxBodyBytes
    )
      frames.pop()
    sending = true
    try {
      const response = await accountFetch(replayPath, userId, { journeyId, frames }, keepalive)
      if (response.ok || [400, 401, 403, 413].includes(response.status)) {
        const sent = new Set(frames.map((frame) => frame.step))
        queue = queue.filter((frame) => !sent.has(frame.step))
        retry = 0
      } else retry = Math.min(retry + 1, 6)
    } catch {
      retry = Math.min(retry + 1, 6)
    } finally {
      sending = false
      if (!stopped && queue.length)
        timer = window.setTimeout(
          () => {
            timer = 0
            void flush()
          },
          retry ? 1000 * 2 ** retry : 300,
        )
    }
  }
  function capture(click?: { x: number; y: number }) {
    if (!enabled || !screen || stopped || step >= replayMaxFrames) return
    const width = Math.max(1, limit(window.innerWidth)),
      height = Math.max(1, limit(window.innerHeight))
    const view = {
      screen,
      width,
      height,
      theme: document.documentElement.dataset.theme === 'dark' ? 'dark' : 'light',
      language: document.documentElement.lang.startsWith('ar') ? 'ar' : 'en',
      boxes: boxes(width, height),
    } satisfies Pick<ReplayFrame, 'screen' | 'width' | 'height' | 'theme' | 'language' | 'boxes'>
    if (!click) {
      const serialized = JSON.stringify(view)
      if (serialized === lastView) return
      lastView = serialized
    }
    const base = {
      ...view,
      step: ++step,
      timeMs: Math.min(Math.round(performance.now() - started), 86_400_000),
    }
    queue.push(click ? { ...base, kind: 'click', click } : { ...base, kind: 'view' })
    if (queue.length > 60) queue.shift()
    if (!sending && !timer) void flush()
  }
  function schedule() {
    window.clearTimeout(trailing)
    trailing = window.setTimeout(() => requestAnimationFrame(() => capture()), 150)
  }
  function clicked(event: MouseEvent) {
    if (!screen || !event.isTrusted) return
    const rect = event.target instanceof Element ? event.target.getBoundingClientRect() : null
    capture({
      x: limit(
        Math.min(
          window.innerWidth,
          event.detail === 0 && rect ? rect.left + rect.width / 2 : event.clientX,
        ),
      ),
      y: limit(
        Math.min(
          window.innerHeight,
          event.detail === 0 && rect ? rect.top + rect.height / 2 : event.clientY,
        ),
      ),
    })
    schedule()
  }
  const hidden = () => {
    if (document.visibilityState === 'hidden') void flush(true)
  }
  const changes = new MutationObserver(() => {
    if (screen) schedule()
  })
  if (enabled) {
    changes.observe(document.documentElement, {
      subtree: true,
      childList: true,
      characterData: true,
      attributes: true,
      attributeFilter: ['data-theme', 'hidden', 'open', 'aria-expanded', 'aria-selected'],
    })
    document.addEventListener('click', clicked, true)
    document.addEventListener('visibilitychange', hidden)
    window.addEventListener('scroll', schedule, true)
    window.addEventListener('resize', schedule)
  }
  return {
    setScreen(value: DiagnosticsScreen | null) {
      screen = value
      window.clearTimeout(trailing)
      if (value && enabled) schedule()
    },
    stop() {
      stopped = true
      window.clearTimeout(timer)
      window.clearTimeout(trailing)
      changes.disconnect()
      document.removeEventListener('click', clicked, true)
      document.removeEventListener('visibilitychange', hidden)
      window.removeEventListener('scroll', schedule, true)
      window.removeEventListener('resize', schedule)
      void flush(true)
    },
  }
}
