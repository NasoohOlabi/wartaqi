import { useEffect, useRef } from 'react'
import type { AttendanceReplayAction, AttendanceReplaySnapshot } from '../shared/attendanceReplay'
import { describeAttendanceReplayAction } from './attendanceReplayLabels'
import { AttendanceRows } from './AttendanceRows'
import { LanguageContext, type Language } from './useLanguage'
import './AttendanceReplayView.css'

export function AttendanceReplayView({
  snapshot,
  action,
  language,
}: {
  snapshot: AttendanceReplaySnapshot
  action?: AttendanceReplayAction
  language: Language
}) {
  const ar = language === 'ar'
  const rows = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const viewport = rows.current
    const target = viewport?.querySelector('[data-replay-highlight="true"]')
    if (!viewport || !target) return
    const bounds = viewport.getBoundingClientRect()
    const row = target.getBoundingClientRect()
    if (row.top < bounds.top || row.bottom > bounds.bottom)
      viewport.scrollTop += row.top - bounds.top
  }, [action])
  const label = action && describeAttendanceReplayAction(snapshot, action, ar)

  return (
    <LanguageContext.Provider value={{ language, setLanguage: () => {} }}>
      <div
        dir={ar ? 'rtl' : 'ltr'}
        lang={language}
        className="attendance-replay panel min-w-0 space-y-3"
      >
        <h3 className="title">
          {ar ? 'الحضور' : 'Attendance'}
          {snapshot.courseName && ` · ${snapshot.courseName}`}
        </h3>
        <p>
          {snapshot.date} · {ar ? 'تغييرات غير محفوظة' : 'Pending changes'}: {snapshot.pendingCount}
        </p>
        {label && <p role="status">{label}</p>}
        <div className="attendance-replay-rows" ref={rows}>
          <AttendanceRows
            rows={snapshot.rows}
            canCorrect={snapshot.canCorrect}
            highlight={action}
            header={
              <thead>
                <tr>
                  <th>{ar ? 'الطالب' : 'Student'}</th>
                  <th>{ar ? 'المستوى' : 'Level'}</th>
                  <th>{ar ? 'المعلم' : 'Teacher'}</th>
                  <th>{ar ? 'الحضور' : 'Attendance'}</th>
                </tr>
              </thead>
            }
          />
        </div>
        {snapshot.totalRows > snapshot.rows.length && (
          <p className="muted">
            {ar ? 'الصفوف المسجّلة' : 'Recorded rows'} {snapshot.rows.length} / {snapshot.totalRows}
          </p>
        )}
        {snapshot.dialog && (
          <p>
            {snapshot.dialog.kind === 'note'
              ? ar
                ? 'ملاحظة'
                : 'Note'
              : ar
                ? 'تصحيح'
                : 'Correction'}
            : {snapshot.dialog.name}
          </p>
        )}
      </div>
    </LanguageContext.Provider>
  )
}
