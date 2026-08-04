// The shared "Brain" routing control — identical on KB docs and artifacts.
// Auto = the item's normal flows decide; None = never indexed; a custom brain
// = explicit assignment (lives only there). Owner-only: routing decides who
// can retrieve the content.
import { listQuery } from '@/components/ui/query-state'
import { Select } from '@/components/ui/select'
import { useBrains } from '@/lib/kb'

export function BrainRoutingSelect({
  value,
  canEdit,
  onChange,
  className,
}: {
  value: string
  canEdit: boolean
  onChange: (routing: string) => void
  className?: string
}) {
  const { rows: brains, notice, failed } = listQuery(useBrains(), { title: 'Could not load brains', variant: 'inline' })
  // `{ data: brains = [] }` made this control DISAPPEAR on a failed read (empty
  // list + routing still 'auto' hits the early return below), so a doc whose
  // brain you were about to change simply had no control for it and no reason
  // given. A failure keeps the row and says so.
  if (failed && value === 'auto') return <div className={className ?? 'w-36 shrink-0'}>{notice}</div>
  // Nothing to choose and nothing chosen — stay out of the header.
  if (brains.length === 0 && value === 'auto') return null
  if (failed)
    return (
      <div className={className ?? 'w-36 shrink-0'}>
        <div className="truncate text-xs text-fg">Brain: {value}</div>
        {notice}
      </div>
    )
  return (
    <Select
      size="sm"
      className={className ?? 'w-36 shrink-0'}
      value={value}
      disabled={!canEdit}
      title={canEdit ? 'Which brain retrieves this content' : 'Only the owner can change brain routing'}
      onChange={(e) => onChange(e.target.value)}
    >
      <option value="auto">Brain: auto</option>
      <option value="none">Brain: none</option>
      {brains.map((b) => (
        <option key={b.id} value={b.id}>
          Brain: {b.name}
        </option>
      ))}
    </Select>
  )
}
