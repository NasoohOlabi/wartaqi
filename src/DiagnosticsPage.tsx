import { useEffect, useEffectEvent, useRef, useState } from 'react'
import type { z } from 'zod'
import {
  diagnosticsJourneysSchema,
  diagnosticsTimelineSchema,
  type DiagnosticsAction,
  type DiagnosticsOutcome,
  type DiagnosticsRole,
  type DiagnosticsScreen,
} from '../shared/diagnostics'
import { request } from './neon'
import type { Profile } from './ProfileMenu'
import { SessionReplayPlayer } from './SessionReplayPlayer'
import { SettingsNavigation } from './SettingsNavigation'
import { useLanguage } from './useLanguage'
import { WorkspaceNavigation } from './WorkspaceNavigation'

type Journeys = z.infer<typeof diagnosticsJourneysSchema>
type Timeline = z.infer<typeof diagnosticsTimelineSchema>
type Journey = Journeys['items'][number]
type Labels<T extends string> = Record<T, [english: string, arabic: string]>

const roles: Labels<DiagnosticsRole> = {
  administrator: ['Administrator', 'مسؤول'],
  teacher: ['Teacher', 'معلم'],
  student: ['Student', 'طالب'],
}
const screens: Labels<DiagnosticsScreen> = {
  dashboard: ['Dashboard', 'الرئيسية'],
  attendance: ['Attendance', 'الحضور'],
  roster: ['Student roster', 'قائمة الطلاب'],
  summary: ['Attendance summary', 'ملخص الحضور'],
  student: ['Student profile', 'ملف الطالب'],
  'student-portal': ['Student portal', 'بوابة الطالب'],
  courses: ['Courses', 'الدورات'],
  'course-create': ['Create course', 'إضافة دورة'],
  'course-edit': ['Edit course', 'تعديل الدورة'],
  'course-students': ['Course membership', 'طلاب الدورة'],
  'course-grades': ['Course grades', 'درجات الدورة'],
  'settings-accounts': ['Account settings', 'إعدادات الحسابات'],
  'settings-students': ['Student settings', 'إعدادات الطلاب'],
  'add-student': ['Add student', 'إضافة طالب'],
  profile: ['Profile settings', 'إعدادات الملف الشخصي'],
  history: ['History and corrections', 'السجل والتصحيحات'],
  changes: ['Pending changes', 'التغييرات المعلقة'],
  invitation: ['Invitation', 'الدعوة'],
}
const actions: Labels<DiagnosticsAction> = {
  attendance: ['Attendance', 'الحضور'],
  note: ['Note', 'ملاحظة'],
  correction: ['Correction', 'تصحيح'],
  student: ['Student record', 'سجل الطالب'],
  account: ['Account', 'الحساب'],
  course: ['Course', 'الدورة'],
  enrollment: ['Enrollment', 'التسجيل'],
  grade: ['Grade', 'الدرجة'],
  profile: ['Profile', 'الملف الشخصي'],
  sync: ['Sync', 'المزامنة'],
  filter: ['Filter', 'التصفية'],
  export: ['Export', 'التصدير'],
  'course-switch': ['Switch course', 'تغيير الدورة'],
  'sign-out': ['Sign out', 'تسجيل الخروج'],
  'account-switch': ['Switch account', 'تبديل الحساب'],
}
const outcomes: Labels<DiagnosticsOutcome> = {
  activated: ['Opened', 'فُتح'],
  saved: ['Saved', 'حُفظ'],
  queued: ['Queued', 'بانتظار المزامنة'],
  failed: ['Failed', 'تعذّر'],
  synced: ['Synced', 'تمت المزامنة'],
}

export function DiagnosticsPage({
  userId,
  profile,
  signOut,
}: {
  userId: string
  profile: Profile
  signOut: () => void
}) {
  const { language } = useLanguage()
  const ar = language === 'ar'
  const locale = ar ? 1 : 0
  const [role, setRole] = useState('')
  const [actor, setActor] = useState<Journey | null>(null)
  const [showReplay, setShowReplay] = useState(false)
  const [selected, setSelected] = useState<Journey | null>(null)
  const [journeys, setJourneys] = useState<Journeys | null>(null)
  const [timeline, setTimeline] = useState<Timeline | null>(null)
  const [error, setError] = useState(false)
  const [loading, setLoading] = useState(true)
  const latestRead = useRef(0)
  const timelineHeading = useRef<HTMLHeadingElement>(null)
  const journeyTrigger = useRef<string | null>(null)
  const listSection = useRef<HTMLElement>(null)

  async function read<T>(get: () => Promise<T>, apply: (data: T) => void) {
    const readId = ++latestRead.current
    setLoading(true)
    setError(false)
    try {
      const data = await get()
      if (latestRead.current === readId) apply(data)
    } catch {
      if (latestRead.current === readId) setError(true)
    } finally {
      if (latestRead.current === readId) setLoading(false)
    }
  }

  function loadList(filterRole: string, filterActor: Journey | null, before?: string) {
    if (!before) setJourneys(null)
    const params = new URLSearchParams()
    if (filterRole) params.set('role', filterRole)
    if (filterActor) params.set('actor', filterActor.actorId)
    if (before) params.set('before', before)
    return read(
      async () =>
        diagnosticsJourneysSchema.parse(
          await request('/api/diagnostics/journeys?' + params, userId),
        ),
      (data) =>
        setJourneys((previous) => ({
          items: before && previous ? [...previous.items, ...data.items] : data.items,
          nextCursor: data.nextCursor,
        })),
    )
  }

  function loadTimeline(item: Journey, before?: string) {
    if (!before) setTimeline(null)
    const params = new URLSearchParams({ journey: item.journeyId, actor: item.actorId })
    if (before) params.set('before', before)
    return read(
      async () =>
        diagnosticsTimelineSchema.parse(await request('/api/diagnostics/events?' + params, userId)),
      (data) =>
        setTimeline((previous) => ({
          items: before && previous ? [...previous.items, ...data.items] : data.items,
          nextCursor: data.nextCursor,
        })),
    )
  }

  const loadInitial = useEffectEvent(() => void loadList('', null))
  useEffect(() => {
    let active = true
    queueMicrotask(() => {
      if (active) loadInitial()
    })
    return () => {
      active = false
    }
  }, [])
  useEffect(() => {
    if (selected) timelineHeading.current?.focus()
  }, [selected])

  return (
    <div className="workspace">
      <WorkspaceNavigation administrator profile={profile} leave={signOut} />
      <main id="main-content" className="workspace-main role-page" tabIndex={-1}>
        <header className="role-heading">
          <h1>{ar ? 'التشخيص' : 'Diagnostics'}</h1>
        </header>
        <SettingsNavigation current="diagnostics" />
        {selected ? (
          <section className="panel space-y-4" aria-busy={loading}>
            <button
              type="button"
              className="secondary"
              onClick={() => {
                latestRead.current++
                setShowReplay(false)
                setSelected(null)
                setLoading(false)
                setError(false)
                requestAnimationFrame(() => {
                  const button =
                    journeyTrigger.current && document.getElementById(journeyTrigger.current)
                  if (button) button.focus()
                  else listSection.current?.focus()
                })
              }}
            >
              {ar ? 'العودة إلى القائمة' : 'Back to list'}
            </button>
            <h2 className="title break-words" tabIndex={-1} ref={timelineHeading} dir="auto">
              {selected.actorName || selected.actorId}
            </h2>
            <p className="muted">{roles[selected.role][locale]}</p>
            <button
              type="button"
              className="secondary"
              aria-expanded={showReplay}
              onClick={() => setShowReplay((value) => !value)}
            >
              {ar ? 'إعادة تشغيل الجلسة' : 'Replay session'}
            </button>
            {showReplay && (
              <SessionReplayPlayer
                userId={userId}
                actor={selected.actorId}
                journey={selected.journeyId}
                ar={ar}
                screenLabel={(screen) => screens[screen][locale]}
              />
            )}
            {timeline?.items.length === 0 && (
              <p>{ar ? 'لا توجد أحداث مسجّلة.' : 'No recorded events.'}</p>
            )}
            {!!timeline?.items.length && (
              <ol className="space-y-4">
                {timeline.items.map(({ event, receivedAt }) => (
                  <li
                    key={event.id}
                    className="grid gap-1 border-s-2 border-current ps-3 sm:grid-cols-[13rem_1fr] sm:gap-4"
                  >
                    <time className="muted" dateTime={receivedAt}>
                      {new Date(receivedAt).toLocaleString(language)}
                    </time>
                    <div>
                      <strong>{screens[event.screen][locale]}</strong>
                      {event.kind === 'screen' && <p>{ar ? 'فُتحت الصفحة' : 'Page opened'}</p>}
                      {event.kind === 'action' && (
                        <p>
                          {actions[event.action][locale]} · {outcomes[event.outcome][locale]}
                        </p>
                      )}
                      {event.kind === 'error' && (
                        <p>
                          {event.code === 'runtime'
                            ? ar
                              ? 'خطأ في المتصفح'
                              : 'Browser error'
                            : ar
                              ? 'خطأ غير معالج'
                              : 'Unhandled promise error'}
                        </p>
                      )}
                      {event.kind === 'vital' && (
                        <p dir="ltr" className="inline-block">
                          {event.metric}{' '}
                          {new Intl.NumberFormat(language, { maximumFractionDigits: 3 }).format(
                            event.value,
                          )}
                          {event.metric === 'CLS' ? '' : ' ms'}
                        </p>
                      )}
                    </div>
                  </li>
                ))}
              </ol>
            )}
            {timeline?.nextCursor && (
              <button
                type="button"
                className="secondary"
                disabled={loading}
                onClick={() => void loadTimeline(selected, timeline.nextCursor ?? undefined)}
              >
                {ar ? 'أحداث أقدم' : 'Older events'}
              </button>
            )}
          </section>
        ) : (
          <section ref={listSection} tabIndex={-1} className="panel space-y-4" aria-busy={loading}>
            <div className="flex flex-wrap items-end gap-3">
              <label>
                {ar ? 'الدور' : 'Role'}
                <select
                  value={role}
                  onChange={(event) => {
                    const value = event.currentTarget.value
                    setRole(value)
                    void loadList(value, actor)
                  }}
                >
                  <option value="">{ar ? 'كل الأدوار' : 'All roles'}</option>
                  {Object.entries(roles).map(([value, label]) => (
                    <option key={value} value={value}>
                      {label[locale]}
                    </option>
                  ))}
                </select>
              </label>
              {actor && (
                <button
                  type="button"
                  className="secondary"
                  onClick={() => {
                    setActor(null)
                    void loadList(role, null)
                  }}
                >
                  {ar ? 'عرض كل الحسابات' : 'Show all accounts'}
                </button>
              )}
              <button
                type="button"
                className="secondary"
                disabled={loading}
                onClick={() => void loadList(role, actor)}
              >
                {ar ? 'تحديث' : 'Refresh'}
              </button>
            </div>
            {actor && (
              <p className="break-words" dir="auto">
                {actor.actorName || actor.actorId}
              </p>
            )}
            {journeys?.items.length === 0 && (
              <p>{ar ? 'لا توجد جلسات مسجّلة.' : 'No recorded journeys.'}</p>
            )}
            <ul className="space-y-3">
              {journeys?.items.map((item) => (
                <li className="role-account min-w-0" key={item.actorId + ':' + item.journeyId}>
                  <div className="grid min-w-0 gap-3 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] sm:items-end">
                    <div className="min-w-0">
                      <strong dir="auto" className="break-words">
                        {item.actorName || item.actorId}
                      </strong>
                      <p className="muted">{roles[item.role][locale]}</p>
                    </div>
                    <div>
                      <p className="muted">{ar ? 'آخر نشاط' : 'Last activity'}</p>
                      <time dateTime={item.lastSeenAt}>
                        {new Date(item.lastSeenAt).toLocaleString(language)}
                      </time>
                    </div>
                    <div>
                      <p className="muted">{ar ? 'الأحداث' : 'Events'}</p>
                      <p>{new Intl.NumberFormat(language).format(item.eventCount)}</p>
                    </div>
                  </div>
                  <div className="mt-3 flex flex-wrap gap-2">
                    <button
                      type="button"
                      className="secondary"
                      id={'journey-' + item.actorId + '-' + item.journeyId}
                      onClick={(event) => {
                        journeyTrigger.current = event.currentTarget.id
                        setSelected(item)
                        void loadTimeline(item)
                      }}
                    >
                      {ar ? 'عرض التسلسل' : 'View timeline'}
                    </button>
                    {!actor && (
                      <button
                        type="button"
                        className="secondary"
                        onClick={() => {
                          setActor(item)
                          void loadList(role, item)
                        }}
                      >
                        {ar ? 'جلسات هذا الحساب' : 'Journeys for this account'}
                      </button>
                    )}
                  </div>
                </li>
              ))}
            </ul>
            {journeys?.nextCursor && (
              <button
                type="button"
                className="secondary"
                disabled={loading}
                onClick={() => void loadList(role, actor, journeys.nextCursor ?? undefined)}
              >
                {ar ? 'جلسات أقدم' : 'Older journeys'}
              </button>
            )}
          </section>
        )}
        {loading && <p role="status">{ar ? 'جار التحميل…' : 'Loading…'}</p>}
        {error && <p role="alert">{ar ? 'تعذر تحميل البيانات.' : 'Could not load diagnostics.'}</p>}
      </main>
    </div>
  )
}
