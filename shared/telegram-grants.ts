import { createHash } from 'node:crypto'
import type { PoolClient } from 'pg'

export const telegramGrantPrefix = 'wartaq:telegram:'

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
