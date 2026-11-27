import { today } from './queryState'
import { formatLevel } from './levelFormat'
import { TableHeader, TableToolbar } from './DataTable'
import { useDataTable } from './useDataTable'
import { RefreshCw } from 'lucide-react'
import { useLanguage } from './useLanguage'
import type { Profile } from './ProfileMenu'
import { WorkspaceNavigation } from './WorkspaceNavigation'
import { WorkspacePageHeader } from './WorkspacePageHeader'
import { studentAttendanceReplica } from './roleReplica'
import type { CachedAccess } from './offlineAccess'
import { useRoleReplica } from './useRoleReplica'
export function StudentAttendance({
  userId,
  accountId,
  signOut,
  cached,
  profile,
}: {
  userId: string
  accountId: string
  cached?: CachedAccess
  signOut: () => void
  profile: Profile
}) {
  const { language, t, errorMessage } = useLanguage()
  const ar = language === 'ar'
  const { data, error, loading, refresh } = useRoleReplica(
    studentAttendanceReplica,
    userId,
    accountId,
  )
  const attendanceTable = useDataTable(
    'portal-attendance',
    [...(data?.attendance ?? [])].sort((a, b) => b.date.localeCompare(a.date)),
    [
      {
        id: 'course',
        label: ar ? 'الدورة' : 'Course',
        value: (item) => item.course ?? (ar ? 'سجل سابق' : 'Previous record'),
      },
      { id: 'date', filterMode: 'literal', label: t('Date'), value: (item) => item.date },
      {
        id: 'status',
        choices: [t('present'), t('absent'), t('excused')],
        label: ar ? 'الحالة' : 'Status',
        value: (item) => t(item.status),
      },
    ],
  )
  return (
    <div className="workspace">
      <WorkspaceNavigation
        administrator={false}
        student
        offline={!!cached}
        profile={profile}
        leave={signOut}
      />
      <main id="main-content" className="workspace-main" tabIndex={-1}>
        <WorkspacePageHeader
          title={ar ? 'حضوري' : 'My attendance'}
          actions={
            <TableToolbar
              model={data?.attendance.length ? attendanceTable : undefined}
              actions={
                <button
                  className="icon-button"
                  aria-label={t('Refresh')}
                  title={t('Refresh')}
                  onClick={() => {
                    cached?.reconnect()
                    void refresh()
                  }}
                >
                  <RefreshCw size={20} strokeWidth={1.6} aria-hidden="true" focusable="false" />
                </button>
              }
            >
              {null}
            </TableToolbar>
          }
        />
        {data && (
          <dl className="grid grid-cols-2 gap-2 break-words">
            <div>
              <dt>{ar ? 'الطالب' : 'Student'}</dt>
              <dd dir="auto">{data.student.name}</dd>
            </div>
            <div>
              <dt>{ar ? 'مستوى السجل' : 'Roster level'}</dt>
              <dd dir="auto">{formatLevel(data.student.group) || '—'}</dd>
            </div>
          </dl>
        )}
        {cached?.offline && <p role="status">{ar ? 'غير متصل' : 'Offline'}</p>}
        {loading && !data && !cached?.offline && (
          <p role="status">{ar ? 'جارٍ التحميل…' : 'Loading…'}</p>
        )}
        {cached?.offline && !data && (
          <p>{ar ? 'أعد الاتصال لعرض سجل الحضور.' : 'Connect to load your attendance.'}</p>
        )}
        {error && <p role="alert">{errorMessage(error)}</p>}
        {data && (
          <section className="panel">
            {!data.attendance.some((item) => item.date === today()) && (
              <p>
                {ar
                  ? '\u0644\u0627 \u064a\u0648\u062c\u062f \u0633\u062c\u0644 \u062d\u0636\u0648\u0631 \u0644\u0644\u064a\u0648\u0645.'
                  : 'No attendance recorded today.'}
              </p>
            )}
            {data.attendance.length ? (
              <table className="mobile-record-table role-table">
                <TableHeader model={attendanceTable} />
                <tbody>
                  {attendanceTable.rows.map((item) => (
                    <tr key={`${item.enrollmentId ?? 'legacy'}:${item.date}`}>
                      <td data-label={ar ? 'الدورة' : 'Course'}>
                        {item.course ?? (ar ? 'سجل سابق' : 'Previous record')}
                      </td>
                      <td data-label={t('Date')}>
                        <bdi>{item.date}</bdi>
                      </td>
                      <td data-label={ar ? 'الحالة' : 'Status'}>{t(item.status)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            ) : (
              <p>{ar ? 'لا توجد سجلات حضور بعد.' : 'No attendance records yet.'}</p>
            )}
          </section>
        )}
      </main>
    </div>
  )
}
