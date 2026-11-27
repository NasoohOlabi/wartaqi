import { z } from 'zod'
import { enrollmentSchema, neonTeacherSchema } from './neon'

export const gradePath = '/api/admin/enrollments/grades'
export const gradeValuesSchema = enrollmentSchema
  .pick({
    practical: true,
    theory: true,
    notes: true,
  })
  .extend({ notes: z.string() })
  .strict()
export type GradeValues = z.infer<typeof gradeValuesSchema>

const scoreChange = z.strictObject({
  previous: gradeValuesSchema.shape.practical,
  next: gradeValuesSchema.shape.practical,
})
const noteChange = z.strictObject({
  previous: gradeValuesSchema.shape.notes,
  next: gradeValuesSchema.shape.notes,
})
export const gradeOperationSchema = z.strictObject({
  kind: z.literal('grade-edit'),
  requestId: z.uuid(),
  id: z.uuid(),
  grant: neonTeacherSchema.pick({ id: true, expiresAt: true }).strict(),
  fields: z
    .strictObject({
      practical: scoreChange.optional(),
      theory: scoreChange.optional(),
      notes: noteChange.optional(),
    })
    .refine(({ practical, theory, notes }) => Boolean(practical || theory || notes)),
})
export type GradeOperation = z.infer<typeof gradeOperationSchema>
export const gradeRecoverySchema = z.strictObject({
  kind: z.literal('recover-grade-edit'),
  operation: gradeOperationSchema,
  grant: gradeOperationSchema.shape.grant,
})
export const gradeReviewSchema = z.discriminatedUnion('status', [
  z.strictObject({ status: z.literal('conflict'), current: gradeValuesSchema }),
  z.strictObject({ status: z.literal('blocked') }),
])
export const gradeOutcomeSchema = z.discriminatedUnion('status', [
  z.strictObject({ status: z.literal('accepted') }),
  ...gradeReviewSchema.options,
])
export type GradeOutcome = z.infer<typeof gradeOutcomeSchema>

export function gradeChanges(previous: GradeValues, next: GradeValues): GradeOperation['fields'] {
  return {
    ...(previous.practical !== next.practical
      ? { practical: { previous: previous.practical, next: next.practical } }
      : {}),
    ...(previous.theory !== next.theory
      ? { theory: { previous: previous.theory, next: next.theory } }
      : {}),
    ...(previous.notes !== next.notes
      ? { notes: { previous: previous.notes, next: next.notes } }
      : {}),
  }
}

export function applyGradeChanges(
  current: GradeValues,
  fields: GradeOperation['fields'],
): GradeValues {
  return {
    practical: fields.practical ? fields.practical.next : current.practical,
    theory: fields.theory ? fields.theory.next : current.theory,
    notes: fields.notes ? fields.notes.next : current.notes,
  }
}

export function gradeChangesConflict(current: GradeValues, fields: GradeOperation['fields']) {
  const diverged = <T>(value: T, change?: { previous: T; next: T }) =>
    change !== undefined && value !== change.previous && value !== change.next
  return (
    diverged(current.practical, fields.practical) ||
    diverged(current.theory, fields.theory) ||
    diverged(current.notes, fields.notes)
  )
}
