import { useEffect, useRef } from 'react'
import { ProfileMenu, type Profile } from './ProfileMenu'
import { ArrowLeft, BookOpen, House, Settings, UserRound } from 'lucide-react'
import { useLanguage, type TranslationKey } from './useLanguage'
import {
  navigateNotebook,
  openChildView,
  returnToParent,
  setQuery,
  useQueryState,
  type Tab,
} from './queryState'
import { WartaqIcon, WartaqLogo } from './brand/components'

const tabs: { id: Exclude<Tab, 'dashboard'>; label: TranslationKey }[] = [
  { id: 'attendance', label: 'Attendance' },
  { id: 'roster', label: 'Students' },
]

export type WorkspaceBack = { label: string; onClick: () => void }

export function WorkspaceNavigation({
  administrator,
  student = false,
  offline = false,
  courseName,
  courses,
  currentCourse,
  chooseDefault,
  defaultCourse,
  profile,
  leave,
  disabled,
  back,
}: {
  administrator: boolean
  student?: boolean
  offline?: boolean
  courseName?: string
  courses?: { id: string; name: string; closed?: boolean }[]
  currentCourse?: string
  chooseDefault?: (course: string) => void
  defaultCourse?: string
  profile?: Profile
  leave: () => void
  disabled?: boolean
  back?: WorkspaceBack
}) {
  const { t, language } = useLanguage()
  const query = useQueryState()
  const notebookCourse =
    query.view === 'notebook' ||
    query.view === 'history' ||
    query.view === 'student' ||
    (query.view === 'courses' && !!query.course) ||
    query.view === 'changes'
      ? query.courseScope === 'all'
        ? ''
        : query.courseScope || defaultCourse
      : defaultCourse
  const roster = query.view === 'notebook' && query.tab === 'roster'
  const studentSubview = ['student', 'add-student', 'edit-student'].includes(query.view)
  const coursesActive = query.view === 'courses' || (studentSubview && !!query.courseView)
  const settingsActive = [
    'settings',
    'administration',
    'history',
    'diagnostics',
    'telegram',
    'invitation-message',
  ].includes(query.view)
  const notebookActive = query.view === 'notebook' || (studentSubview && !query.courseView)
  const courseNumber = courseName?.match(/^وارتق\s+([0-9\u0660-\u0669\u06f0-\u06f9]+)$/u)?.[1]
  const switcher = useRef<HTMLDetailsElement>(null)
  useEffect(() => {
    function dismiss(event: MouseEvent | KeyboardEvent) {
      const element = switcher.current
      if (!element?.open) return
      if (event instanceof KeyboardEvent && event.key === 'Escape') {
        element.open = false
        element.querySelector('summary')?.focus()
      } else if (
        event instanceof MouseEvent &&
        event.target instanceof Node &&
        !element.contains(event.target)
      ) {
        element.open = false
      }
    }
    document.addEventListener('click', dismiss)
    document.addEventListener('keydown', dismiss)
    return () => {
      document.removeEventListener('click', dismiss)
      document.removeEventListener('keydown', dismiss)
    }
  }, [])
  return (
    <aside className="workspace-rail">
      <div className="workspace-brand">
        <div className="flex items-center gap-1">
          {back && (
            <button
              type="button"
              className="workspace-up size-12 shrink-0 items-center justify-center rounded-full text-white hover:bg-white/10"
              aria-label={back.label}
              title={back.label}
              onClick={back.onClick}
            >
              <ArrowLeft className="icon-directional" size={24} aria-hidden="true" />
            </button>
          )}
          <WartaqLogo />
        </div>
        <ProfileMenu
          profile={profile}
          administrator={administrator}
          offline={offline}
          leave={leave}
          disabled={disabled}
        />
        {courses && chooseDefault && (
          <div className="workspace-course-controls" data-numbered={courseNumber !== undefined}>
            <details
              ref={switcher}
              className="workspace-course-switcher"
              data-numbered={courseNumber !== undefined}
            >
              <summary
                className="workspace-course-name"
                aria-label={language === 'ar' ? 'تغيير الدورة' : 'Change course'}
                aria-description={courseName}
                title={courseName}
              >
                <span dir="auto">
                  {courseNumber ??
                    courseName ??
                    (roster
                      ? language === 'ar'
                        ? 'جميع الطلاب'
                        : 'All students'
                      : query.view === 'history'
                        ? language === 'ar'
                          ? 'السجل السابق'
                          : 'Previous'
                        : language === 'ar'
                          ? 'اختر دورة'
                          : 'Choose course')}
                </span>
              </summary>
              <div className="workspace-course-menu">
                {[
                  roster
                    ? {
                        id: 'all',
                        name: language === 'ar' ? 'جميع الطلاب' : 'All students',
                      }
                    : {
                        id: 'none',
                        name:
                          language === 'ar' ? 'السجل السابق / بلا دورة' : 'Previous / no course',
                      },
                  ...courses.filter(
                    (course) =>
                      query.view === 'history' ||
                      !course.closed ||
                      course.id === currentCourse ||
                      course.id === defaultCourse,
                  ),
                ].map((course) => (
                  <button
                    key={course.id}
                    type="button"
                    data-diagnostic-action="course-switch"
                    aria-pressed={
                      (roster && (!currentCourse || currentCourse === 'none')
                        ? 'all'
                        : currentCourse || 'none') === course.id
                    }
                    onClick={(event) => {
                      setQuery(
                        { course: course.id, ...(query.view === 'student' ? {} : { student: '' }) },
                        true,
                      )
                      const details = event.currentTarget.closest('details')
                      details?.removeAttribute('open')
                      details?.querySelector('summary')?.focus()
                    }}
                    dir="auto"
                  >
                    {course.name}
                  </button>
                ))}
                {currentCourse !== 'all' &&
                  (!roster || !!query.course) &&
                  !courses.find((course) => course.id === currentCourse)?.closed &&
                  query.course !== (defaultCourse || '') && (
                    <button
                      type="button"
                      onClick={(event) => {
                        chooseDefault(currentCourse === 'none' ? '' : currentCourse || '')
                        const details = event.currentTarget.closest('details')
                        details?.removeAttribute('open')
                        details?.querySelector('summary')?.focus()
                      }}
                    >
                      {language === 'ar' ? 'تعيين كافتراضية' : 'Make default'}
                    </button>
                  )}
              </div>
            </details>
            {query.view === 'student' && query.course && (
              <button
                type="button"
                className="workspace-course-clear"
                title={
                  language === 'ar'
                    ? 'عرض معلومات الطالب العامة'
                    : 'Show general student information'
                }
                onClick={() => {
                  setQuery({ course: 'none' })
                  switcher.current?.querySelector('summary')?.focus()
                }}
              >
                {language === 'ar' ? 'المعلومات العامة' : 'General info'}
              </button>
            )}
          </div>
        )}
      </div>
      <nav aria-label={t('Main navigation')} data-courses={administrator} data-student={student}>
        {student ? (
          <>
            <button
              className={query.view !== 'profile' ? 'active' : ''}
              aria-current={query.view !== 'profile' ? 'page' : undefined}
              onClick={() => returnToParent({ view: 'notebook' })}
            >
              <WartaqIcon name="attendance" />
              <span>{language === 'ar' ? 'حضوري' : 'My attendance'}</span>
            </button>
            <button
              className={query.view === 'profile' ? 'active' : ''}
              aria-current={query.view === 'profile' ? 'page' : undefined}
              onClick={() => openChildView({ view: 'profile' })}
            >
              <UserRound size={20} aria-hidden="true" />
              <span>{language === 'ar' ? 'إعدادات الملف الشخصي' : 'Profile settings'}</span>
            </button>
          </>
        ) : (
          <>
            <button
              className={notebookActive && query.tab === 'dashboard' ? 'active' : ''}
              aria-label={language === 'ar' ? 'الرئيسية' : 'Dashboard'}
              aria-current={notebookActive && query.tab === 'dashboard' ? 'page' : undefined}
              onClick={() =>
                navigateNotebook('dashboard', notebookCourse, {
                  courseView: '',
                  student: '',
                  account: '',
                })
              }
            >
              <House size={20} strokeWidth={1.6} aria-hidden="true" focusable="false" />
              <span className="workspace-tab-long">
                {language === 'ar' ? 'الرئيسية' : 'Dashboard'}
              </span>
              <span className="workspace-tab-short" aria-hidden="true">
                {language === 'ar' ? 'الرئيسية' : 'Home'}
              </span>
            </button>
            {administrator && (
              <button
                className={coursesActive ? 'active' : ''}
                aria-current={coursesActive ? 'page' : undefined}
                disabled={offline}
                onClick={() =>
                  setQuery(
                    {
                      view: 'courses',
                      courseView: '',
                      course: '',
                      q: '',
                      student: '',
                      account: '',
                    },
                    true,
                  )
                }
              >
                <BookOpen size={20} strokeWidth={1.6} aria-hidden="true" focusable="false" />
                <span>{language === 'ar' ? 'الدورات' : 'Courses'}</span>
              </button>
            )}
            {tabs.map((tab) => {
              const active =
                notebookActive &&
                (query.tab === tab.id || (tab.id === 'attendance' && query.tab === 'summary'))
              return (
                <button
                  key={tab.id}
                  className={active ? 'active' : ''}
                  aria-label={t(tab.label)}
                  aria-current={active ? 'page' : undefined}
                  onClick={() =>
                    navigateNotebook(
                      tab.id,
                      tab.id === 'roster' && query.courseScope === 'all' ? 'all' : notebookCourse,
                      {
                        student: '',
                        account: '',
                        courseView: '',
                      },
                    )
                  }
                >
                  <WartaqIcon name={tab.id === 'roster' ? 'students' : tab.id} />
                  <span className={tab.id === 'attendance' ? 'workspace-tab-long' : undefined}>
                    {t(tab.label)}
                  </span>
                  {tab.id === 'attendance' && (
                    <span className="workspace-tab-short" aria-hidden="true">
                      {language === 'ar' ? 'الحضور' : 'Attend'}
                    </span>
                  )}
                </button>
              )
            })}
            {administrator && (
              <button
                className={settingsActive ? 'active' : ''}
                aria-current={settingsActive ? 'page' : undefined}
                disabled={offline}
                onClick={() =>
                  setQuery(
                    {
                      view: 'settings',
                      student: '',
                      settingsSection: '',
                      courseView: '',
                      account: '',
                    },
                    true,
                  )
                }
              >
                <Settings size={20} strokeWidth={1.6} aria-hidden="true" />
                <span>{language === 'ar' ? 'الإعدادات' : 'Settings'}</span>
              </button>
            )}
          </>
        )}
      </nav>
      <div className="workspace-identity">
        <span>{t('Attendance & notes')}</span>
      </div>
    </aside>
  )
}
