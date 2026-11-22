import { useEffect, useRef, useState } from 'react'
import { useLanguage } from './useLanguage'
import {
  subscribeOfflineQueue,
  attendanceKey,
  getOfflineSnapshot,
  listPendingAttendance,
  listPendingStudent,
  resolveStudentConflict,
  studentOperationId,
  type PendingStudent,
  resolveAttendanceConflict,
  type PendingAttendance,
  type OfflineSnapshot,
} from './offlineAttendanceStore'
import { offlineAccess } from './offlineAccess'

export function OfflineAttendance({
  userId,
  close,
  syncChanges,
  syncing,
  offline,
}: {
  userId: string
  close: () => void
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
  useEffect(() => {
    const timer = window.setInterval(() => {
      if (offlineAccess()?.userId !== userId) setLeaseExpired(true)
    }, 1000)
    return () => window.clearInterval(timer)
  }, [userId])
  const refresh = async () => {
    setSnapshot(await getOfflineSnapshot(userId))
    setPending(await listPendingAttendance(userId))
    setStudentPending(await listPendingStudent(userId))
    setLoaded(true)
  }
  // The store is external and must be read when the account changes.
  useEffect(() => {
    void refresh().catch(() => setError('Offline storage is unavailable.'))
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [userId])
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
  useEffect(
    () =>
      subscribeOfflineQueue(() => {
        void refresh().catch(() => setError('Offline storage is unavailable.'))
      }),
    // The subscription follows the account; refresh reads that account.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
    [userId],
  )
  const pendingCount = pending.length + studentPending.length
  if (leaseExpired)
    return (
      <section className="max-w-4xl space-y-4">
        <p role="alert">
          {ar
            ? '\u0627\u0646\u062a\u0647\u062a \u0635\u0644\u0627\u062d\u064a\u0629 \u0627\u0644\u0648\u0635\u0648\u0644'
            : 'Offline access expired. Connect and sign in again.'}
        </p>
        <button className="secondary" onClick={close}>
          {ar ? '\u0631\u062c\u0648\u0639' : 'Back'}
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
              onClick={() => void sync(true)}
            >
              {ar ? '\u0645\u0632\u0627\u0645\u0646\u0629 \u0627\u0644\u0622\u0646' : 'Sync now'}
            </button>
          )}
          <button className="secondary" onClick={close}>
            {ar ? '\u0631\u062c\u0648\u0639' : 'Back'}
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
                {item.outcome?.kind !== 'conflict' && <> · {t(item.event.payload.status)}</>}
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
                      )?.event.payload.status ?? item.event.payload.status,
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
                  {operation.kind === 'note' ? (ar ? 'ملاحظة' : 'Note') : ar ? 'هاتف' : 'Phone'} ·{' '}
                  {changeStatus(item.outcome?.kind)}
                </p>
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
