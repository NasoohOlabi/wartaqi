import './course-sections.css'
import { DirectionProvider } from '@base-ui/react/direction-provider'
import { NavigationMenu } from '@base-ui/react/navigation-menu'
import { queryHref, setQuery } from './queryState'
import { useLanguage } from './useLanguage'

export function SettingsNavigation({
  current,
}: {
  current: 'accounts' | 'students' | 'history' | 'diagnostics' | 'telegram'
}) {
  const { language, t } = useLanguage()
  const ar = language === 'ar'
  const sections = [
    { id: 'accounts', label: ar ? 'الحسابات' : 'Accounts' },
    { id: 'students', label: t('Students') },
    { id: 'history', label: t('History & corrections') },
    { id: 'diagnostics', label: ar ? 'التشخيص' : 'Diagnostics' },
    { id: 'telegram', label: ar ? 'تيليغرام' : 'Telegram' },
  ]
  return (
    <DirectionProvider direction={ar ? 'rtl' : 'ltr'}>
      <NavigationMenu.Root aria-label={ar ? 'أقسام الإعدادات' : 'Settings sections'}>
        <NavigationMenu.List className="workspace-sections">
          {sections.map(({ id, label }) => {
            const values: Record<string, string> =
              id === 'history' && current === 'history'
                ? {}
                : {
                    view:
                      id === 'history' || id === 'diagnostics' || id === 'telegram'
                        ? id
                        : 'settings',
                    settingsSection: id === 'students' ? id : '',
                    student: '',
                    account: '',
                    ...(id === 'history' ? { tab: '', q: '', group: '' } : {}),
                  }
            const href = queryHref(values)
            return (
              <NavigationMenu.Item key={id}>
                <NavigationMenu.Link
                  active={current === id}
                  aria-current={current === id ? 'page' : undefined}
                  href={href}
                  onClick={(event) => {
                    if (
                      event.button !== 0 ||
                      event.metaKey ||
                      event.ctrlKey ||
                      event.shiftKey ||
                      event.altKey
                    )
                      return
                    event.preventDefault()
                    if (href !== window.location.pathname + window.location.search)
                      setQuery(values, true)
                  }}
                >
                  {label}
                </NavigationMenu.Link>
              </NavigationMenu.Item>
            )
          })}
        </NavigationMenu.List>
      </NavigationMenu.Root>
    </DirectionProvider>
  )
}
