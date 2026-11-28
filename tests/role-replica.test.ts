import { expect, test } from 'vitest-runner'
import { neonAccountSchema } from '../shared/neon'
import {
  authorizeLocalGrant,
  clearOfflineAccess,
  grantStorageKey,
  localGrant,
} from '../src/offlineAccess'
import {
  adminBoardReplica,
  parseRoleProjection,
  replicaScope,
  studentAttendanceReplica,
} from '../src/roleReplica'

const studentId = '439a3c8c-50d4-48cf-9b47-9ecd2b6189aa'
const replacementId = 'c68e0569-dd55-4071-b12f-ed6cb566fae4'

test('local grant generations rotate when role, assignment or authorization changes', () => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'localStorage')
  const storage = new Map<string, string>()
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    value: {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, item: string) => storage.set(key, item),
      removeItem: (key: string) => storage.delete(key),
    },
  })
  try {
    const userId = 'synthetic-student'
    const account = neonAccountSchema.parse({
      role: 'student',
      id: studentId,
      name: 'Synthetic Student',
      canRead: true,
      canWrite: false,
      expiresAt: null,
      invitationProfile: { requiresSetup: false, phone: 'private-contact', verifiedPhone: null },
    })
    authorizeLocalGrant(userId, account)
    const generation = localGrant(userId)?.generation
    expect(generation).toBeDefined()
    expect(replicaScope(studentAttendanceReplica, userId)?.generation).toBe(generation)
    expect(replicaScope(adminBoardReplica, userId)).toBeNull()
    expect(replicaScope(studentAttendanceReplica, 'different-user')).toBeNull()
    expect(JSON.parse(storage.get(grantStorageKey) ?? '{}').account).not.toHaveProperty(
      'invitationProfile',
    )

    authorizeLocalGrant(userId, { ...account, name: 'Changed display name' })
    expect(localGrant(userId)?.generation).toBe(generation)

    authorizeLocalGrant(userId, { ...account, id: replacementId })
    const replacement = localGrant(userId)
    expect(replacement?.generation).not.toBe(generation)
    expect(replacement?.account.id).toBe(replacementId)

    authorizeLocalGrant(userId, {
      ...account,
      role: 'administrator',
      id: replacementId,
      canWrite: true,
    })
    expect(replicaScope(adminBoardReplica, userId)?.generation).not.toBe(replacement?.generation)
    expect(replicaScope(studentAttendanceReplica, userId)).toBeNull()

    authorizeLocalGrant(userId, { ...account, expiresAt: '2000-01-01T00:00:00Z' })
    expect(localGrant(userId)).toBeNull()
    authorizeLocalGrant(userId, account)
    expect(localGrant(userId)?.generation).not.toBe(generation)

    authorizeLocalGrant(userId, {
      ...account,
      invitationProfile: { requiresSetup: true, phone: null, verifiedPhone: null },
    })
    expect(localGrant(userId)).toBeNull()

    authorizeLocalGrant(userId, account)
    clearOfflineAccess()
    expect(localGrant(userId)).toBeNull()
  } finally {
    if (descriptor) Object.defineProperty(globalThis, 'localStorage', descriptor)
    else Reflect.deleteProperty(globalThis, 'localStorage')
  }
})

test('attendance projection rejects a different student and excludes other data', () => {
  const value = {
    student: { studentId, name: 'Synthetic Student', group: '2', note: 'private' },
    attendance: [{ date: '2026-11-18', status: 'present' }],
    events: [{ note: 'private' }],
  }
  expect(() => parseRoleProjection(studentAttendanceReplica, value, replacementId)).toThrow()
  expect(parseRoleProjection(studentAttendanceReplica, value, studentId)).toEqual({
    student: { studentId, name: 'Synthetic Student', group: '2' },
    attendance: [{ date: '2026-11-18', status: 'present' }],
  })
})
