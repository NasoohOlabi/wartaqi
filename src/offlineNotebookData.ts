import { neonSnapshotSchema, type NeonTeacher } from '../shared/neon'
import type { OfflineSnapshot, PendingAttendance, PendingStudent } from './offlineAttendanceStore'

export function offlineNotebook(
  snapshot: OfflineSnapshot,
  teacher: NeonTeacher,
  attendance: PendingAttendance[],
  students: PendingStudent[],
) {
  const event = (id: string, payload: object) => ({
    id,
    actor: teacher.name,
    clientTime: '1970-01-01T00:00:00.000Z',
    receivedAt: '',
    payload,
  })
  const contacts = new Map(
    students.flatMap(({ operation }) =>
      operation.kind === 'contact' ? [[operation.studentId, operation.whatsapp] as const] : [],
    ),
  )
  return neonSnapshotSchema.parse({
    teacher: { ...teacher, invitationProfile: undefined },
    students: snapshot.students.map((student) => ({
      kind: 'student',
      studentId: student.id,
      name: student.name,
      group: student.group,
      active: true,
      whatsapp: contacts.has(student.id) ? contacts.get(student.id) : student.phone,
    })),
    courses: snapshot.courses.map(({ id, name, open }) => ({ id, name, open })),
    enrollments: snapshot.courses.flatMap((course) =>
      course.enrollments.map((entry) => ({
        ...entry,
        courseId: course.id,
        practical: null,
        theory: null,
      })),
    ),
    events: [
      ...snapshot.attendance.map(({ eventId, ...payload }) =>
        event(eventId, { kind: 'attendance', ...payload }),
      ),
      ...snapshot.notes.map(({ id, ...payload }) => event(id, { kind: 'note', ...payload })),
      ...attendance.map((entry) => entry.event),
      ...students.flatMap(({ operation }) => (operation.kind === 'note' ? [operation.event] : [])),
    ],
  })
}
