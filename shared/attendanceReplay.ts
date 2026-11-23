import { z } from 'zod'
import { statusSchema } from './ledger'

export const attendanceReplayMaxRows = 150

const attendanceReplayRowSchema = z.strictObject({
  id: z.uuid(),
  name: z.string().min(1).max(200),
  level: z.string().max(200),
  teacherName: z.string().max(200),
  saved: statusSchema.nullable(),
  draft: statusSchema.nullable(),
})

export const attendanceReplaySnapshotSchema = z
  .strictObject({
    courseName: z.string().max(200),
    date: z.iso.date(),
    rows: z.array(attendanceReplayRowSchema).max(attendanceReplayMaxRows),
    totalRows: z.number().int().nonnegative(),
    pendingCount: z.number().int().nonnegative(),
    unmarkedCount: z.number().int().nonnegative(),
    unmarkedOnly: z.boolean(),
    canCorrect: z.boolean(),
    dialog: z
      .strictObject({ kind: z.enum(['note', 'correction']), name: z.string().min(1).max(200) })
      .nullable(),
  })
  .refine((snapshot) => snapshot.totalRows >= snapshot.rows.length)

export const attendanceReplayActionSchema = z.discriminatedUnion('kind', [
  z.strictObject({ kind: z.literal('mark'), studentId: z.uuid(), status: statusSchema }),
  z.strictObject({ kind: z.enum(['student', 'note', 'correction']), studentId: z.uuid() }),
  z.strictObject({ kind: z.enum(['save', 'bulk', 'unmarked', 'date']) }),
])

export type AttendanceReplayRow = z.infer<typeof attendanceReplayRowSchema>
export type AttendanceReplaySnapshot = z.infer<typeof attendanceReplaySnapshotSchema>
export type AttendanceReplayAction = z.infer<typeof attendanceReplayActionSchema>
