<script lang="ts">
  // Admin → Secrets. Everything this instance holds sealed, in one list, with
  // per-row health and a per-row way out.
  //
  // The shape follows from two facts. Values can never be shown, so the row's
  // job is provenance and health rather than content — what it is, what breaks
  // without it, when it was set, whether it still decrypts. And an operator
  // arrives here in one of two moods: "what does this instance have?" (browsing)
  // or "three things broke at once" (recovering). So what needs a decision
  // leads: unreadable secrets, then the ones never set, then the rest by group.
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Stat from '@/components/settings/Stat.svelte'
  import StatStrip from '@/components/settings/StatStrip.svelte'
  import Button from '@/components/ui/Button.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { slide } from '@/lib/motion'
  import { GROUP_LABELS, useClearSecret, useSecretHealth, type SecretGroup, type SecretRow } from '@/lib/secrets'
  import RootCard from './RootCard.svelte'
  import SecretsRow from './SecretsRow.svelte'

  const GROUP_ORDER: SecretGroup[] = ['models', 'integrations', 'agents', 'platform']

  const query = useSecretHealth()
  const data = $derived(query.data)
  const clear = useClearSecret()
  let busy = $state(false)
  let msg = $state<string | null>(null)

  const clearOne = async (row: SecretRow) => {
    const ok = await confirm({
      title: `Clear ${row.label}?`,
      // Every clear names the consequence and the way back. This is the same
      // contract `talaria reset secrets` keeps, at the granularity of one row.
      message: `This deletes the stored value and nothing else. What stops working: ${row.unlocks}. To restore it, enter the value again in ${row.surface}.${
        row.id.startsWith('agent-key:')
          ? ' The agent will need its configuration rebuilt to get a new credential.'
          : ''
      }`,
      confirmLabel: 'Clear',
    })
    if (!ok) return
    busy = true
    msg = null
    try {
      const res = await clear({ id: row.id })
      msg = res.error ?? (res.changed ? `Cleared ${row.label}.` : `${row.label} was already cleared.`)
    } finally {
      busy = false
    }
  }

  const clearAllUnreadable = async () => {
    const rows = (data?.rows ?? []).filter((r) => r.state === 'unreadable' && r.clearable)
    const ok = await confirm({
      title: `Clear ${rows.length} unreadable secret${rows.length === 1 ? '' : 's'}?`,
      // Naming them is the whole point: "clear all" without a list is how an
      // operator ends up destroying something they could still have recovered.
      message: `These will be deleted:\n\n${rows.map((r) => `· ${r.label}${r.owner ? ` (${r.owner})` : ''}: re-enter in ${r.surface}`).join('\n')}\n\nEverything readable is left alone. If you still have the original root secret anywhere, restoring it recovers these instead.`,
      confirmLabel: 'Clear them',
    })
    if (!ok) return
    busy = true
    msg = null
    try {
      const res = await clear({ unreadable: true })
      msg =
        res.error ??
        `Cleared ${res.cleared?.length ?? 0}${res.failed?.length ? ` · ${res.failed.length} could not be cleared (see server logs)` : ''}.`
    } finally {
      busy = false
    }
  }
</script>

<!-- An empty inventory over a failed read would read as "this instance holds
     nothing" — the most reassuring possible way to be wrong, so QueryState's
     error branch owns that. -->
<QueryState query={query} errorTitle="Could not load the secrets inventory" errorVariant="compact">
  {#snippet skeleton()}
    <Skeleton class="h-[104px] w-full rounded-lg" />
    <SettingsGroup><div class="px-5 py-4"><SkeletonRows rows={5} /></div></SettingsGroup>
  {/snippet}
  {#snippet children(data)}
    {@const unreadable = data.rows.filter((r) => r.state === 'unreadable')}
    {@const missing = data.rows.filter((r) => r.state === 'missing')}
    <StatStrip>
      <Stat label="Stored" value={data.counts.ok} sub="Readable with the current key" />
      <Stat label="Not set" value={data.counts.missing} sub="Nothing entered yet" />
      <Stat label="From environment" value={data.counts.env} sub="Set where the server runs" />
      <Stat label="Unreadable" value={data.counts.unreadable} sub="Locked with a key that is gone" />
    </StatStrip>

    <SettingsSection title="Encryption" description="One value protects every secret on this page.">
      <SettingsGroup><RootCard root={data.root} /></SettingsGroup>
    </SettingsSection>

    {#if msg}<p transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-muted">{msg}</p>{/if}

    {#if data.counts.unreadable > 0}
      <div transition:slide={{ duration: 150 }}>
        <SettingsSection
          title="Unreadable"
          description="Locked with a key this instance no longer has. Restore the original root key, or clear them and enter the values again."
        >
          {#snippet action()}
            <Button size="sm" variant="danger-outline" onclick={() => void clearAllUnreadable()} disabled={busy}>
              Clear all unreadable
            </Button>
          {/snippet}
          <SettingsGroup>
            {#each unreadable as r (r.id)}
              <SecretsRow row={r} onClear={(row) => void clearOne(row)} {busy} showGroup />
            {/each}
          </SettingsGroup>
        </SettingsSection>
      </div>
    {/if}

    {#if missing.length}
      <SettingsSection title="Not set" description="Nothing is stored for these yet. Whatever depends on one stays off until it is set.">
        <SettingsGroup>
          {#each missing as r (r.id)}
            <SecretsRow row={r} onClear={(row) => void clearOne(row)} {busy} showGroup />
          {/each}
        </SettingsGroup>
      </SettingsSection>
    {/if}

    {#each GROUP_ORDER as group (group)}
      {@const rows = data.rows.filter((r) => r.group === group && r.state !== 'unreadable' && r.state !== 'missing')}
      {#if rows.length}
        <SettingsSection title={GROUP_LABELS[group]}>
          <SettingsGroup>
            {#each rows as r (r.id)}
              <SecretsRow row={r} onClear={(row) => void clearOne(row)} {busy} />
            {/each}
          </SettingsGroup>
        </SettingsSection>
      {/if}
    {/each}

    {#if !data.rows.length}
      <SettingsGroup>
        <SettingsRow label="Nothing stored yet" hint="Secrets appear here as you add them: a provider key on Models, a connection, an agent secret." />
      </SettingsGroup>
    {/if}
  {/snippet}
</QueryState>
