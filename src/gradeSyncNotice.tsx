import { useEffect, useState } from 'react'
import {
  gradeAcceptedEvent,
  gradeScope,
  replayGrades,
  resolveGradeEdit,
  retryRecoveredGrade,
  subscribeGradeQueue,
  useGradeQueue,
} from './gradeSync'
import { subscribeWorkspaceChanges } from './syncStream'
import { useLanguage } from './useLanguage'

export function GradeSyncNotice({ userId, confirmed }: { userId: string; confirmed: boolean }) {
  const ar = useLanguage().language === 'ar'
  const items = useGradeQueue(userId)
  const recovered = useGradeQueue(userId, confirmed ? 'recover' : 'hidden')
  const generation = gradeScope(userId)?.generation
  const [acceptedGeneration, setAcceptedGeneration] = useState<string>()
  const [errorGeneration, setErrorGeneration] = useState<string>()
  useEffect(() => {
    const scope = gradeScope(userId)
    if (!scope || scope.generation !== generation) return
    let timeout: number | undefined
    function accepted(event: Event) {
      if (!(event instanceof CustomEvent) || event.detail !== generation) return
      setAcceptedGeneration(generation)
      window.clearTimeout(timeout)
      timeout = window.setTimeout(() => setAcceptedGeneration(undefined), 4000)
    }
    const sync = () => replayGrades(scope).catch(() => setErrorGeneration(scope.generation))
    const unsubscribe = subscribeGradeQueue(() => void sync())
    const disconnect = subscribeWorkspaceChanges(userId, sync)
    window.addEventListener(gradeAcceptedEvent, accepted)
    return () => {
      window.clearTimeout(timeout)
      unsubscribe()
      disconnect()
      window.removeEventListener(gradeAcceptedEvent, accepted)
    }
  }, [generation, userId])
  const scope = gradeScope(userId)
  if (!scope) return null
  const supported = Boolean(navigator.locks)
  const showError = errorGeneration === generation
  const resolve = (item: (typeof items)[number], mine: boolean) => {
    setErrorGeneration(undefined)
    void resolveGradeEdit(scope, item, mine).catch(() => setErrorGeneration(scope.generation))
  }
  return (
    <>
      {acceptedGeneration === generation && (
        <div className="course-save-status" role="status">
          <span className="rounded-lg bg-[var(--dark-active,#004d57)] px-4 py-2.5 text-sm font-medium text-white shadow-lg">
            {ar ? 'تم الحفظ' : 'Saved'}
          </span>
        </div>
      )}
      {showError && (
        <p className="form-error" role="alert">
          {ar
            ? 'تعذر تحديث الدرجات المحفوظة على الجهاز.'
            : 'Unable to update the grades saved on this device.'}
        </p>
      )}
      {!supported && (
        <p className="form-error" role="alert">
          {ar
            ? 'هذا المتصفح لا يدعم حفظ الدرجات ومزامنتها بأمان.'
            : 'This browser cannot safely save and sync grade changes.'}
        </p>
      )}
      {items.length + recovered.length > 0 && (
        <aside className="grid gap-2" aria-label={ar ? 'مزامنة الدرجات' : 'Grade sync'}>
          {items.some(({ outcome }) => !outcome) && (
            <p role="status">
              {ar
                ? 'تم حفظ تغييرات الدرجات على هذا الجهاز بانتظار المزامنة.'
                : 'Grade changes saved on this device and waiting to sync.'}
            </p>
          )}
          {[...items, ...recovered].map((item) => {
            const { operation, outcome, label } = item
            const previousGrant = item.generation !== generation
            if (!outcome && !previousGrant) return null
            const fields = [
              {
                name: ar ? 'عملي' : 'Practical',
                edit: operation.fields.practical,
                latest: outcome?.status === 'conflict' ? outcome.current.practical : null,
              },
              {
                name: ar ? 'نظري' : 'Theory',
                edit: operation.fields.theory,
                latest: outcome?.status === 'conflict' ? outcome.current.theory : null,
              },
              {
                name: ar ? 'ملاحظة الأداء' : 'Performance note',
                edit: operation.fields.notes,
                latest: outcome?.status === 'conflict' ? outcome.current.notes : null,
              },
            ]
            return (
              <section
                key={JSON.stringify([item.generation, operation.requestId])}
                className="grid gap-2 rounded border p-3"
              >
                <p role="status">
                  <strong dir="auto">{label}</strong>{' '}
                  {outcome?.status === 'conflict'
                    ? ar
                      ? 'تحتاج تغييرات الدرجات إلى مراجعة.'
                      : 'Grade changes need review.'
                    : previousGrant
                      ? ar
                        ? 'تغييرات درجات محفوظة من تسجيل دخول سابق.'
                        : 'Grade changes saved during an earlier sign-in.'
                      : ar
                        ? 'تعذر إرسال تغييرات الدرجات.'
                        : 'Grade changes could not be delivered.'}
                </p>
                <dl className="grid gap-2 break-words whitespace-pre-wrap">
                  {fields.map(
                    ({ name, edit, latest }) =>
                      edit && (
                        <div key={name} className="grid gap-1">
                          <dt>{name}</dt>
                          {outcome?.status === 'conflict' && (
                            <dd>
                              {ar ? 'المحفوظ' : 'Saved value'}{' '}
                              <bdi>{latest === '' || latest === null ? '—' : latest}</bdi>
                            </dd>
                          )}
                          <dd>
                            {ar ? 'على جهازك' : 'Your value'}{' '}
                            <bdi>{edit.next === '' || edit.next === null ? '—' : edit.next}</bdi>
                          </dd>
                        </div>
                      ),
                  )}
                </dl>
                <div className="flex flex-wrap gap-2">
                  {outcome?.status === 'conflict' ? (
                    <button
                      type="button"
                      className="secondary-button"
                      disabled={!supported}
                      onClick={() => resolve(item, true)}
                    >
                      {ar ? 'احتفظ بتغييراتي' : 'Keep my grade changes'}
                    </button>
                  ) : (
                    <button
                      type="button"
                      className="secondary-button"
                      disabled={!navigator.onLine || !supported}
                      onClick={() => {
                        setErrorGeneration(undefined)
                        const retry = previousGrant
                          ? retryRecoveredGrade(scope, item)
                          : replayGrades(scope, true)
                        void retry.catch(() => setErrorGeneration(scope.generation))
                      }}
                    >
                      {previousGrant
                        ? ar
                          ? 'أرسل التغييرات السابقة'
                          : 'Send earlier grade changes'
                        : ar
                          ? 'زامن الآن'
                          : 'Sync grades now'}
                    </button>
                  )}
                  <button
                    type="button"
                    className="secondary-button"
                    disabled={!supported}
                    onClick={() => resolve(item, false)}
                  >
                    {outcome?.status === 'conflict'
                      ? ar
                        ? 'استخدم القيم المحفوظة'
                        : 'Use saved grade values'
                      : ar
                        ? 'تجاهل التغييرات المحلية'
                        : 'Discard local grade changes'}
                  </button>
                </div>
              </section>
            )
          })}
        </aside>
      )}
    </>
  )
}
