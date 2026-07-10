// The two page archetypes — every route uses exactly one, so the app stops
// feeling like fifteen apps:
//
//   <RailSurface>            work surfaces with a list rail + a stage
//     <Rail title actions>   LEFT rail: w-72 · bg-sidebar · border-r
//       rows go here
//     </Rail>
//     <Stage header={<StageHeader title />}>the main area</Stage>
//   </RailSurface>
//
//   (Centered "page" surfaces — Agents, Models, Admin, and the rest — keep the
//   `h-full overflow-y-auto p-8` + `mx-auto max-w-5xl` shape; PageTitle below
//   standardizes their heading row.)
//
// Rail rows share RailRow; surface headers share StageHeader (the one-line
// bar Comms channels already had). Contextual actions are IconButtons.
import { cn } from '@/lib/cn'

export function RailSurface({ children, className }: { children: React.ReactNode; className?: string }) {
  return <div className={cn('flex h-full min-h-0', className)}>{children}</div>
}

/** The list rail — ALWAYS on the left, always w-72. */
export function Rail({
  title,
  actions,
  children,
  className,
}: {
  title?: string
  /** Compact IconButtons; never a giant labeled button. */
  actions?: React.ReactNode
  children: React.ReactNode
  className?: string
}) {
  return (
    <aside className={cn('flex h-full w-72 shrink-0 flex-col border-r border-line-subtle bg-sidebar', className)}>
      {(title || actions) && (
        <div className="flex h-12 shrink-0 items-center gap-1.5 border-b border-line-subtle px-4">
          {title && <span className="min-w-0 flex-1 truncate text-sm font-semibold text-fg">{title}</span>}
          {actions}
        </div>
      )}
      <div className="min-h-0 flex-1 overflow-y-auto p-3">{children}</div>
    </aside>
  )
}

/** The stage (main area) beside a rail. */
export function Stage({ header, children, className }: { header?: React.ReactNode; children: React.ReactNode; className?: string }) {
  return (
    <main className={cn('flex min-h-0 min-w-0 flex-1 flex-col', className)}>
      {header}
      <div className="min-h-0 min-w-0 flex-1">{children}</div>
    </main>
  )
}

/** The one-line surface header: title + meta on the left, actions right.
 *  Same height as the rail header so the top line runs straight across. */
export function StageHeader({
  title,
  meta,
  actions,
  className,
}: {
  title: React.ReactNode
  meta?: React.ReactNode
  actions?: React.ReactNode
  className?: string
}) {
  return (
    <header className={cn('flex h-12 shrink-0 items-center gap-2 border-b border-line-subtle px-5', className)}>
      <span className="min-w-0 shrink truncate text-sm font-semibold text-fg">{title}</span>
      {meta && <span className="min-w-0 truncate text-xs text-muted">{meta}</span>}
      <span className="ml-auto" />
      {actions}
    </header>
  )
}

/** Rail section label ("CHANNELS", "AGENTS") with optional icon action. */
export function RailSection({ label, action, children }: { label: string; action?: React.ReactNode; children: React.ReactNode }) {
  return (
    <div className="mb-4">
      <div className="mb-1 flex items-center px-2">
        <span className="flex-1 text-[10px] font-semibold uppercase tracking-wide text-muted">{label}</span>
        {action}
      </div>
      <ul className="space-y-0.5">{children}</ul>
    </div>
  )
}

/** The rail row — one hover/active treatment everywhere. */
export function RailRow({
  active,
  onClick,
  className,
  children,
}: {
  active?: boolean
  onClick: () => void
  className?: string
  children: React.ReactNode
}) {
  return (
    <li>
      <button
        type="button"
        onClick={onClick}
        className={cn(
          'flex w-full min-w-0 items-center gap-1.5 rounded-lg px-2 py-1.5 text-left text-sm transition-colors hover:bg-card',
          active ? 'bg-card text-fg' : 'text-muted',
          className,
        )}
      >
        {children}
      </button>
    </li>
  )
}

/** Count pill (unread, pending) — one look everywhere. */
export function CountPill({ count, className }: { count?: number; className?: string }) {
  if (!count) return null
  return (
    <span className={cn('ml-auto shrink-0 rounded-full bg-accent px-1.5 text-[10px] font-semibold leading-4 text-surface', className)}>
      {count > 99 ? '99+' : count}
    </span>
  )
}
