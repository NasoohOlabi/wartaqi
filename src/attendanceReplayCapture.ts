import type { AttendanceReplaySnapshot } from '../shared/attendanceReplay'

let current: { userId: string; snapshot: AttendanceReplaySnapshot } | undefined

export function readAttendanceReplay(userId: string): AttendanceReplaySnapshot | undefined {
  return current?.userId === userId ? current.snapshot : undefined
}

export function registerAttendanceReplay(userId: string, snapshot: AttendanceReplaySnapshot) {
  const entry = { userId, snapshot }
  current = entry
  return () => {
    if (current === entry) current = undefined
  }
}
