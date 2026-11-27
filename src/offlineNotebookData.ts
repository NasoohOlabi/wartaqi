import {
  neonSnapshotSchema,
  type NeonRosterStudent,
  type NeonSnapshot,
  type NeonTeacher,
} from '../shared/neon'
import type { EditableProfile } from '../shared/offlineStudent'
import type { OfflineSnapshot, PendingAttendance, PendingStudent } from './offlineAttendanceStore'

export function applyStudentProfile(student: NeonRosterStudent, patch: Partial<EditableProfile>) {
  const updated = { ...student, ...patch }
  if (patch.firstName !== undefined || patch.lastName !== undefined)
    updated.name = [updated.firstName ?? student.name, updated.lastName].filter(Boolean).join(' ')
  return updated
}

export function offlineNotebook(
  snapshot: OfflineSnapshot,
  teacher: NeonTeacher,
  attendance: PendingAttendance[],
  students: PendingStudent[],
) {
  const confirmed = snapshot.notebook ?? legacyNotebook(snapshot, teacher)
  return overlayNotebook(confirmed, attendance, students)
}

export function overlayNotebook(
  confirmed: NeonSnapshot,
  attendance: PendingAttendance[],
  students: PendingStudent[],
): NeonSnapshot {
  const rosterIds = new Set(confirmed.students.map((student) => student.studentId))
  const enrollmentIds = new Set(confirmed.enrollments.map(({ id }) => id))
  const eventIds = new Set(confirmed.events.map(({ id }) => id))
  const eligible = (event: { id: string; payload: { studentId?: string } }) =>
    !eventIds.has(event.id) && !!event.payload.studentId && rosterIds.has(event.payload.studentId)
  const profiles = new Map<string, Partial<EditableProfile>>()
  for (const { operation } of students) {
    if (operation.kind === 'note') continue
    const patch =
      operation.kind === 'contact' ? { whatsapp: operation.whatsapp } : operation.changes
    profiles.set(operation.studentId, { ...profiles.get(operation.studentId), ...patch })
  }
  return {
    ...confirmed,
    students: profiles.size
      ? confirmed.students.map((student) => {
          const patch = profiles.get(student.studentId)
          return patch ? applyStudentProfile(student, patch) : student
        })
      : confirmed.students,
    events: [
      ...confirmed.events,
      ...attendance.flatMap(({ event }) =>
        eligible(event) && enrollmentIds.has(event.payload.enrollmentId) ? [event] : [],
      ),
      ...students.flatMap(({ operation }) =>
        operation.kind === 'note' && eligible(operation.event) ? [operation.event] : [],
      ),
    ],
  }
}

function legacyNotebook(snapshot: OfflineSnapshot, teacher: NeonTeacher) {
  const event = (id: string, payload: object) => ({
    id,
    actor: teacher.name,
    clientTime: '1970-01-01T00:00:00.000Z',
    receivedAt: '',
    payload,
  })
  return neonSnapshotSchema.parse({
    teacher: { ...teacher, invitationProfile: undefined },
    students: snapshot.students.map((student) => ({
      kind: 'student',
      studentId: student.id,
      name: student.name,
      group: student.group,
      active: true,
      whatsapp: student.phone,
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
    ],
  })
}
