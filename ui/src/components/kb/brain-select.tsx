// The shared "Brain" routing control — identical on KB docs and artifacts.
// Auto = the item's normal flows decide; None = never indexed; a custom brain
// = explicit assignment (lives only there). Owner-only: routing decides who
// can retrieve the content.
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
  const { data: brains = [] } = useBrains()
  // Nothing to choose and nothing chosen — stay out of the header.
  if (brains.length === 0 && value === 'auto') return null
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
