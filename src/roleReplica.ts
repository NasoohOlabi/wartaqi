import { z } from 'zod'
import { neonAdminBoardSchema, neonStudentAttendanceSchema } from '../shared/neon'
import { localGrant } from './offlineAccess'

export const studentAttendanceReplica = {
  kind: 'student-attendance',
  role: 'student',
  path: '/api/student/attendance',
  schema: neonStudentAttendanceSchema,
  matches: (data: z.output<typeof neonStudentAttendanceSchema>, id: string) =>
    data.student.studentId === id,
} as const

export const adminBoardReplica = {
  kind: 'admin-board',
  role: 'administrator',
  path: '/api/admin',
  schema: neonAdminBoardSchema,
} as const

export type RoleReplica<T> = {
  kind: string
  role: 'administrator' | 'student'
  path: string
  schema: z.ZodType<T>
  matches?: (data: T, id: string) => boolean
}

export function replicaScope<T>(definition: RoleReplica<T>, userId: string) {
  const grant = localGrant(userId)
  return grant?.account.role === definition.role ? grant : null
}

function withReplica<T>(
  mode: IDBTransactionMode,
  access: (store: IDBObjectStore) => IDBRequest<T>,
): Promise<T> {
  return new Promise((resolve, reject) => {
    const opening = indexedDB.open('wartaq-role-replicas', 1)
    opening.onupgradeneeded = () => opening.result.createObjectStore('replicas')
    opening.onerror = () => reject(opening.error)
    opening.onsuccess = () => {
      const db = opening.result
      const tx = db.transaction('replicas', mode)
      const request = access(tx.objectStore('replicas'))
      tx.oncomplete = () => {
        db.close()
        resolve(request.result)
      }
      tx.onabort = () => {
        db.close()
        reject(tx.error)
      }
    }
  })
}

export function parseRoleProjection<T>(definition: RoleReplica<T>, value: unknown, id: string): T {
  const projection = definition.schema.parse(value)
  if (definition.matches && !definition.matches(projection, id))
    throw new Error('The request failed. Please try again.')
  return projection
}

export async function readRoleReplica<T>(
  definition: RoleReplica<T>,
  userId: string,
  generation: string,
) {
  const value: unknown = await withReplica('readonly', (store) =>
    store.get([userId, definition.kind, generation]),
  )
  const grant = replicaScope(definition, userId)
  if (value === undefined || grant?.generation !== generation) return null
  return parseRoleProjection(definition, value, grant.account.id)
}

export async function writeRoleReplica<T>(
  definition: RoleReplica<T>,
  userId: string,
  generation: string,
  data: T,
) {
  const grant = replicaScope(definition, userId)
  if (grant?.generation !== generation) return
  const projection = parseRoleProjection(definition, data, grant.account.id)
  await withReplica('readwrite', (store) =>
    store.put(projection, [userId, definition.kind, generation]),
  )
}
