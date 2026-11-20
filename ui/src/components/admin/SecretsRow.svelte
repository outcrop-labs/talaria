<script lang="ts">
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowAction from '@/components/settings/SettingsRowAction.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import type { DotStatus } from '@/components/ui/chip'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { relativeTime } from '@/lib/fleet'
  import { GROUP_LABELS, STATE_COPY, type SecretRow, type SecretState } from '@/lib/secrets'
  import { navigateHref } from '@/router'
  import RowStatus from './RowStatus.svelte'
  import { SECRET_COLS } from './secret-hint'

  // One secret of the inventory. Never a value: what it is, what stops working
  // without it, whether this instance can still read it, and when it was set.
  let {
    row,
    onClear,
    busy,
    showGroup = false,
    showSet = false,
  }: {
    row: SecretRow
    onClear: (row: SecretRow) => void
    busy: boolean
    /** Name the row's group beside it, for a list that mixes groups. */
    showGroup?: boolean
    /** The list has a "Set" column, under a header row that names it. */
    showSet?: boolean
  } = $props()

  const DOT: Record<SecretState, DotStatus> = { ok: 'ok', unreadable: 'danger', missing: 'idle', env: 'idle' }
  // An environment variable's name is an identifier; a provider's name is not.
  const identifier = $derived(/^[A-Z][A-Z0-9_]+$/.test(row.label))
  const when = (iso: string) => new Date(iso).toLocaleString()
  const times = $derived(
    [
      row.setAt ? `Set ${when(row.setAt)}` : null,
      row.lastUsedAt ? `Last used ${when(row.lastUsedAt)}` : null,
      row.expiresAt ? `Expires ${when(row.expiresAt)}` : null,
    ]
      .filter(Boolean)
      .join('\n'),
  )
  const setUp = $derived(row.state === 'missing' && !!row.href)
  const items = $derived.by((): ContextMenuEntry[] => {
    const list: ContextMenuEntry[] = []
    const href = row.href
    if (href) list.push({ label: `${setUp ? 'Set up' : 'Replace'} in ${row.surface}`, onSelect: () => void navigateHref(href) })
    // No link when the value belongs to a person: they replace it in their own
    // Settings, and a link would land an admin on their own page.
    if (!href) list.push({ label: `Replaced by ${row.owner ?? 'its owner'} in ${row.surface}`, disabled: true })
    if (row.clearable) {
      if (list.length) list.push('sep')
      list.push({ label: 'Clear secret', danger: true, disabled: busy, onSelect: () => onClear(row) })
    }
    return list
  })
</script>

<SettingsRow hint={row.unlocks}>
  {#snippet label()}
    {#if identifier}<span class="font-mono text-[13px] font-normal">{row.label}</span>{:else}{row.label}{/if}
  {/snippet}
  {#snippet meta()}
    {#if row.owner}<span class="min-w-0 truncate font-sans text-[13px] leading-5 text-muted">{row.owner}</span>{/if}
    {#if showGroup}<span class="shrink-0 font-sans text-[13px] font-normal leading-5 text-muted">{GROUP_LABELS[row.group]}</span>{/if}
    {#if row.expiresAt}
      <Chip tone="warn" title={when(row.expiresAt)}>
        {new Date(row.expiresAt).getTime() <= Date.now() ? 'Expired' : `Expires ${new Date(row.expiresAt).toLocaleDateString()}`}
      </Chip>
    {/if}
  {/snippet}
  <span class={SECRET_COLS.state}>
    <RowStatus status={DOT[row.state]} title={STATE_COPY[row.state].hint}>{STATE_COPY[row.state].label}</RowStatus>
  </span>
  {#if showSet}
    <span class="{SECRET_COLS.set} font-sans text-[13px] leading-5 tabular-nums text-muted" title={times || undefined}>
      {row.setAt ? relativeTime(row.setAt) : '—'}
    </span>
  {/if}
  {#if setUp}<SettingsRowAction href={row.href} label={`Set up ${row.label} in ${row.surface}`}>Set up</SettingsRowAction>{/if}
  <SettingsRowMenu label={`Actions for ${row.label}`} {items} />
</SettingsRow>
