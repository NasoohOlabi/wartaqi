import type { Enrollment, NeonRosterStudent } from '../shared/neon'
import { formatLevel } from './levelFormat'

export function rosterRows(students: NeonRosterStudent[], enrollments: Enrollment[], ar: boolean) {
  return [
    ar
      ? ['الاسم الأول', 'اسم العائلة', 'رقم الهاتف', 'المستوى']
      : ['First name', 'Last name', 'Phone', 'Level'],
    ...students.map((student) => [
      student.firstName ?? student.name,
      student.lastName ?? '',
      student.whatsapp ?? '',
      formatLevel(enrollments.find((entry) => entry.studentId === student.studentId)?.level ?? ''),
    ]),
  ]
}

function filename(course: string, extension: string) {
  const safe =
    course
      .replace(/[\\/:*?"<>|\p{Cc}]/gu, '_')
      .trim()
      .slice(0, 100)
      .replace(/[. ]+$/, '') || 'course'
  return `${safe}-students.${extension}`
}

export function downloadRosterCsv(course: string, rows: string[][]) {
  const csv = rows
    .map((row) =>
      row
        .map((value) => {
          const literal = /^\s*[=+\-@]/u.test(value) ? `'${value}` : value
          return `"${literal.replace(/"/g, '""')}"`
        })
        .join(','),
    )
    .join('\r\n')
  download(filename(course, 'csv'), new Blob(['\uFEFF', csv], { type: 'text/csv;charset=utf-8' }))
}

export async function downloadRosterExcel(course: string, rows: string[][]) {
  const { default: writeExcelFile } = await import('write-excel-file/universal')
  const workbook = await writeExcelFile(rows, {
    columns: [{ width: 22 }, { width: 27 }, { width: 22 }, { width: 22 }],
  }).toBlob()
  download(filename(course, 'xlsx'), workbook)
}

function download(name: string, blob: Blob) {
  const url = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = url
  link.download = name
  link.click()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}
