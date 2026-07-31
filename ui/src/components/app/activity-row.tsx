// The one activity/audit row: "actor · detail" + relative time, with an
// optional quiet context line underneath. Shared by Home's activity feeds
// (ActivityList, Fleet pulse) and the Observability overview's audit trail.
import { relativeTime } from '@/lib/fleet'

export function ActivityRow({
  actor,
  detail,
  at,
  context,
  onClick,
}: {
  actor: string
  detail: string
  at: string
  /** Second line under the row; omit to render the single-line (audit) form. */
  context?: string
  onClick: () => void
}) {
  return (
    <li>
      <button
        type="button"
        onClick={onClick}
        className="w-full rounded-md px-1.5 py-1 text-left transition-colors hover:bg-card2"
      >
        <div className="flex items-baseline gap-2">
          <span className="min-w-0 flex-1 truncate font-sans text-xs text-fg">
            <span className="font-medium">{actor}</span> · {detail}
          </span>
          <span className="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">{relativeTime(at)}</span>
        </div>
        {context !== undefined && <div className="truncate font-sans text-[11px] text-muted">{context}</div>}
      </button>
    </li>
  )
}
