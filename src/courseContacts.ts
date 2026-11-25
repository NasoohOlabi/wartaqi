import { internationalPhone } from './phoneFormat'

export function courseContacts(
  students: {
    name: string
    firstName?: string | null
    lastName?: string | null
    whatsapp?: string | null
  }[],
) {
  return students.flatMap((student) => {
    const phone = internationalPhone(student.whatsapp ?? '')
    return phone
      ? [{ name: student.name, firstName: student.firstName, lastName: student.lastName, phone }]
      : []
  })
}

function fold(line: string) {
  const encoder = new TextEncoder()
  const lines = ['']
  for (const char of line) {
    const last = lines.length - 1
    if (encoder.encode(lines[last] + char).length > 75) lines.push(' ')
    lines[lines.length - 1] += char
  }
  return lines.join('\r\n')
}

export function downloadContacts(course: string, contacts: ReturnType<typeof courseContacts>) {
  const escape = (value: string) =>
    value
      .replace(/\\/g, '\\\\')
      .replace(/\r\n|\r|\n/g, '\\n')
      .replace(/[,;]/g, '\\$&')
  const cards = contacts.flatMap(({ name, firstName, lastName, phone }) => [
    'BEGIN:VCARD',
    'VERSION:3.0',
    `FN:${escape(name)}`,
    `N:${escape(lastName || '')};${escape(firstName || (!lastName ? name : ''))};;;`,
    `TEL;TYPE=CELL:${phone}`,
    'END:VCARD',
  ])
  const blob = new Blob([cards.map(fold).join('\r\n') + '\r\n'], {
    type: 'text/vcard;charset=utf-8',
  })
  const url = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = url
  link.download = `${
    course
      .replace(/[\\/:*?"<>|\p{Cc}]/gu, '_')
      .trim()
      .slice(0, 100)
      .replace(/[. ]+$/, '') || 'course'
  }-contacts.vcf`
  link.click()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}
