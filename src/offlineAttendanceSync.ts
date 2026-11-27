import { offlineStudentOutcomeSchema } from '../shared/offlineStudent'
import { NeonRequestError, request } from './neon'
import { recordDiagnosticAction } from './diagnostics'
import {
  attendanceKey,
  listPendingAttendance,
  listPendingStudent,
  recordStudentOutcome,
  studentOperationId,
  recordAttendanceOutcome,
  snapshotKey,
  syncResponseSchema,
} from './offlineAttendanceStore'

async function sendQueuedOperation(path: string, userId: string, body: unknown): Promise<unknown> {
  for (let attempt = 0; ; attempt += 1) {
    try {
      return await request(path, userId, body, false)
    } catch (cause) {
      if (attempt === 2 || (cause instanceof NeonRequestError && cause.status < 500)) throw cause
      await new Promise((resolve) => setTimeout(resolve, 500 * 2 ** attempt))
    }
  }
}

export async function syncOfflineAttendance(
  userId: string,
  retryForbidden = false,
  cacheKey = snapshotKey(userId),
) {
  if (!navigator.locks)
    throw new Error('This browser cannot safely coordinate offline sync between tabs.')
  return navigator.locks.request(`halaqa-offline-sync:${userId}`, async () => {
    const pending = await listPendingAttendance(userId)
    const outcomes = []
    const blocked = new Set<string>()
    const accepted = new Map<string, string | null>()
    for (const item of pending) {
      if (snapshotKey(userId) !== cacheKey) break
      const key = attendanceKey(item)
      if (item.outcome && !(retryForbidden && item.outcome.kind === 'forbidden')) {
        blocked.add(key)
        continue
      }
      if (blocked.has(key)) continue
      const { event, expectedRevision } = item
      const expectedEventId =
        item.expectedEventId && accepted.has(item.expectedEventId)
          ? (accepted.get(item.expectedEventId) ?? null)
          : item.expectedEventId
      const result = await sendQueuedOperation('/api/attendance/sync', userId, {
        accountId: userId,
        records: [{ event, expectedEventId, expectedRevision }],
      })
      const response = syncResponseSchema.parse(result)
      if (response.outcomes.length !== 1 || response.outcomes[0]?.id !== item.event.id)
        throw new Error('The attendance sync response did not match the queued record.')
      const outcome = response.outcomes[0]
      await recordAttendanceOutcome(userId, outcome, cacheKey)
      recordDiagnosticAction(
        userId,
        'attendance',
        outcome.kind === 'saved' || outcome.kind === 'unchanged' ? 'synced' : 'failed',
      )
      outcomes.push(outcome)
      if (outcome.kind !== 'saved' && outcome.kind !== 'unchanged') blocked.add(key)
      else if (outcome.kind === 'unchanged')
        accepted.set(item.event.id, outcome.current?.eventId ?? null)
    }
    return outcomes
  })
}

export async function syncOfflineStudent(
  userId: string,
  retryForbidden = false,
  cacheKey = snapshotKey(userId),
) {
  if (!navigator.locks)
    throw new Error('This browser cannot safely coordinate offline sync between tabs.')
  return navigator.locks.request(`halaqa-offline-student-sync:${userId}`, async () => {
    const pending = await listPendingStudent(userId)
    const outcomes = []
    const blocked = new Set<string>()
    for (const item of pending) {
      if (snapshotKey(userId) !== cacheKey) break
      const operation = item.operation
      const key = operation.kind === 'note' ? operation.event.id : operation.studentId
      if (blocked.has(key)) continue
      if (item.outcome && !(retryForbidden && item.outcome.kind === 'forbidden')) {
        blocked.add(key)
        continue
      }
      const result = await sendQueuedOperation('/api/student/sync', userId, {
        accountId: userId,
        operation,
      })
      const outcome = offlineStudentOutcomeSchema.parse(result)
      if (outcome.id !== studentOperationId(operation))
        throw new Error('The student sync response did not match the queued record.')
      if (
        operation.kind === 'profile-v1' &&
        ((outcome.kind === 'saved' && !outcome.profile) ||
          (outcome.kind === 'conflict' && (!outcome.profile || !outcome.profileConflicts?.length)))
      )
        throw new Error('The profile sync response did not include the saved values.')
      await recordStudentOutcome(userId, outcome, cacheKey)
      recordDiagnosticAction(
        userId,
        operation.kind === 'note' ? 'note' : 'student',
        outcome.kind === 'saved' ? 'synced' : 'failed',
      )
      outcomes.push(outcome)
      if (outcome.kind !== 'saved') blocked.add(key)
    }
    return outcomes
  })
}
