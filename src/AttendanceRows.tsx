import { DirectionProvider } from '@base-ui/react/direction-provider'
import { Toggle } from '@base-ui/react/toggle'
import { ToggleGroup } from '@base-ui/react/toggle-group'
import { Check, X, CircleMinus, Pencil, StickyNote } from 'lucide-react'
import { useLayoutEffect, useRef, type ReactNode } from 'react'
import { statuses } from '../shared/ledger'
import type { AttendanceReplayAction, AttendanceReplayRow } from '../shared/attendanceReplay'
import { useLanguage } from './useLanguage'

export const attendanceExitDuration = 260

function AttendanceRow({
  id,
  exiting,
  highlighted,
  children,
}: {
  id: string
  exiting: boolean
  highlighted: boolean
  children: ReactNode
}) {
  const element = useRef<HTMLTableRowElement>(null)
  useLayoutEffect(() => {
    const row = element.current
    if (!exiting || !row) return
    const focused = document.activeElement
    if (row.contains(focused)) {
      const neighbor = [row.nextElementSibling, row.previousElementSibling].find(
        (sibling) => sibling && !sibling.hasAttribute('data-attendance-exiting'),
      )
      const nextFocus =
        neighbor?.querySelector<HTMLButtonElement>(
          'button[data-replay-action="mark"][tabindex="0"]',
        ) ?? row.closest('section')?.querySelector<HTMLButtonElement>('.unmarked-toggle')
      nextFocus?.focus({ preventScroll: true })
    }
    row.inert = true
    let slide: Animation | undefined
    if (matchMedia('(max-width: 64rem) and (prefers-reduced-motion: no-preference)').matches) {
      // Compact records use a grid with intrinsic row heights. Measure the card so long names
      // and translated labels keep their normal sizing until the exit starts.
      slide = row.animate(
        [
          { maxHeight: `${row.getBoundingClientRect().height}px`, overflow: 'clip' },
          { maxHeight: '0px', overflow: 'clip', paddingBlock: 0, borderWidth: 0 },
        ],
        { duration: attendanceExitDuration, easing: 'ease', fill: 'forwards' },
      )
    }
    return () => {
      slide?.cancel()
      row.inert = false
    }
  }, [exiting])
  return (
    <tr
      ref={element}
      data-student-id={id}
      data-attendance-exiting={exiting ? 'true' : undefined}
      data-replay-highlight={highlighted ? 'true' : undefined}
    >
      {children}
    </tr>
  )
}

export function AttendanceRows({
  rows,
  header,
  canCorrect,
  disabled,
  onOpenStudent,
  onNote,
  onMark,
  onCorrect,
  highlight,
  exitingRows,
}: {
  rows: AttendanceReplayRow[]
  header?: ReactNode
  canCorrect: boolean
  disabled?: boolean
  onOpenStudent?: (id: string, element: HTMLElement) => void
  onNote?: (id: string) => void
  onMark?: (id: string, value: string[]) => void
  onCorrect?: (id: string) => void
  highlight?: AttendanceReplayAction
  exitingRows?: ReadonlySet<string>
}) {
  const { t, language } = useLanguage()
  return (
    <DirectionProvider direction={language === 'ar' ? 'rtl' : 'ltr'}>
      <div
        className="role-table-scroll"
        role="region"
        aria-label={language === 'ar' ? 'قائمة الحضور' : 'Attendance list'}
        tabIndex={0}
      >
        <table className="mobile-record-table role-table roster-table">
          <colgroup>
            <col />
            <col className="roster-level-column" />
            <col />
            <col
              className={
                canCorrect ? 'roster-attendance-correction-column' : 'roster-attendance-column'
              }
            />
          </colgroup>
          {header}
          <tbody>
            {rows.map((row) => {
              const note = (
                <button
                  type="button"
                  className="attendance-note-button"
                  data-replay-action="note"
                  aria-label={`${t('Add note for')} ${row.name}`}
                  title={t('Add note')}
                  aria-haspopup="dialog"
                  disabled={!onNote}
                  onClick={() => onNote?.(row.id)}
                >
                  <StickyNote size={18} aria-hidden="true" />
                </button>
              )
              return (
                <AttendanceRow
                  key={row.id}
                  id={row.id}
                  exiting={exitingRows?.has(row.id) ?? false}
                  highlighted={
                    !!highlight && 'studentId' in highlight && highlight.studentId === row.id
                  }
                >
                  <td className="record-title" data-label={t('Student')}>
                    <div className="attendance-cell">
                      <div>
                        <button
                          type="button"
                          className="student-name-link"
                          data-replay-action="student"
                          disabled={!onOpenStudent}
                          onClick={(event) => onOpenStudent?.(row.id, event.currentTarget)}
                        >
                          <bdi>{row.name}</bdi>
                        </button>
                      </div>
                    </div>
                  </td>
                  <td data-label={language === 'ar' ? 'المستوى' : 'Level'}>
                    <div className="attendance-cell">
                      <div>
                        <bdi>{row.level || '—'}</bdi>
                      </div>
                    </div>
                  </td>
                  <td data-label={language === 'ar' ? 'المعلم' : 'Teacher'}>
                    <div className="attendance-cell">
                      <div>
                        <bdi>{row.teacherName || '—'}</bdi>
                      </div>
                    </div>
                  </td>
                  <td data-label={language === 'ar' ? 'الحضور' : 'Attendance'}>
                    <div className="attendance-cell">
                      <div>
                        <div className="attendance-row-controls">
                          {language === 'ar' && note}
                          <ToggleGroup
                            className="attendance-segments"
                            aria-label={`${t('Attendance for')} ${row.name}`}
                            value={[row.draft ?? row.saved ?? ''].filter(Boolean)}
                            disabled={disabled || !onMark}
                            onValueChange={(value) => onMark?.(row.id, value)}
                          >
                            {statuses.map((status) => {
                              const Icon =
                                status === 'present' ? Check : status === 'absent' ? X : CircleMinus
                              return (
                                <Toggle
                                  key={status}
                                  value={status}
                                  className={`attendance-segment attendance-segment-${status}`}
                                  data-replay-action="mark"
                                  data-replay-status={status}
                                  aria-label={t(status)}
                                  title={t(status)}
                                >
                                  <Icon size={18} aria-hidden="true" />
                                </Toggle>
                              )
                            })}
                          </ToggleGroup>
                          {language === 'en' && note}
                          {canCorrect && row.saved && (
                            <button
                              type="button"
                              className="attendance-correct-button"
                              data-replay-action="correction"
                              aria-label={`${language === 'ar' ? 'تصحيح العلامة' : 'Correct this mark'}: ${row.name}`}
                              title={language === 'ar' ? 'تصحيح العلامة' : 'Correct this mark'}
                              aria-haspopup="dialog"
                              disabled={disabled || !onCorrect}
                              onClick={() => onCorrect?.(row.id)}
                            >
                              <Pencil size={16} aria-hidden="true" />
                            </button>
                          )}
                        </div>
                        {!onMark && row.draft && row.draft !== row.saved && (
                          <span className="muted">{language === 'ar' ? 'مسودة' : 'Draft'}</span>
                        )}
                      </div>
                    </div>
                  </td>
                </AttendanceRow>
              )
            })}
          </tbody>
        </table>
      </div>
    </DirectionProvider>
  )
}
