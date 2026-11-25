import { AjvJsonSchemaValidator } from '@modelcontextprotocol/sdk/validation/ajv'
import { z } from 'zod'
import type { McpTool } from '../transport'

const reads = new Set([
  'get_account',
  'get_profile',
  'get_my_attendance',
  'list_courses',
  'find_students',
  'get_student_details',
  'get_course_attendance_view',
  'get_course_workflow',
  'get_notebook_history',
  'export_course_report',
])
const writes = new Set([
  'update_profile',
  'update_student',
  'sync_attendance',
  'sync_student',
  'append_events',
  'save_student',
  'create_student_in_course',
  'save_course',
  'set_course_status',
  'set_course_thresholds',
  'save_enrollment',
  'save_grades',
  'save_grade_decision',
  'set_handbook_received',
])
export type AgentTool = McpTool & { kind: 'read' | 'write' }

export function agentCatalog(discovered: readonly McpTool[]): AgentTool[] {
  return discovered.flatMap((tool): AgentTool[] => {
    if (reads.has(tool.name)) return [{ ...tool, kind: 'read' }]
    if (writes.has(tool.name)) return [{ ...tool, kind: 'write' }]
    return []
  })
}

const validator = new AjvJsonSchemaValidator()
const object = z.record(z.string(), z.unknown())
const secretKey = /password|secret|token|credential|authorization|phoneconfirmed/i

function checkWrite(value: unknown, depth = 0): void {
  if (depth > 12) throw new Error('Write too deeply nested')
  if (Array.isArray(value)) {
    if (value.length > 20) throw new Error('Write batch too large')
    for (const item of value) checkWrite(item, depth + 1)
  } else if (value && typeof value === 'object') {
    for (const [key, item] of Object.entries(value)) {
      if (secretKey.test(key)) throw new Error('Protected field')
      checkWrite(item, depth + 1)
    }
  } else if (typeof value === 'string' && value.length > 2000) {
    throw new Error('Write value too large')
  }
}

export function validateAgentCall(tool: AgentTool, args: unknown): Record<string, unknown> {
  if (!(tool.kind === 'read' ? reads : writes).has(tool.name)) throw new Error('Tool denied')
  const input = object.parse(args)
  if (JSON.stringify(input).length > 16000) throw new Error('Arguments too large')
  const result = validator.getValidator(tool.inputSchema)(input)
  if (!result.valid) throw new Error('Invalid tool arguments')
  if (tool.kind === 'write') checkWrite(input)
  if (tool.name === 'append_events') {
    const events = z
      .array(z.object({ payload: z.object({ kind: z.enum(['note', 'reversal']) }) }))
      .parse(input.events)
    if (!events.length) throw new Error('No events')
  }
  if (tool.name === 'export_course_report' && input.format === 'xlsx')
    throw new Error('Use the report menu for files')
  return input
}

// Only these structured fields enter the model; unknown fields fail closed.
const modelFields = new Set([
  'id',
  'studentId',
  'courseId',
  'enrollmentId',
  'eventId',
  'targetId',
  'teacherId',
  'name',
  'studentName',
  'courseName',
  'teacherName',
  'firstName',
  'lastName',
  'level',
  'active',
  'open',
  'status',
  'date',
  'kind',
  'view',
  'total',
  'offset',
  'limit',
  'present',
  'absent',
  'excused',
  'unmarked',
  'marked',
  'rate',
  'result',
  'calculatedResult',
  'effectiveResult',
  'theory',
  'practical',
  'handbookReceived',
  'missingTheory',
  'missingPractical',
  'missingAttendance',
  'lowTheory',
  'lowPractical',
  'course',
  'student',
  'students',
  'courses',
  'enrollment',
  'session',
  'sessionCounts',
  'attendance',
  'rows',
  'records',
  'event',
  'payload',
  'target',
  'undone',
  'readiness',
  'gradesClose',
  'missingScores',
  'followUp',
  'totals',
  'breakdown',
  'recentDates',
  'marks',
  'absences',
  'chart',
  'label',
  'evidence',
  'columns',
])
function sanitize(value: unknown, depth = 0, key = ''): unknown {
  if (depth > 8) return null
  if (Array.isArray(value))
    return value
      .slice(0, 30)
      .map((item) =>
        item && typeof item === 'object'
          ? sanitize(item, depth + 1, key)
          : ['marks', 'recentDates'].includes(key)
            ? sanitize(item, depth + 1, key)
            : null,
      )
  if (value && typeof value === 'object')
    return Object.fromEntries(
      Object.entries(value)
        .filter(([key]) => modelFields.has(key))
        .map(([key, item]) => [key, sanitize(item, depth + 1, key)]),
    )
  if (typeof value === 'string') return value.slice(0, 200)
  return typeof value === 'number' || typeof value === 'boolean' || value === null ? value : null
}

export function toolResultForModel(name: string, result: unknown): unknown {
  if (!reads.has(name) || !result || typeof result !== 'object') return null
  return sanitize(result)
}

const fieldNames: Record<string, [string, string]> = {
  studentId: ['Student', 'الطالب'],
  courseId: ['Course', 'الدورة'],
  enrollmentId: ['Enrollment', 'التسجيل'],
  teacherId: ['Teacher', 'المعلم'],
  targetId: ['Entry to correct', 'السجل المراد تصحيحه'],
  id: ['Record', 'السجل'],
  name: ['Name', 'الاسم'],
  firstName: ['First name', 'الاسم الأول'],
  lastName: ['Last name', 'الكنية'],
  studentName: ['Student', 'الطالب'],
  courseName: ['Course', 'الدورة'],
  teacherName: ['Teacher', 'المعلم'],
  level: ['Level', 'المستوى'],
  group: ['Level', 'المستوى'],
  active: ['Active', 'نشط'],
  open: ['Open', 'مفتوحة'],
  date: ['Date', 'التاريخ'],
  status: ['Status', 'الحالة'],
  present: ['Present', 'حاضر'],
  absent: ['Absent', 'غائب'],
  excused: ['Excused', 'معذور'],
  unmarked: ['Unmarked', 'غير مسجل'],
  marked: ['Marked', 'مسجل'],
  total: ['Total', 'المجموع'],
  theory: ['Theory', 'النظري'],
  practical: ['Practical', 'العملي'],
  result: ['Result', 'النتيجة'],
  calculatedResult: ['Calculated result', 'النتيجة المحسوبة'],
  effectiveResult: ['Final result', 'النتيجة النهائية'],
  attendanceMinimum: ['Minimum attendance', 'الحد الأدنى للحضور'],
  practicalMinimum: ['Minimum practical', 'الحد الأدنى للعملي'],
  theoryMinimum: ['Minimum theory', 'الحد الأدنى للنظري'],
  handbookReceived: ['Handbook received', 'استلم الدليل'],
  text: ['Note', 'الملاحظة'],
  note: ['Reason', 'السبب'],
  notes: ['Notes', 'الملاحظات'],
  reason: ['Reason', 'السبب'],
  tags: ['Tags', 'الوسوم'],
  kind: ['Type', 'النوع'],
  attendance: ['Attendance', 'الحضور'],
  reversal: ['Correction', 'تصحيح'],
  whatsapp: ['WhatsApp', 'واتساب'],
  phone: ['Phone', 'الهاتف'],
  address: ['Address', 'العنوان'],
}
const writeTitles: Record<string, [string, string]> = {
  update_profile: ['Update profile', 'تعديل الملف الشخصي'],
  update_student: ['Update student', 'تعديل الطالب'],
  sync_attendance: ['Save attendance', 'حفظ الحضور'],
  sync_student: ['Save student change', 'حفظ تعديل الطالب'],
  append_events: ['Save notebook entry', 'حفظ سجل المتابعة'],
  save_student: ['Save student', 'حفظ الطالب'],
  create_student_in_course: ['Create student and enrollment', 'إضافة طالب وتسجيله'],
  save_course: ['Save course', 'حفظ الدورة'],
  set_course_status: ['Change course status', 'تغيير حالة الدورة'],
  set_course_thresholds: ['Change course thresholds', 'تغيير حدود الدورة'],
  save_enrollment: ['Save enrollment', 'حفظ التسجيل'],
  save_grades: ['Save grades', 'حفظ الدرجات'],
  save_grade_decision: ['Save result decision', 'حفظ قرار النتيجة'],
  set_handbook_received: ['Save handbook receipt', 'حفظ استلام الدليل'],
}
const label = (key: string, lang: 'en' | 'ar') =>
  fieldNames[key]?.[lang === 'ar' ? 1 : 0] ?? key.replace(/([A-Z])/g, ' $1')
const display = (value: unknown, lang: 'en' | 'ar') => {
  if (value === null) return lang === 'ar' ? 'فارغ' : 'Blank'
  if (typeof value === 'boolean')
    return value ? (lang === 'ar' ? 'نعم' : 'Yes') : lang === 'ar' ? 'لا' : 'No'
  if (typeof value === 'string' && fieldNames[value]) return label(value, lang)
  return String(value)
}

export function toolResultForChat(name: string, result: unknown, lang: 'en' | 'ar'): string {
  if (!reads.has(name))
    return lang === 'ar' ? 'استخدم القائمة لعرض النتيجة.' : 'Open the menu to view the result.'
  const lines: string[] = []
  function visit(value: unknown, key = '', depth = 0): void {
    if (depth > 8 || lines.length >= 60) return
    if (Array.isArray(value)) {
      for (const item of value) visit(item, key, depth + 1)
    } else if (value && typeof value === 'object') {
      for (const [field, item] of Object.entries(value)) {
        if (/Id$/.test(field) || field === 'id') continue
        visit(item, field, depth + 1)
      }
    } else if (key && value !== null) lines.push(`${label(key, lang)}: ${display(value, lang)}`)
  }
  visit(toolResultForModel(name, result))
  if (!lines.length) return lang === 'ar' ? 'لا توجد نتائج للعرض.' : 'No results to display.'
  const text = lines.join('\n')
  return text.length <= 3500
    ? text
    : `${text.slice(0, 3400)}\n${lang === 'ar' ? 'افتح القائمة لعرض المزيد.' : 'Open the menu for more.'}`
}

export function previewAgentWrite(
  tool: AgentTool,
  args: unknown,
  lang: 'en' | 'ar',
  labels: Readonly<Record<string, string>>,
): string {
  if (tool.kind !== 'write') throw new Error('Not a write')
  const input = validateAgentCall(tool, args)
  const lines: string[] = []
  function visit(value: unknown, key: string, parent: string): void {
    if (
      key === 'expected' ||
      key === 'expectedEventId' ||
      key === 'expectedRevision' ||
      key === 'clientTime' ||
      key === 'requestId'
    )
      return
    if (Array.isArray(value)) {
      value.forEach((item, index) => {
        if (value.length > 1) lines.push(`${lang === 'ar' ? 'عنصر' : 'Item'} ${index + 1}`)
        visit(item, key, parent)
      })
    } else if (value && typeof value === 'object') {
      for (const [field, item] of Object.entries(value)) visit(item, field, key)
    } else if (
      typeof value === 'string' &&
      (/^(studentId|courseId|enrollmentId|teacherId|targetId)$/.test(key) ||
        (key === 'id' && parent === 'input'))
    ) {
      const resolved = labels[value]
      if (!resolved || resolved.length > 200 || /[\r\n]/.test(resolved))
        throw new Error('Unresolved target')
      lines.push(`${label(key, lang)}: ${resolved}`)
    } else if (key !== 'id') lines.push(`${label(key, lang)}: ${display(value, lang)}`)
  }
  visit(input, '', '')
  const title = writeTitles[tool.name]
  if (!title || !lines.length) throw new Error('Write cannot be previewed')
  const effect =
    tool.name === 'set_course_status'
      ? lang === 'ar'
        ? '\nقد يؤدي إغلاق الدورة إلى ترقية مستويات الطلاب المؤهلين.'
        : '\nClosing the course may advance eligible students’ levels.'
      : ''
  const text = `${lang === 'ar' ? 'تأكيد' : 'Confirm'}: ${title[lang === 'ar' ? 1 : 0]}\n${lines.join('\n')}${effect}`
  if (text.length > 3500) throw new Error('Write preview too large')
  return text
}
