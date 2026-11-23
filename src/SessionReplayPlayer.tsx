import { useEffect, useRef, useState } from 'react'
import {
  replayLabels,
  replayPageSchema,
  replayPath,
  type ReplayFrame,
} from '../shared/sessionReplay'
import { request } from './neon'
import { AttendanceReplayView } from './AttendanceReplayView'

export function SessionReplayPlayer({
  userId,
  actor,
  journey,
  ar,
}: {
  userId: string
  actor: string
  journey: string
  ar: boolean
}) {
  const [frames, setFrames] = useState<ReplayFrame[]>([])
  const [nextAfter, setNextAfter] = useState<number | null>(null)
  const [index, setIndex] = useState(0)
  const [playing, setPlaying] = useState(false)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState(false)
  const canvas = useRef<HTMLCanvasElement>(null)
  const frame = frames[index]
  const activePlayback = playing && index < frames.length - 1
  const previousClick = frames.findLastIndex(
    (entry, position) => position < index && entry.kind === 'click',
  )
  const nextClick = frames.findIndex(
    (entry, position) => position > index && entry.kind === 'click',
  )
  const scale = frame ? Math.min(1, 1600 / frame.width, 1200 / frame.height) : 1

  function seek(position: number) {
    setPlaying(false)
    setIndex(position)
  }

  useEffect(() => {
    let active = true
    const params = new URLSearchParams({ actor, journey })
    request(replayPath + '?' + params, userId)
      .then((data) => {
        const page = replayPageSchema.parse(data)
        if (active) {
          setFrames(page.items)
          setNextAfter(page.nextAfter)
          setIndex(
            Math.max(
              0,
              page.items.findIndex((frame) => frame.attendance),
            ),
          )
        }
      })
      .catch(() => {
        if (active) setError(true)
      })
      .finally(() => {
        if (active) setLoading(false)
      })
    return () => {
      active = false
    }
  }, [actor, journey, userId])

  async function loadMore() {
    if (nextAfter === null || loading) return
    setLoading(true)
    setError(false)
    try {
      const params = new URLSearchParams({ actor, journey, after: String(nextAfter) })
      const page = replayPageSchema.parse(await request(replayPath + '?' + params, userId))
      setFrames((previous) => [...previous, ...page.items])
      setNextAfter(page.nextAfter)
    } catch {
      setError(true)
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    if (!activePlayback) return
    const delay = Math.max(
      200,
      Math.min(3000, (frames[index + 1]?.timeMs ?? 0) - (frame?.timeMs ?? 0)),
    )
    const timer = window.setTimeout(() => setIndex((value) => value + 1), delay)
    return () => window.clearTimeout(timer)
  }, [activePlayback, index, frames, frame])

  useEffect(() => {
    const viewport = canvas.current
    const context = viewport?.getContext('2d')
    if (!context || !frame || !viewport) return
    const dark = frame.theme === 'dark'
    context.setTransform(viewport.width / frame.width, 0, 0, viewport.height / frame.height, 0, 0)
    context.fillStyle = dark ? '#171717' : '#ffffff'
    context.fillRect(0, 0, frame.width, frame.height)
    for (const box of frame.boxes) {
      if (box.fill || box.kind === 'image' || box.kind === 'field' || box.kind === 'control') {
        context.fillStyle = box.fill ?? (dark ? '#323d48' : '#e2edf0')
        context.fillRect(box.x, box.y, box.w, box.h)
      }
      if (box.label) {
        context.fillStyle = dark ? '#ffffff' : '#111111'
        context.font = `${Math.max(10, Math.min(18, box.h * 0.6))}px sans-serif`
        context.direction = frame.language === 'ar' ? 'rtl' : 'ltr'
        context.textAlign = frame.language === 'ar' ? 'right' : 'left'
        context.fillText(
          replayLabels[box.label][frame.language === 'ar' ? 1 : 0],
          frame.language === 'ar' ? box.x + box.w - 4 : box.x + 4,
          box.y + box.h * 0.7,
          Math.max(1, box.w - 8),
        )
      } else if (box.kind === 'line' || box.kind === 'text') {
        const width = Math.min(128, box.w * 0.7)
        const x = frame.language === 'ar' ? box.x + box.w - width : box.x
        const height = Math.min(8, box.h * 0.5)
        context.fillStyle = dark ? '#64748b' : '#c0d0d4'
        context.fillRect(x, box.y + (box.h - height) / 2, width, height)
      }
    }
    if (frame.kind === 'click') {
      context.strokeStyle = '#ef4444'
      context.lineWidth = 4
      context.beginPath()
      context.arc(frame.click.x, frame.click.y, 12, 0, Math.PI * 2)
      context.stroke()
    }
  }, [frame])

  return (
    <section className="space-y-3" aria-label={ar ? 'إعادة تشغيل الجلسة' : 'Session replay'}>
      {frame && (
        <>
          {!frame.attendance && (
            <p className="muted">
              {ar ? 'الشاشة' : 'Screen'}: {frame.screen} ·{' '}
              {frame.kind === 'click'
                ? `${ar ? 'نقرة' : 'Click'} (${frame.click.x}, ${frame.click.y})`
                : ar
                  ? 'عرض'
                  : 'View'}
            </p>
          )}
          <p>
            {index + 1} / {frames.length} · {(frame.timeMs / 1000).toFixed(1)}s
          </p>
          <input
            type="range"
            className="w-full"
            aria-label={ar ? 'موضع التشغيل' : 'Playback position'}
            min={0}
            max={frames.length - 1}
            value={index}
            disabled={frames.length < 2}
            onChange={(event) => seek(event.currentTarget.valueAsNumber)}
          />
          <div className="flex flex-wrap gap-2">
            <button
              className="secondary"
              type="button"
              disabled={index === 0}
              onClick={() => seek(index - 1)}
            >
              {ar ? 'السابق' : 'Previous'}
            </button>
            <button
              className="secondary"
              type="button"
              onClick={() => {
                if (index === frames.length - 1) setIndex(0)
                setPlaying(!activePlayback)
              }}
              disabled={frames.length < 2}
            >
              {activePlayback ? (ar ? 'إيقاف مؤقت' : 'Pause') : ar ? 'تشغيل' : 'Play'}
            </button>
            <button
              className="secondary"
              type="button"
              disabled={index === frames.length - 1}
              onClick={() => seek(index + 1)}
            >
              {ar ? 'التالي' : 'Next'}
            </button>
            <button
              type="button"
              className="secondary"
              disabled={previousClick < 0}
              onClick={() => seek(previousClick)}
            >
              {ar ? 'النقرة السابقة' : 'Previous click'}
            </button>
            <button
              type="button"
              className="secondary"
              disabled={nextClick < 0}
              onClick={() => seek(nextClick)}
            >
              {ar ? 'النقرة التالية' : 'Next click'}
            </button>
          </div>
          {frame.attendance ? (
            <AttendanceReplayView
              snapshot={frame.attendance}
              action={frame.kind === 'click' ? frame.attendanceAction : undefined}
              language={frame.language}
            />
          ) : (
            <canvas
              ref={canvas}
              width={Math.max(1, Math.round(frame.width * scale))}
              height={Math.max(1, Math.round(frame.height * scale))}
              className="block h-auto max-w-full border"
              aria-hidden="true"
            />
          )}
        </>
      )}
      {nextAfter !== null && (
        <button
          className="secondary"
          type="button"
          disabled={loading}
          onClick={() => void loadMore()}
        >
          {ar ? 'تحميل المزيد' : 'Load more'}
        </button>
      )}
      {loading && <p role="status">{ar ? 'جار التحميل…' : 'Loading…'}</p>}
      {error && <p role="alert">{ar ? 'تعذر تحميل التسجيل.' : 'Could not load replay.'}</p>}
      {!loading && !error && !frames.length && (
        <p>{ar ? 'لا يوجد تسجيل أو انتهت صلاحيته.' : 'No replay available, or it has expired.'}</p>
      )}
    </section>
  )
}
