import { z } from 'zod'
import { eventSchema } from './ledger'
import { studentProfileVersionSchema } from './neon'

const whatsapp = z
  .string()
  .trim()
  .max(40)
  .nullable()
  .transform((value) => value || null)
export const editableProfileSchema = studentProfileVersionSchema
  .omit({ group: true })
  .extend({ whatsapp })
export const profileFieldSchema = editableProfileSchema.keyof()
export const profileFields = profileFieldSchema.options
export type ProfileField = z.infer<typeof profileFieldSchema>
export type EditableProfile = z.infer<typeof editableProfileSchema>
const profilePatchSchema = editableProfileSchema
  .extend({
    firstName: z.string().trim().min(1).max(200),
    lastName: z.string().trim().max(200),
    whatsapp,
    location: z
      .string()
      .trim()
      .max(200)
      .nullable()
      .transform((value) => value || null),
    birthYear: z.number().int().min(1900).max(new Date().getFullYear()).nullable(),
  })
  .partial()
export const offlineProfileOperationSchema = z
  .strictObject({
    kind: z.literal('profile-v1'),
    editTime: z.iso.datetime().optional(),
    id: z.uuid(),
    studentId: z.uuid(),
    clientTime: z.iso.datetime(),
    expected: editableProfileSchema.partial(),
    changes: profilePatchSchema,
  })
  .refine(
    ({ expected, changes }) =>
      profileFields.some((field) => changes[field] !== undefined) &&
      profileFields.every(
        (field) => (changes[field] === undefined) === (expected[field] === undefined),
      ),
  )
export type OfflineProfileOperation = z.infer<typeof offlineProfileOperationSchema>

export function profileConflicts(current: EditableProfile, operation: OfflineProfileOperation) {
  return profileFields.filter(
    (field) =>
      operation.changes[field] !== undefined &&
      current[field] !== operation.expected[field] &&
      current[field] !== operation.changes[field],
  )
}

export const offlineStudentOperationSchema = z.discriminatedUnion('kind', [
  z
    .object({
      kind: z.literal('note'),
      event: eventSchema.extend({
        payload: z.object({
          kind: z.literal('note'),
          studentId: z.uuid(),
          date: z.iso.date(),
          tags: z.array(z.string().trim().min(1).max(60)).min(1).max(20),
          text: z.string().trim().min(1).max(10000),
        }),
      }),
    })
    .strict(),
  offlineProfileOperationSchema,
  z
    .object({
      kind: z.literal('contact'),
      id: z.uuid(),
      studentId: z.uuid(),
      clientTime: z.iso.datetime(),
      expectedWhatsapp: whatsapp,
      whatsapp,
    })
    .strict(),
])
export const offlineStudentSyncSchema = z
  .object({
    accountId: z.string().min(1),
    operation: offlineStudentOperationSchema,
  })
  .strict()
export const offlineStudentOutcomeSchema = z.object({
  id: z.uuid(),
  kind: z.enum(['saved', 'conflict', 'forbidden', 'id_conflict']),
  currentWhatsapp: whatsapp,
  profile: editableProfileSchema.optional(),
  profileConflicts: z.array(profileFieldSchema).optional(),
})
