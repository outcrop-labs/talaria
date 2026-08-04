import { Combobox } from '@/components/ui/combobox'
import type { ControlSize } from '@/components/ui/control'
import { listQuery } from '@/components/ui/query-state'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/cn'
import { useUsers, type DirectoryUser } from '@/lib/users'

// The one way to bring a person in — a searchable picker over everyone who has
// signed in. Acts as a command: picking fires onPick and the trigger resets, so
// it drops into any "add person" row (share, invite, channels).
export function UserPicker({
  exclude = [],
  onPick,
  placeholder = 'Add a person',
  size,
  className,
}: {
  /** User ids already in — hidden from the options. */
  exclude?: string[]
  onPick: (user: DirectoryUser) => void
  placeholder?: string
  size?: ControlSize
  className?: string
}) {
  // Not `{ data: users = [] }`: an empty directory and a 500 are the same `[]`,
  // and this picker's whole job is to say who exists. Over an outage it used to
  // render "Everyone's already here" and disable itself — a confident, wrong
  // answer with no way to retry.
  const {
    rows: users,
    notice,
    failed,
    pending,
  } = listQuery(useUsers(), { title: 'Could not load people', variant: 'inline' })
  const excluded = new Set(exclude)
  const options = users
    .filter((u) => !excluded.has(u.id))
    .map((u) => ({ value: u.id, label: u.name ?? u.email ?? u.id, sub: u.name ? (u.email ?? undefined) : undefined }))

  // While the directory loads, a disabled empty picker would read as "no one to
  // add" (and swallow the click) — hold the slot with a combobox-shaped shimmer.
  if (pending) return <Skeleton className={cn(size === 'sm' ? 'h-9' : 'h-11', 'w-full', className)} />
  // The directory is gone: say so where the picker was, rather than offering an
  // empty one. `notice` carries its own Retry. A stale directory keeps the
  // picker and wears the marker — you can still pick someone you can see.
  if (failed) return <div className={className}>{notice}</div>

  return (
    <div className={className}>
      {notice}
      <Combobox
        options={options}
        selected={[]}
        onChange={([id]) => {
          const user = users.find((u) => u.id === id)
          if (user) onPick(user)
        }}
        placeholder={options.length ? placeholder : 'Everyone’s already here'}
        disabled={options.length === 0}
        size={size}
      />
    </div>
  )
}
