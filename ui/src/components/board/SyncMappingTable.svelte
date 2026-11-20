<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import Select from '@/components/ui/Select.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { targetNote, type MappingRow, type SyncTarget } from '@/lib/board-sync'

  // THE STATUS MAPPING: an explicit table from each ClickUp status to a board
  // column, nothing guessed. A row left on the first option is unmapped — its
  // tasks land in the sync intake and are listed as problems. Under each row,
  // one sentence says what landing in that column means: a pickup column
  // starts agent work (and the sync holds a move into it while a ticket has
  // agents), a done column closes the ticket and hands a workchain step on,
  // review and Blocked are entered but never left by the sync.
  //
  // `draft` is the caller's: keys are the status names lowercased (the table's
  // key on the wire), values a column key or '' for unmapped.
  let {
    rows,
    targets,
    intake,
    draft = $bindable(),
    disabled = false,
  }: {
    rows: MappingRow[]
    targets: SyncTarget[]
    intake: SyncTarget | null
    draft: Record<string, string>
    disabled?: boolean
  } = $props()

  const targetOf = (key: string | undefined) => (key ? (targets.find((t) => t.key === key) ?? null) : null)
  const suffix = (t: SyncTarget) => (t.agentStart ? ' — starts agent work' : t.terminal ? ' — done' : '')
</script>

<div class="divide-y divide-line-subtle rounded-lg border border-line" data-sync-mapping>
  {#each rows as row (row.key)}
    {@const target = targetOf(draft[row.key])}
    {@const note = targetNote(target, intake)}
    <div class="grid grid-cols-1 gap-1.5 px-3 py-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)] sm:items-start sm:gap-3" data-mapping-row={row.key}>
      <div class="flex min-w-0 items-center gap-2 pt-1.5">
        <StatusDot color={row.color ?? 'var(--theme-muted)'} />
        <span class="truncate font-sans text-sm text-fg" title={row.name}>{row.name}</span>
        {#if row.type}<Chip>{row.type}</Chip>{/if}
        {#if row.stale}<span class="font-sans text-[11px] text-muted">no longer in the list</span>{/if}
      </div>
      <div class="min-w-0">
        <Select
          size="sm"
          class="w-full"
          {disabled}
          aria-label={`Column for the ClickUp status ${row.name}`}
          value={draft[row.key] ?? ''}
          onchange={(e) => (draft = { ...draft, [row.key]: e.currentTarget.value })}
        >
          <option value="">{intake ? `${intake.label} (unmapped)` : 'Unmapped — refused'}</option>
          {#each targets as t (t.key)}
            <option value={t.key}>{t.label}{suffix(t)}</option>
          {/each}
        </Select>
        {#if note}
          <div class="mt-1 font-sans text-[11px] leading-snug {target ? 'text-muted' : 'text-warning'}">{note}</div>
        {/if}
      </div>
    </div>
  {/each}
</div>
