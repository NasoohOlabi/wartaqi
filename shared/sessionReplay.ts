import { z } from 'zod'
import { attendanceReplayActionSchema, attendanceReplaySnapshotSchema } from './attendanceReplay'
import { diagnosticsScreenSchema } from './diagnostics'

export const replayPath = '/api/diagnostics/replay'
export const replayMaxBodyBytes = 48 * 1024
export const replayMaxFramesPerBatch = 3
export const replayMaxBoxes = 120
export const replayMaxFrames = 600
export const replayMaxDimension = 8192

export const replayLabels = {
  dashboard: ['Dashboard', 'الرئيسية'],
  attendance: ['Attendance', 'الحضور'],
  students: ['Students', 'الطلاب'],
  courses: ['Courses', 'الدورات'],
  settings: ['Settings', 'الإعدادات'],
  profile: ['Profile', 'الملف الشخصي'],
  'take-attendance': ['Take attendance', 'تسجيل الحضور'],
  'add-student': ['Add student', 'إضافة طالب'],
  'add-note': ['Add note', 'إضافة ملاحظة'],
  'save-note': ['Save note', 'حفظ الملاحظة'],
  'save-changes': ['Save changes', 'حفظ التغييرات'],
  'choose-course': ['Choose course', 'اختر دورة'],
  'find-student': ['Find a student', 'ابحث عن طالب'],
  'attendance-and-notes': ['Attendance & notes', 'الحضور والملاحظات'],
  'sign-out': ['Sign out', 'تسجيل الخروج'],
  back: ['Back', 'رجوع'],
  cancel: ['Cancel', 'إلغاء'],
  refresh: ['Refresh', 'تحديث'],
  filter: ['Filter', 'التصفية'],
  sort: ['Sort', 'الفرز'],
  present: ['Present', 'حاضر'],
  absent: ['Absent', 'غائب'],
  excused: ['Excused', 'معذور'],
} satisfies Record<string, [english: string, arabic: string]>

const dimension = z.number().int().min(1).max(replayMaxDimension)
const coordinate = z.number().int().min(0).max(replayMaxDimension)
export const replayLabelSchema = z.custom<ReplayLabel>(
  (value) => typeof value === 'string' && Object.hasOwn(replayLabels, value),
)
const boxSchema = z.strictObject({
  x: coordinate,
  y: coordinate,
  w: dimension,
  h: dimension,
  kind: z.enum(['surface', 'line', 'text', 'control', 'field', 'image']),
  fill: z
    .string()
    .regex(/^#[0-9a-f]{6}$/i)
    .optional(),
  label: replayLabelSchema.optional(),
})

export const replayFrameNumberSchema = z.number().int().min(1).max(replayMaxFrames)
const frameFields = {
  step: replayFrameNumberSchema,
  timeMs: z.number().int().min(0).max(86_400_000),
  screen: diagnosticsScreenSchema,
  width: dimension,
  height: dimension,
  theme: z.enum(['light', 'dark']),
  language: z.enum(['en', 'ar']),
  boxes: z.array(boxSchema).max(replayMaxBoxes),
  attendance: attendanceReplaySnapshotSchema.optional(),
}
export const replayFrameSchema = z
  .discriminatedUnion('kind', [
    z.strictObject({ ...frameFields, kind: z.literal('view') }),
    z.strictObject({
      ...frameFields,
      kind: z.literal('click'),
      click: z.strictObject({ x: coordinate, y: coordinate }),
      attendanceAction: attendanceReplayActionSchema.optional(),
    }),
  ])
  .refine(
    (frame) =>
      frame.boxes.every((box) => box.x + box.w <= frame.width && box.y + box.h <= frame.height) &&
      (!frame.attendance || frame.screen === 'attendance') &&
      (frame.kind === 'view' ||
        (frame.click.x <= frame.width &&
          frame.click.y <= frame.height &&
          (!frame.attendanceAction || frame.attendance !== undefined))),
  )

export const replayBatchSchema = z.strictObject({
  journeyId: z.uuid(),
  frames: z.array(replayFrameSchema).min(1).max(replayMaxFramesPerBatch),
})
export const replayPageSchema = z.strictObject({
  items: z.array(replayFrameSchema).max(20),
  nextAfter: replayFrameNumberSchema.nullable(),
})

export type ReplayFrame = z.infer<typeof replayFrameSchema>
export type ReplayBox = z.infer<typeof boxSchema>
export type ReplayLabel = keyof typeof replayLabels
