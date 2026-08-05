<script lang="ts">
  import Combobox from '@/components/ui/Combobox.svelte'
  import type { ControlSize } from '@/components/ui/control'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { cn } from '@/lib/cn'
  import { useUsers, type DirectoryUser } from '@/lib/users'

  // The one way to bring a person in — a searchable picker over everyone who has
  // signed in. Acts as a command: picking fires onPick and the trigger resets, so
  // it drops into any "add person" row (share, invite, channels).
  let {
    exclude = [],
    onPick,
    placeholder = 'Add a person',
    size,
    class: className,
  }: {
    /** User ids already in — hidden from the options. */
    exclude?: string[]
    onPick: (user: DirectoryUser) => void
    placeholder?: string
    size?: ControlSize
    class?: string
  } = $props()

  // Not `{ data: users = [] }`: an empty directory and a 500 are the same `[]`,
  // and this picker's whole job is to say who exists. Over an outage it used to
  // render "Everyone's already here" and disable itself — a confident, wrong
  // answer with no way to retry.
  const list = listQuery(useUsers(), { title: 'Could not load people', variant: 'inline' })
  const options = $derived.by(() => {
    const excluded = new Set(exclude)
    return list.rows
      .filter((u) => !excluded.has(u.id))
      .map((u) => ({ value: u.id, label: u.name ?? u.email ?? u.id, sub: u.name ? (u.email ?? undefined) : undefined }))
  })
</script>

{#if list.pending}
  <!-- While the directory loads, a disabled empty picker would read as "no one to
       add" (and swallow the click) — hold the slot with a combobox-shaped shimmer. -->
  <Skeleton class={cn(size === 'sm' ? 'h-9' : 'h-11', 'w-full', className)} />
{:else if list.failed}
  <!-- The directory is gone: say so where the picker was, rather than offering an
       empty one. `notice` carries its own Retry. A stale directory keeps the
       picker and wears the marker — you can still pick someone you can see. -->
  <div class={className}>
    {#if list.notice}<QueryError {...list.notice} />{/if}
  </div>
{:else}
  <div class={className}>
    {#if list.notice}<QueryError {...list.notice} />{/if}
    <Combobox
      {options}
      selected={[]}
      onChange={([id]) => {
        const user = list.rows.find((u) => u.id === id)
        if (user) onPick(user)
      }}
      placeholder={options.length ? placeholder : 'Everyone’s already here'}
      disabled={options.length === 0}
      {size}
    />
  </div>
{/if}
