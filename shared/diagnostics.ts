import { z } from 'zod'

export const diagnosticsScreenSchema = z.enum([
  'dashboard',
  'attendance',
  'roster',
  'summary',
  'student',
  'student-portal',
  'courses',
  'course-create',
  'course-edit',
  'course-students',
  'course-grades',
  'settings-accounts',
  'settings-students',
  'add-student',
  'profile',
  'history',
  'changes',
  'invitation',
])
export const diagnosticsActionSchema = z.enum([
  'attendance',
  'note',
  'correction',
  'student',
  'account',
  'course',
  'enrollment',
  'grade',
  'profile',
  'sync',
  'filter',
  'export',
  'course-switch',
  'sign-out',
  'account-switch',
])
export const diagnosticsRoleSchema = z.enum(['administrator', 'teacher', 'student'])
export const diagnosticsOutcomeSchema = z.enum(['activated', 'saved', 'queued', 'failed', 'synced'])
export const diagnosticsMetricSchema = z.enum(['LCP', 'INP', 'CLS'])

const eventFields = {
  id: z.uuid(),
  journeyId: z.uuid(),
  step: z.number().int().min(1).max(1_000_000),
  screen: diagnosticsScreenSchema,
}

export const diagnosticsEventSchema = z.discriminatedUnion('kind', [
  z.strictObject({ ...eventFields, kind: z.literal('screen') }),
  z.strictObject({
    ...eventFields,
    kind: z.literal('action'),
    action: diagnosticsActionSchema,
    outcome: diagnosticsOutcomeSchema,
  }),
  z.strictObject({
    ...eventFields,
    kind: z.literal('vital'),
    metric: diagnosticsMetricSchema,
    value: z.number().min(0).max(600_000),
  }),
  z.strictObject({
    ...eventFields,
    kind: z.literal('error'),
    code: z.enum(['runtime', 'promise']),
  }),
])

export const diagnosticsBatchSchema = z.strictObject({
  events: z.array(diagnosticsEventSchema).min(1).max(30),
})

export const diagnosticsJourneysSchema = z.object({
  items: z.array(
    z.object({
      journeyId: z.uuid(),
      actorId: z.string(),
      actorName: z.string().nullable(),
      role: diagnosticsRoleSchema,
      lastSeenAt: z.iso.datetime(),
      eventCount: z.number().int().positive(),
    }),
  ),
  nextCursor: z.string().nullable(),
})

export const diagnosticsTimelineSchema = z.object({
  items: z.array(z.object({ event: diagnosticsEventSchema, receivedAt: z.iso.datetime() })),
  nextCursor: z.string().nullable(),
})

export type DiagnosticsScreen = z.infer<typeof diagnosticsScreenSchema>
export type DiagnosticsAction = z.infer<typeof diagnosticsActionSchema>
export type DiagnosticsEvent = z.infer<typeof diagnosticsEventSchema>
export type DiagnosticsRole = z.infer<typeof diagnosticsRoleSchema>
export type DiagnosticsOutcome = z.infer<typeof diagnosticsOutcomeSchema>
