import { useEffect, useRef, useState } from 'react'
import type { DiagnosticsScreen } from '../shared/diagnostics'
import {
  replayLabels,
  replayPageSchema,
  replayPath,
  type ReplayFrame,
} from '../shared/sessionReplay'
import { request } from './neon'
import { AttendanceReplayView } from './AttendanceReplayView'
import { describeAttendanceReplayAction } from './attendanceReplayLabels'
import './SessionReplayPlayer.css'

function eventLabel(frame: ReplayFrame, ar: boolean) {
  if (frame.kind === 'view') return ar ? 'عرض الصفحة' : 'Viewed page'
  if (frame.attendance && frame.attendanceAction)
    return describeAttendanceReplayAction(frame.attendance, frame.attendanceAction, ar)
  const target = frame.boxes
    .filter(
      (box) =>
        box.label &&
        box.kind === 'control' &&
        frame.click.x >= box.x &&
        frame.click.x <= box.x + box.w &&
        frame.click.y >= box.y &&
        frame.click.y <= box.y + box.h,
    )
    .sort((a, b) => a.w * a.h - b.w * b.h)[0]
  const label = target?.label ? replayLabels[target.label][ar ? 1 : 0] : null
  return label
    ? ar
      ? `نقر على ${label}`
      : `Clicked ${label}`
    : ar
      ? 'نقر على الصفحة'
      : 'Clicked on page'
}

function elapsed(timeMs: number) {
  const seconds = Math.floor(timeMs / 1000)
  const tenths = Math.floor((timeMs % 1000) / 100)
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}.${tenths}`
}

export function SessionReplayPlayer({
  userId,
  actor,
  journey,
  ar,
  screenLabel,
}: {
  userId: string
  actor: string
  journey: string
  ar: boolean
  screenLabel: (screen: DiagnosticsScreen) => string
}) {
  const [frames, setFrames] = useState<ReplayFrame[]>([])
  const [nextAfter, setNextAfter] = useState<number | null>(null)
  const [index, setIndex] = useState(0)
  const [playing, setPlaying] = useState(false)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState(false)
  const activityRef = useRef<HTMLOListElement>(null)
  const frame = frames[index]
  const activePlayback = playing && index < frames.length - 1
  const previousClick = frames.findLastIndex(
    (entry, position) => position < index && entry.kind === 'click',
  )
  const nextClick = frames.findIndex(
    (entry, position) => position > index && entry.kind === 'click',
  )

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
              page.items.findIndex((item) => item.attendance),
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
    const timer = window.setTimeout(() => {
      const nextIndex = index + 1
      setIndex(nextIndex)
      if (nextIndex === frames.length - 1) setPlaying(false)
    }, delay)
    return () => window.clearTimeout(timer)
  }, [activePlayback, index, frames, frame])

  useEffect(() => {
    const activity = activityRef.current
    const selected = activity?.querySelector<HTMLButtonElement>('button[aria-current="true"]')
    if (!activity || !selected) return
    const top = selected.offsetTop
    const bottom = top + selected.offsetHeight
    if (top < activity.scrollTop) activity.scrollTop = top
    else if (bottom > activity.scrollTop + activity.clientHeight)
      activity.scrollTop = bottom - activity.clientHeight
  }, [index])

  return (
    <section className="session-replay" aria-label={ar ? 'إعادة تشغيل الجلسة' : 'Session replay'}>
      {frame && (
        <>
          <div className="session-replay-transport">
            <div className="session-replay-controls">
              <button
                className="secondary"
                type="button"
                disabled={index === 0}
                onClick={() => seek(index - 1)}
              >
                {ar ? 'السابق' : 'Previous'}
              </button>
              <button
                className="primary"
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
            <div className="session-replay-progress">
              <bdi dir="ltr">{elapsed(frame.timeMs)}</bdi>
              <input
                type="range"
                aria-label={ar ? 'موضع التشغيل' : 'Playback position'}
                min={0}
                max={frames.length - 1}
                value={index}
                disabled={frames.length < 2}
                onChange={(event) => seek(event.currentTarget.valueAsNumber)}
              />
              <bdi dir="ltr">{elapsed(frames[frames.length - 1].timeMs)}</bdi>
            </div>
          </div>
          <div className={`session-replay-content${frame.attendance ? ' with-attendance' : ''}`}>
            <div className="session-replay-activity">
              <div className="session-replay-activity-header">
                <h3>{ar ? 'النشاط' : 'Activity'}</h3>
                {!frame.attendance && (
                  <p className="muted">
                    {ar
                      ? 'سُجّل النشاط فقط لهذه الشاشة.'
                      : 'Only activity was recorded for this screen.'}
                  </p>
                )}
              </div>
              <ol ref={activityRef}>
                {frames.map((entry, position) => (
                  <li key={entry.step}>
                    <button
                      type="button"
                      aria-current={position === index ? 'true' : undefined}
                      onClick={() => seek(position)}
                    >
                      <span className="session-replay-dot" aria-hidden="true" />
                      <span className="session-replay-event">
                        <strong>{screenLabel(entry.screen)}</strong>
                        <span>{eventLabel(entry, ar)}</span>
                      </span>
                      <bdi dir="ltr">{elapsed(entry.timeMs)}</bdi>
                    </button>
                  </li>
                ))}
              </ol>
              {nextAfter !== null && (
                <button
                  className="secondary session-replay-more"
                  type="button"
                  disabled={loading}
                  onClick={() => void loadMore()}
                >
                  {ar ? 'تحميل المزيد' : 'Load more'}
                </button>
              )}
            </div>
            {frame.attendance && (
              <AttendanceReplayView
                snapshot={frame.attendance}
                action={frame.kind === 'click' ? frame.attendanceAction : undefined}
                language={frame.language}
              />
            )}
          </div>
        </>
      )}
      {loading && <p role="status">{ar ? 'جار التحميل…' : 'Loading…'}</p>}
      {error && <p role="alert">{ar ? 'تعذر تحميل التسجيل.' : 'Could not load replay.'}</p>}
      {!loading && !error && !frames.length && (
        <p>{ar ? 'لا يوجد تسجيل أو انتهت صلاحيته.' : 'No replay available, or it has expired.'}</p>
      )}
    </section>
  )
}
