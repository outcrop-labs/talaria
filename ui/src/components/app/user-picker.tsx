import { Combobox } from '@/components/ui/combobox'
import type { ControlSize } from '@/components/ui/control'
import { useUsers, type DirectoryUser } from '@/lib/users'

// The one way to bring a person in — a searchable picker over everyone who has
// signed in. Acts as a command: picking fires onPick and the trigger resets, so
// it drops into any "add person" row (share, invite, channels).
export function UserPicker({
  exclude = [],
  onPick,
  placeholder = 'Add a person…',
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
  const { data: users = [] } = useUsers()
  const excluded = new Set(exclude)
  const options = users
    .filter((u) => !excluded.has(u.id))
    .map((u) => ({ value: u.id, label: u.name ?? u.email ?? u.id, sub: u.name ? (u.email ?? undefined) : undefined }))

  return (
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
      className={className}
    />
  )
}
