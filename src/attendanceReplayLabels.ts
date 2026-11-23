import type { AttendanceReplayAction, AttendanceReplaySnapshot } from '../shared/attendanceReplay'

export function describeAttendanceReplayAction(
  snapshot: AttendanceReplaySnapshot,
  action: AttendanceReplayAction,
  ar: boolean,
) {
  const name =
    'studentId' in action
      ? snapshot.rows.find((row) => row.id === action.studentId)?.name
      : undefined
  if (action.kind === 'mark' && name) {
    const status = {
      present: ar ? 'حاضر' : 'present',
      absent: ar ? 'غائب' : 'absent',
      excused: ar ? 'معذور' : 'excused',
    }[action.status]
    return ar ? `تم تسجيل ${name}: ${status}` : `Marked ${name} ${status}`
  }
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
  return `${actionLabels[action.kind]}${name ? ` · ${name}` : ''}`
}
