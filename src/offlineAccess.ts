import { z } from 'zod'
import {
  neonStudentAccountSchema,
  neonTeacherSchema,
  type NeonAccount,
  type NeonTeacher,
} from '../shared/neon'

const key = 'halaqa-offline-access'
export type CachedAccess = { offline: boolean; reconnect: () => void }

export const grantStorageKey = 'wartaq-local-grant'
export const grantChangeEvent = 'wartaq-local-grant-change'
const localAccountSchema = z.union([
  neonTeacherSchema.omit({ invitationProfile: true }),
  neonStudentAccountSchema.omit({ invitationProfile: true }),
])
const grantSchema = z.object({
  userId: z.string(),
  generation: z.uuid(),
  account: localAccountSchema,
})

function readGrant() {
  try {
    return grantSchema.parse(JSON.parse(localStorage.getItem(grantStorageKey) ?? 'null'))
  } catch {
    return null
  }
}

export function localGrant(userId?: string) {
  const grant = readGrant()
  if (
    !grant ||
    (userId !== undefined && grant.userId !== userId) ||
    !grant.account.canRead ||
    (grant.account.expiresAt !== null && Date.parse(grant.account.expiresAt) <= Date.now())
  )
    return null
  return grant
}

export function authorizeLocalGrant(userId: string, account: NeonAccount) {
  if (
    !account.canRead ||
    account.invitationProfile?.requiresSetup ||
    (account.expiresAt !== null && Date.parse(account.expiresAt) <= Date.now())
  ) {
    clearOfflineAccess()
    return
  }
  const previous = localGrant(userId)
  const legacy = offlineAccess()
  const sameScope =
    previous?.account.id === account.id &&
    previous.account.role === account.role &&
    previous.account.canWrite === account.canWrite &&
    previous.account.expiresAt === account.expiresAt
  if (
    !sameScope &&
    (readGrant() ||
      (legacy &&
        (legacy.userId !== userId ||
          !account.canWrite ||
          (legacy.teacher &&
            (legacy.teacher.id !== account.id || legacy.teacher.role !== account.role)))))
  )
    clearOfflineAccess()
  try {
    const grant = grantSchema.parse({
      userId,
      generation: sameScope ? previous.generation : crypto.randomUUID(),
      account,
    })
    const serialized = JSON.stringify(grant)
    if (localStorage.getItem(grantStorageKey) !== serialized) {
      localStorage.setItem(grantStorageKey, serialized)
      window.dispatchEvent(new Event(grantChangeEvent))
    }
  } catch {
    /* Storage may be unavailable. */
  }
}

const schema = z.object({
  userId: z.string(),
  actor: z.string(),
  expires: z.number().finite().max(8.64e15).nullable(),
  teacher: neonTeacherSchema.omit({ invitationProfile: true }).optional(),
})
export function offlineAccess() {
  try {
    const value = schema.parse(JSON.parse(localStorage.getItem(key) ?? 'null'))
    return value.expires === null || value.expires > Date.now() ? value : null
  } catch {
    return null
  }
}
export function allowOfflineAccess(
  userId: string,
  actor: string,
  expires: number | null,
  teacher?: NeonTeacher,
) {
  localStorage.setItem(key, JSON.stringify(schema.parse({ userId, actor, expires, teacher })))
}
export function clearOfflineAccess() {
  clearLegacyOfflineAccess()
  try {
    localStorage.removeItem(grantStorageKey)
    window.dispatchEvent(new Event(grantChangeEvent))
  } catch {
    /* Storage may be unavailable. */
  }
}

export function clearLegacyOfflineAccess() {
  try {
    localStorage.removeItem(key)
  } catch {
    /* Storage may be unavailable. */
  }
}
