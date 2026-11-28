import { z } from 'zod'
import { statuses, type Attendance } from '../shared/ledger'

const index = z.int().nonnegative()

export const notebookSummarySchema = z
  .object({
    dates: z.array(z.iso.date()),
    students: z.array(z.uuid()),
    enrollments: z.array(z.uuid()),
    marks: z.array(z.tuple([index, index.nullable(), z.array(index)])),
    notes: z.array(z.tuple([index, index])),
  })
  .transform(({ dates, students, enrollments, marks, notes }, context) => {
    const attendance: { value: Attendance }[] = []
    const noteCounts = new Map<string, number>()
    const invalid = () => {
      context.addIssue({ code: 'custom', message: 'Invalid notebook summary index' })
      return z.NEVER
    }
    for (const [studentIndex, enrollmentIndex, entries] of marks) {
      const studentId = students[studentIndex]
      const enrollmentId = enrollmentIndex === null ? undefined : enrollments[enrollmentIndex]
      if (!studentId || (enrollmentIndex !== null && !enrollmentId)) return invalid()
      for (const entry of entries) {
        const date = dates[Math.floor(entry / statuses.length)]
        if (!date) return invalid()
        attendance.push({
          value: {
            kind: 'attendance',
            studentId,
            enrollmentId,
            date,
            status: statuses[entry % statuses.length],
          },
        })
      }
    }
    for (const [studentIndex, count] of notes) {
      const studentId = students[studentIndex]
      if (!studentId) return invalid()
      noteCounts.set(studentId, count)
    }
    return { attendance, noteCounts }
  })

export type NotebookSummary = z.infer<typeof notebookSummarySchema>
