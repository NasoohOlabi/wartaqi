import { expect, test } from 'bun:test'
import { makeEvent, projectEvents } from '../shared/ledger'
import { neonSnapshotSchema } from '../shared/neon'
import { offlineAttendanceRecordSchema } from '../shared/offlineAttendance'
import { overlayNotebook } from '../src/offlineNotebookData'

test('staff queue overlays preserve original ledger history and full authorized course data', () => {
  const studentId = crypto.randomUUID()
  const courseId = crypto.randomUUID()
  const enrollmentId = crypto.randomUUID()
  const date = '2026-11-27'
  const teacher = 'Synthetic Teacher'
  const previous = makeEvent(teacher, {
    kind: 'attendance',
    studentId,
    enrollmentId,
    date,
    status: 'late',
  })
  const correction = makeEvent(teacher, {
    kind: 'reversal',
    targetId: previous.id,
    reason: 'Recorded twice',
  })
  const intended = makeEvent(teacher, {
    kind: 'attendance',
    studentId,
    enrollmentId,
    date,
    status: 'absent',
  })
  const snapshot = neonSnapshotSchema.parse({
    teacher: {
      id: crypto.randomUUID(),
      name: teacher,
      canRead: true,
      canWrite: true,
      expiresAt: null,
    },
    students: [{ kind: 'student', studentId, name: 'Synthetic Student', group: '', active: true }],
    courses: [{ id: courseId, name: 'Synthetic Course', open: true }],
    enrollments: [
      {
        id: enrollmentId,
        courseId,
        studentId,
        level: 'Two',
        practical: 88,
        theory: 91,
        active: true,
      },
    ],
    events: [previous, correction],
  })
  const pending = {
    ...offlineAttendanceRecordSchema.parse({ event: intended, expectedEventId: null }),
    outcome: null,
  }
  const projected = overlayNotebook(snapshot, [pending], [])
  expect(projected.enrollments).toEqual(snapshot.enrollments)
  expect(projected.events).toEqual([previous, correction, intended])
  expect(projectEvents(projected.events).attendance.map(({ value }) => value.status)).toEqual([
    'absent',
  ])
  expect(overlayNotebook(projected, [pending], []).events).toEqual(projected.events)
  expect(
    overlayNotebook({ ...snapshot, students: [], enrollments: [], events: [] }, [pending], [])
      .events,
  ).toEqual([])
})
