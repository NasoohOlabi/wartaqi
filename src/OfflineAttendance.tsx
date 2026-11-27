import { useCallback, useEffect, useRef, useState } from 'react'
import { useLanguage } from './useLanguage'
import type { ProfileField } from '../shared/offlineStudent'
import {
  subscribeOfflineQueue,
  attendanceKey,
  getOfflineSnapshot,
  listPendingAttendance,
  listPendingStudent,
  resolveStudentConflict,
  snapshotKey,
  studentOperationId,
  type PendingStudent,
  resolveAttendanceConflict,
  type PendingAttendance,
  type OfflineSnapshot,
} from './offlineAttendanceStore'
import type { WorkspaceBack } from './WorkspaceNavigation'
import { localGrant, offlineAccess } from './offlineAccess'

const profileFieldLabels: Record<ProfileField, [string, string]> = {
  firstName: ['first name', 'الاسم الأول'],
  lastName: ['last name', 'اسم العائلة'],
  whatsapp: ['phone', 'الهاتف'],
  location: ['location', 'المنطقة'],
  birthYear: ['birth year', 'سنة الميلاد'],
}

function mayReview(userId: string) {
  const grant = localGrant(userId)
  return grant
    ? grant.account.role !== 'student' && grant.account.canWrite
    : offlineAccess()?.userId === userId
}

export function OfflineAttendance({
  userId,
  back,
  syncChanges,
  syncing,
  offline,
}: {
  userId: string
  back: WorkspaceBack
  syncChanges: (retryBlocked?: boolean) => Promise<boolean>
  syncing: boolean
  offline: boolean
}) {
  const { language, t } = useLanguage()
  const ar = language === 'ar'
  const [pending, setPending] = useState<PendingAttendance[]>([])
  const [snapshot, setSnapshot] = useState<OfflineSnapshot | null>(null)
  const [studentPending, setStudentPending] = useState<PendingStudent[]>([])
  const [error, setError] = useState('')
  const reviewHeading = useRef<HTMLHeadingElement>(null)
  const [leaseExpired, setLeaseExpired] = useState(false)
  const [loaded, setLoaded] = useState(false)
  const revision = useRef(0)
  const refresh = useCallback(async () => {
    if (!mayReview(userId)) {
      setLeaseExpired(true)
      return
    }
    const current = ++revision.current
    const key = snapshotKey(userId)
    const [snapshot, marks, students] = await Promise.all([
      getOfflineSnapshot(userId, key),
      listPendingAttendance(userId),
      listPendingStudent(userId),
    ])
    if (current !== revision.current) return
    if (!mayReview(userId) || snapshotKey(userId) !== key) {
      setLeaseExpired(true)
      return
    }
    setSnapshot(snapshot)
    setPending(marks)
    setStudentPending(students)
    setLoaded(true)
  }, [userId])
  useEffect(() => {
    const readQueue = () => void refresh().catch(() => setError('Offline storage is unavailable.'))
    queueMicrotask(readQueue)
    const unsubscribe = subscribeOfflineQueue(readQueue)
    const timer = window.setInterval(() => {
      if (!mayReview(userId)) setLeaseExpired(true)
    }, 1000)
    return () => {
      revision.current += 1
      unsubscribe()
      window.clearInterval(timer)
    }
  }, [refresh, userId])
  useEffect(() => {
    reviewHeading.current?.focus({ preventScroll: true })
  }, [])
  const sync = async (retryBlocked = false) => {
    if (offline || syncing) return
    const connected = await syncChanges(retryBlocked)
    if (connected) {
      await refresh()
      setError('')
    } else {
      setError(t('The request failed. Please try again.'))
    }
  }
  const pendingCount = pending.length + studentPending.length
  if (leaseExpired)
    return (
      <section className="max-w-4xl space-y-4">
        <p role="alert">
          {ar
            ? '\u0627\u0646\u062a\u0647\u062a \u0635\u0644\u0627\u062d\u064a\u0629 \u0627\u0644\u0648\u0635\u0648\u0644'
            : 'Offline access expired. Connect and sign in again.'}
        </p>
        <button className="secondary workspace-page-back" onClick={back.onClick}>
          {back.label}
        </button>
      </section>
    )
  const changeStatus = (
    kind?: 'conflict' | 'id_conflict' | 'forbidden' | 'saved' | 'unchanged',
  ) => {
    switch (kind) {
      case 'conflict':
        return ar ? 'تعارض — يلزم المراجعة' : 'Conflict — review required'
      case 'id_conflict':
        return ar ? 'يلزم مراجعة المسؤول' : 'Needs administrator review'
      case 'forbidden':
        return ar
          ? 'متوقف — حاول المزامنة عند استعادة الصلاحية'
          : 'Blocked — retry when access returns'
      default:
        return ar ? 'بانتظار المزامنة' : 'Pending sync'
    }
  }
  return (
    <section className="max-w-4xl space-y-4">
      <header className="workspace-welcome flex-wrap">
        <div className="space-y-1">
          <h1 ref={reviewHeading} tabIndex={-1}>
            {ar ? '\u062a\u063a\u064a\u064a\u0631\u0627\u062a\u064a' : 'My changes'}
          </h1>
          <p role="status" className="muted">
            {!loaded
              ? t('Loading records\u2026')
              : pendingCount > 0
                ? `${pendingCount} ${ar ? '\u0628\u0627\u0646\u062a\u0638\u0627\u0631 \u0627\u0644\u0645\u0632\u0627\u0645\u0646\u0629' : 'pending sync'}`
                : ''}
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          {pendingCount > 0 && (
            <button
              className="primary"
              disabled={syncing || offline}
              data-diagnostic-action="sync"
              onClick={() => void sync(true)}
            >
              {ar ? '\u0645\u0632\u0627\u0645\u0646\u0629 \u0627\u0644\u0622\u0646' : 'Sync now'}
            </button>
          )}
          <button className="secondary workspace-page-back" onClick={back.onClick}>
            {back.label}
          </button>
        </div>
      </header>
      {error && <p role="alert">{error}</p>}
      {loaded && pendingCount === 0 && (
        <p role="status" className="panel">
          {ar ? 'لا توجد تغييرات بانتظار المزامنة.' : 'No changes waiting to sync.'}
        </p>
      )}
      {(pending.length > 0 || studentPending.length > 0) && (
        <section className="space-y-2" aria-label={ar ? 'التغييرات المنتظرة' : 'Pending changes'}>
          {pending.map((item) => (
            <div key={item.event.id} className="panel space-y-3">
              <p className="font-semibold">
                <bdi>
                  {snapshot?.students.find((student) => student.id === item.event.payload.studentId)
                    ?.name ?? (ar ? 'طالب غير متاح' : 'Student unavailable')}
                </bdi>
              </p>
              <p className="muted">
                {t('Attendance')} · <bdi>{item.event.payload.date}</bdi> ·{' '}
                {changeStatus(item.outcome?.kind)}
                {item.outcome?.kind !== 'conflict' && (
                  <> · {t(item.event.payload.status ?? 'Not marked')}</>
                )}
              </p>
              {item.outcome?.kind === 'conflict' && (
                <div className="flex flex-wrap gap-2">
                  <p className="basis-full">
                    {ar
                      ? '\u0627\u0644\u0639\u0644\u0627\u0645\u0629 \u0627\u0644\u0645\u062d\u0641\u0648\u0638\u0629'
                      : 'Saved mark'}
                    :{' '}
                    {item.outcome.current?.status
                      ? t(item.outcome.current.status)
                      : t('Not marked')}{' '}
                    - {ar ? '\u0639\u0644\u0627\u0645\u062a\u064a' : 'My mark'}:{' '}
                    {t(
                      pending.findLast(
                        (candidate) => attendanceKey(candidate) === attendanceKey(item),
                      )?.event.payload.status ?? 'Not marked',
                    )}
                  </p>
                  <button
                    className="secondary"
                    onClick={() =>
                      void resolveAttendanceConflict(userId, item.event.id, 'server')
                        .then(refresh)
                        .catch(() => setError('Could not resolve conflict.'))
                    }
                  >
                    {ar
                      ? '\u0627\u0644\u0627\u062d\u062a\u0641\u0627\u0638 \u0628\u0639\u0644\u0627\u0645\u0629 \u0627\u0644\u062e\u0627\u062f\u0645'
                      : 'Keep saved mark'}
                  </button>
                  <button
                    className="secondary"
                    onClick={() =>
                      void resolveAttendanceConflict(userId, item.event.id, 'local')
                        .then(async () => {
                          await refresh()
                          await sync()
                        })
                        .catch(() => setError('Could not resolve conflict.'))
                    }
                  >
                    {ar
                      ? '\u0627\u0633\u062a\u062e\u062f\u0627\u0645 \u0639\u0644\u0627\u0645\u062a\u064a'
                      : 'Use my mark'}
                  </button>
                </div>
              )}
            </div>
          ))}
          {studentPending.map((item) => {
            const operation = item.operation
            const id = studentOperationId(operation)
            const student =
              operation.kind === 'note' ? operation.event.payload.studentId : operation.studentId
            const latestContact = studentPending.findLast(
              ({ operation }) => operation.kind === 'contact' && operation.studentId === student,
            )?.operation
            return (
              <div key={id} className="panel space-y-3">
                <p className="font-semibold">
                  <bdi>
                    {snapshot?.students.find((entry) => entry.id === student)?.name ??
                      (ar ? 'طالب غير متاح' : 'Student unavailable')}
                  </bdi>
                </p>
                <p className="muted">
                  {operation.kind === 'note'
                    ? ar
                      ? 'ملاحظة'
                      : 'Note'
                    : operation.kind === 'contact'
                      ? ar
                        ? 'هاتف'
                        : 'Phone'
                      : ar
                        ? 'معلومات الطالب'
                        : 'Student information'}{' '}
                  · {changeStatus(item.outcome?.kind)}
                </p>
                {operation.kind === 'profile-v1' &&
                  item.outcome?.kind === 'conflict' &&
                  item.outcome.profileConflicts?.map((field) => {
                    const [en, arabic] = profileFieldLabels[field]
                    const label = ar ? arabic : en
                    return (
                      <div key={field} className="flex flex-wrap gap-2">
                        <p className="basis-full">
                          {ar ? `${label} المحفوظ` : `Saved ${label}`}:{' '}
                          <bdi>{item.outcome?.profile?.[field] ?? '—'}</bdi> ·{' '}
                          {ar ? `${label} الذي أدخلته` : `Your ${label}`}:{' '}
                          <bdi>{operation.changes[field] ?? '—'}</bdi>
                        </p>
                        <button
                          className="secondary"
                          onClick={() =>
                            void resolveStudentConflict(userId, id, 'server', field)
                              .then(async () => {
                                await refresh()
                                await sync()
                              })
                              .catch(() =>
                                setError(ar ? 'تعذّر حل التعارض.' : 'Could not resolve conflict.'),
                              )
                          }
                        >
                          {ar ? `الاحتفاظ بالقيمة المحفوظة لحقل ${label}` : `Keep saved ${label}`}
                        </button>
                        <button
                          className="secondary"
                          onClick={() =>
                            void resolveStudentConflict(userId, id, 'local', field)
                              .then(async () => {
                                await refresh()
                                await sync()
                              })
                              .catch(() =>
                                setError(ar ? 'تعذّر حل التعارض.' : 'Could not resolve conflict.'),
                              )
                          }
                        >
                          {ar ? `استخدام القيمة التي أدخلتها لحقل ${label}` : `Use my ${label}`}
                        </button>
                      </div>
                    )
                  })}
                {item.outcome?.kind === 'conflict' && operation.kind === 'contact' && (
                  <div className="flex flex-wrap gap-2">
                    <p className="basis-full">
                      {ar ? 'الهاتف المحفوظ' : 'Saved phone'}:{' '}
                      <bdi>{item.outcome.currentWhatsapp ?? '-'}</bdi> ·{' '}
                      {ar ? 'هاتفي' : 'Your phone'}:{' '}
                      <bdi>
                        {(latestContact?.kind === 'contact'
                          ? latestContact.whatsapp
                          : operation.whatsapp) ?? '-'}
                      </bdi>
                    </p>
                    <button
                      className="secondary"
                      onClick={() =>
                        void resolveStudentConflict(userId, id, 'server')
                          .then(refresh)
                          .catch(() => setError('Could not resolve conflict.'))
                      }
                    >
                      {ar ? 'استخدام المحفوظ' : 'Keep saved phone'}
                    </button>
                    <button
                      className="secondary"
                      onClick={() =>
                        void resolveStudentConflict(userId, id, 'local')
                          .then(async () => {
                            await refresh()
                            await sync()
                          })
                          .catch(() => setError('Could not resolve conflict.'))
                      }
                    >
                      {ar ? 'استخدام هاتفي' : 'Use my phone'}
                    </button>
                  </div>
                )}
              </div>
            )
          })}
        </section>
      )}
    </section>
  )
}
