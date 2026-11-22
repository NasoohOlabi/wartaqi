import { createHash } from 'node:crypto'
import type { PoolClient } from 'pg'
import { z } from 'zod'
import { phoneSchema } from './invitations'

export const telegramGrantPrefix = 'wartaq:telegram:'
export const telegramRosterSubjectPrefix = 'roster:'

export const rosterPhoneSchema = z
  .string()
  .trim()
  .max(40)
  .transform((value) => {
    const compact = value.replace(/[ ()-]/g, '')
    return /^[1-9][0-9]{10,14}$/.test(compact) ? `+${compact}` : value
  })
  .pipe(phoneSchema)

export function telegramGrantBinding(accountId: string, phone: string, studentId: string) {
  return createHash('sha256')
    .update(JSON.stringify([accountId, phone, studentId]))
    .digest('hex')
}

export function telegramTeacherGrantBinding(accountId: string, phone: string, teacherId: string) {
  return createHash('sha256')
    .update(JSON.stringify(['teacher', accountId, phone, teacherId]))
    .digest('hex')
}

export async function revokeTelegramGrants(db: PoolClient, authUserId: string): Promise<void> {
  await db.query(
    'DELETE FROM neon_auth.verification WHERE starts_with(identifier, $1) AND right(identifier, length($2)) = $2',
    [telegramGrantPrefix, `:${authUserId}`],
  )
}

export function rosterGrantBinding(studentId: string, phone: string) {
  return createHash('sha256')
    .update(JSON.stringify(['roster', studentId, phone]))
    .digest('hex')
}

export async function rosterStudentForPhone(db: PoolClient, phone: string): Promise<string | null> {
  const rows = z
    .array(z.object({ id: z.uuid(), whatsapp: z.string().nullable(), allowed: z.boolean() }))
    .parse(
      (
        await db.query(`SELECT s.id, s.whatsapp,
          (a.auth_user_id IS NULL OR (a.active AND (a.expires_at IS NULL OR a.expires_at > now()))) AS allowed
          FROM students s LEFT JOIN app_accounts a ON a.student_id = s.id
          WHERE s.active AND s.whatsapp IS NOT NULL`)
      ).rows,
    )
  const matches = rows.filter((row) => {
    const parsed = rosterPhoneSchema.safeParse(row.whatsapp)
    return parsed.success && parsed.data === phone
  })
  return matches.length === 1 && matches[0].allowed ? matches[0].id : null
}
