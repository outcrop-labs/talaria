<script lang="ts">
  // The shared "Brain" routing control — identical on KB docs and artifacts.
  // Auto = the item's normal flows decide; None = never indexed; a custom brain
  // = explicit assignment (lives only there). Owner-only: routing decides who
  // can retrieve the content.
  import QueryError from '@/components/ui/QueryError.svelte'
  import Select from '@/components/ui/Select.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { useBrains } from '@/lib/kb'

  let {
    value,
    canEdit,
    onChange,
    class: className,
  }: {
    value: string
    canEdit: boolean
    onChange: (routing: string) => void
    class?: string
  } = $props()

  const brains = listQuery(useBrains(), { title: 'Could not load brains', variant: 'inline' })
</script>

<!-- `{ data: brains = [] }` made this control DISAPPEAR on a failed read (empty
     list + routing still 'auto' hits the early return below), so a doc whose
     brain you were about to change simply had no control for it and no reason
     given. A failure keeps the row and says so. -->
{#if brains.failed && value === 'auto'}
  <div class={className ?? 'w-36 shrink-0'}>
    {#if brains.notice}<QueryError {...brains.notice} />{/if}
  </div>
{:else if brains.rows.length === 0 && value === 'auto'}
  <!-- Nothing to choose and nothing chosen — stay out of the header. -->
{:else if brains.failed}
  <div class={className ?? 'w-36 shrink-0'}>
    <div class="truncate text-xs text-fg">Brain: {value}</div>
    {#if brains.notice}<QueryError {...brains.notice} />{/if}
  </div>
{:else}
  <Select
    size="sm"
    class={className ?? 'w-36 shrink-0'}
    {value}
    disabled={!canEdit}
    title={canEdit ? 'Which brain retrieves this content' : 'Only the owner can change brain routing'}
    onchange={(e) => onChange(e.currentTarget.value)}
  >
    <option value="auto">Brain: auto</option>
    <option value="none">Brain: none</option>
    {#each brains.rows as b (b.id)}
      <option value={b.id}>Brain: {b.name}</option>
    {/each}
  </Select>
{/if}
