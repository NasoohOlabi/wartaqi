import { offlineAttendanceSyncSchema } from '../shared/offlineAttendance'
import { offlineStudentSyncSchema } from '../shared/offlineStudent'
import type { LedgerEvent } from '../shared/ledger'

// Exercise real clients against deterministic receipts. Postgres independently tests the CAS rules.
export function acceptSyntheticSync(path: string, body: unknown, events: LedgerEvent[]) {
  const append = (event: LedgerEvent) => {
    if (!events.some(({ id }) => id === event.id)) events.push(event)
  }
  if (path === '/api/attendance/sync') {
    const { records } = offlineAttendanceSyncSchema.parse(body)
    return {
      outcomes: records.map(({ event }) => {
        append(event)
        return {
          id: event.id,
          kind: 'saved',
          current: { eventId: event.id, status: event.payload.status },
        }
      }),
    }
  }
  if (path === '/api/student/sync') {
    const { operation } = offlineStudentSyncSchema.parse(body)
    if (operation.kind !== 'note')
      throw new Error('This fixture only prepares attendance and notes')
    append(operation.event)
    return { id: operation.event.id, kind: 'saved', currentWhatsapp: null }
  }
}
