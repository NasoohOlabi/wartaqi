import type { ReactNode } from 'react'

export function WorkspacePageHeader({
  title,
  parent,
  actions,
  children,
}: {
  title: ReactNode
  parent?: ReactNode
  actions?: ReactNode
  children?: ReactNode
}) {
  return (
    <header className="workspace-page-header">
      <div className="workspace-page-title">
        <div className="workspace-page-context">
          {parent}
          <h1>{title}</h1>
        </div>
        {actions && <div className="workspace-page-actions">{actions}</div>}
      </div>
      {children}
    </header>
  )
}
