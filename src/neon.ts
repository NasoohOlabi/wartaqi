import { createAuthClient } from 'better-auth/react'
import { z } from 'zod'
import { accountFetch } from './accountRequest'
import { recordDiagnosticAction } from './diagnostics'
import { localGrant } from './offlineAccess'
import type { DiagnosticsAction } from '../shared/diagnostics'
import { neonAccountSchema, neonBatchSchema, neonSnapshotSchema } from '../shared/neon'
import type { LedgerEvent } from '../shared/ledger'

export const apiUrl = window.location.origin
export const authClient = createAuthClient()

export const sessionChangeEvent = 'halaqa-session-change'
export const sessionChangeSource = crypto.randomUUID()

export function announceSessionChange() {
  try {
    localStorage.setItem(sessionChangeEvent, crypto.randomUUID())
  } catch {
    /* Storage can be disabled. */
  }
  try {
    const channel = new BroadcastChannel(sessionChangeEvent)
    channel.postMessage(sessionChangeSource)
    channel.close()
  } catch {
    /* BroadcastChannel can be unavailable. */
  }
}

export function resetWorkspace() {
  announceSessionChange()
  window.location.replace(window.location.pathname + '?page=app')
}

export class NeonRequestError extends Error {
  constructor(public status: number) {
    super(
      status === 403
        ? 'Your account is awaiting approval or no longer has access. Contact your manager.'
        : status === 401
          ? 'Your session has expired. Please sign in again.'
          : 'The request failed. Please try again.',
    )
  }
}

export class AccessDeniedEvent extends Event {
  static readonly eventName = 'neon-access-denied'

  constructor(
    readonly userId: string,
    readonly status: 401 | 403,
  ) {
    super(AccessDeniedEvent.eventName)
  }
}

const diagnosticWrites: Readonly<Partial<Record<string, DiagnosticsAction>>> = {
  '/api/admin/accounts/disable': 'account',
  '/api/admin/accounts/grant': 'account',
  '/api/admin/accounts/invite': 'account',
  '/api/admin/accounts/invite-message': 'account',
  '/api/admin/accounts/reset-password-generated': 'account',
  '/api/admin/accounts/verify-phone': 'account',
  '/api/admin/courses': 'course',
  '/api/admin/courses/settings': 'course',
  '/api/admin/courses/status': 'course',
  '/api/admin/enrollments/assignment': 'enrollment',
  '/api/admin/enrollments/decision': 'grade',
  '/api/admin/enrollments/grades': 'grade',
  '/api/admin/enrollments/handbook': 'enrollment',
  '/api/admin/enrollments/membership': 'enrollment',
  '/api/admin/students': 'student',
  '/api/admin/students/enroll': 'enrollment',
  '/api/me/invitation-profile': 'profile',
  '/api/student/update': 'student',
}

export async function request(
  path: string,
  userId: string,
  body?: unknown,
  reportDenial = true,
): Promise<unknown> {
  const generation = localGrant(userId)?.generation
  const deny = (status: 401 | 403) => {
    if (reportDenial && localGrant(userId)?.generation === generation)
      window.dispatchEvent(new AccessDeniedEvent(userId, status))
  }
  const session = await authClient.getSession({
    fetchOptions: { signal: AbortSignal.timeout(15000) },
  })
  const signedInUserId = session.data?.user.id
  if (signedInUserId && signedInUserId !== userId) {
    deny(403)
    throw new NeonRequestError(403)
  }
  if (!signedInUserId) {
    const status = session.error?.status
    if (status === 401 || status === 403 || !session.error) {
      const denial = status === 403 ? 403 : 401
      deny(denial)
      throw new NeonRequestError(denial)
    }
    throw new NeonRequestError(503)
  }
  const action = body !== undefined ? diagnosticWrites[path] : undefined
  let response: Response
  try {
    response = await accountFetch(path, userId, body)
  } catch (error) {
    if (action) recordDiagnosticAction(userId, action, 'failed')
    throw error
  }
  if (action) recordDiagnosticAction(userId, action, response.ok ? 'saved' : 'failed')
  if (!response.ok) {
    if (response.status === 401 || response.status === 403) deny(response.status)
    throw new NeonRequestError(response.status)
  }
  return response.json()
}
export type InvitationProfile = z.infer<
  typeof import('../shared/invitations').invitationProfileSchema
>

export async function getAccount(userId: string) {
  return z
    .object({ account: neonAccountSchema })
    .parse(await request('/api/me', userId, undefined, false)).account
}

export class InvalidNotebookSnapshotError extends Error {
  constructor() {
    super('The request failed. Please try again.')
  }
}

export async function getNotebook(userId: string) {
  try {
    return neonSnapshotSchema.parse(await request('/api/notebook', userId))
  } catch (cause) {
    if (cause instanceof NeonRequestError) throw cause
    if (cause instanceof z.ZodError || cause instanceof SyntaxError)
      throw new InvalidNotebookSnapshotError()
    throw new Error('The request failed. Please try again.')
  }
}
export async function postEvents(userId: string, events: LedgerEvent[]) {
  try {
    return neonSnapshotSchema.parse(
      await request('/api/events', userId, neonBatchSchema.parse({ events })),
    )
  } catch (cause) {
    if (cause instanceof NeonRequestError) throw cause
    throw new Error('The request failed. Please try again.')
  }
}
