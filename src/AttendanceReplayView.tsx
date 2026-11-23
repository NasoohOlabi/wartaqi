import { useEffect, useRef } from 'react'
import type { AttendanceReplayAction, AttendanceReplaySnapshot } from '../shared/attendanceReplay'
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
  const name =
    action && 'studentId' in action
      ? snapshot.rows.find((row) => row.id === action.studentId)?.name
      : undefined
  const status =
    action?.kind === 'mark'
      ? {
          present: ar ? 'حاضر' : 'present',
          absent: ar ? 'غائب' : 'absent',
          excused: ar ? 'معذور' : 'excused',
        }[action.status]
      : ''
  const actionLabels = {
    mark: ar ? 'تسجيل الحضور' : 'Marked attendance',
    student: ar ? 'فتح بيانات الطالب' : 'Opened student details',
    note: ar ? 'فتح ملاحظة' : 'Opened note',
    correction: ar ? 'فتح التصحيح' : 'Opened correction',
    save: ar ? 'ضغط حفظ الحضور' : 'Clicked Save attendance',
    bulk: ar ? 'تسجيل غير المسجلين حاضرين' : 'Marked unrecorded students present',
    unmarked: ar ? 'تبديل عرض غير المسجلين' : 'Toggled unmarked filter',
    date: ar ? 'فتح تاريخ الجلسة' : 'Opened session date',
  }
  const label =
    action?.kind === 'mark' && name
      ? ar
        ? `تم تسجيل ${name}: ${status}`
        : `Marked ${name} ${status}`
      : action && `${actionLabels[action.kind]}${name ? ` · ${name}` : ''}`

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
