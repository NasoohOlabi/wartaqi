import { createHash } from 'node:crypto'
import type { PoolClient } from 'pg'
import { z } from 'zod'
import {
  applyGradeChanges,
  gradeChangesConflict,
  gradeValuesSchema,
  type GradeOperation,
  type GradeOutcome,
} from '../shared/gradeSync'

export async function saveGradeEdit(
  db: PoolClient,
  subject: string,
  grant: GradeOperation['grant'],
  operation: GradeOperation,
  submittedGrant = operation.grant,
): Promise<GradeOutcome> {
  if (
    grant.id !== operation.grant.id ||
    grant.id !== submittedGrant.id ||
    grant.expiresAt !== submittedGrant.expiresAt
  )
    return { status: 'blocked' }
  const fingerprint = createHash('sha256')
    .update(JSON.stringify([subject, operation]))
    .digest('hex')
  // The surrounding authorized transaction commits the grade and its receipt together.
  await db.query("SELECT pg_advisory_xact_lock(hashtextextended('grade-sync:' || $1, 0))", [
    operation.requestId,
  ])
  const receipt = await db.query(
    'SELECT fingerprint FROM grade_sync_receipts WHERE request_id = $1',
    [operation.requestId],
  )
  const existing = z.string().optional().parse(receipt.rows[0]?.fingerprint)
  if (existing !== undefined) return { status: existing === fingerprint ? 'accepted' : 'blocked' }
  const row = await db.query(
    'SELECT practical::float8 AS practical, theory::float8 AS theory, notes FROM course_enrollments WHERE id = $1 FOR UPDATE',
    [operation.id],
  )
  if (!row.rowCount) return { status: 'blocked' }
  const current = gradeValuesSchema.parse(row.rows[0])
  if (gradeChangesConflict(current, operation.fields)) return { status: 'conflict', current }
  const next = applyGradeChanges(current, operation.fields)
  if (
    next.practical !== current.practical ||
    next.theory !== current.theory ||
    next.notes !== current.notes
  )
    await db.query(
      'UPDATE course_enrollments SET practical = $2, theory = $3, notes = $4 WHERE id = $1',
      [operation.id, next.practical, next.theory, next.notes],
    )
  await db.query('INSERT INTO grade_sync_receipts (request_id, fingerprint) VALUES ($1, $2)', [
    operation.requestId,
    fingerprint,
  ])
  return { status: 'accepted' }
}
