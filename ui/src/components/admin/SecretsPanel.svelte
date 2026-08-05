<script lang="ts">
  // Admin → Secrets. Everything this instance holds sealed, in one list, with
  // per-row health and a per-row way out.
  //
  // The shape follows from two facts. Values can never be shown, so the row's
  // job is provenance and health rather than content — what it is, what breaks
  // without it, when it was set, whether it still decrypts. And an operator
  // arrives here in one of two moods: "what does this instance have?" (browsing)
  // or "three things broke at once" (recovering). The unreadable strip at the
  // top serves the second without making the first read like an incident.
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Button from '@/components/ui/Button.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { GROUP_LABELS, useClearSecret, useSecretHealth, type SecretGroup, type SecretRow } from '@/lib/secrets'
  import RootCard from './RootCard.svelte'
  import SecretsRow from './SecretsRow.svelte'

  const GROUP_ORDER: SecretGroup[] = ['models', 'integrations', 'agents', 'platform']

  const query = useSecretHealth()
  const data = $derived(query.data)
  const isPending = $derived(query.isPending)
  const clear = useClearSecret()
  let busy = $state(false)
  let msg = $state<string | null>(null)

  const clearOne = async (row: SecretRow) => {
    const ok = await confirm({
      title: `Clear ${row.label}?`,
      // Every clear names the consequence and the way back. This is the same
      // contract scripts/reset.sh keeps, at the granularity of one row.
      message: `This deletes the stored value and nothing else. What stops working: ${row.unlocks}. To restore it, enter the value again in ${row.surface}.${
        row.id.startsWith('agent-key:')
          ? ' The agent will need a fleet re-render to get a new credential.'
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
      message: `These will be deleted:\n\n${rows.map((r) => `· ${r.label}${r.owner ? ` (${r.owner})` : ''} — re-enter in ${r.surface}`).join('\n')}\n\nEverything readable is left alone. If you still have the original root secret anywhere, restoring it recovers these instead.`,
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

<Panel>
  <SectionHeader
    class="mb-4"
    title="Secrets"
    info="Every credential this instance holds, wherever it was entered. Values are never shown — sealed secrets cannot be read back, only replaced. Each row says what it unlocks and whether this instance can still decrypt it."
  />

  {#if isPending}
    <div class="space-y-3">
      <Skeleton class="h-16 w-full rounded-md" />
      {#each Array.from({ length: 5 }) as _, i (i)}
        <div class="space-y-1.5 py-1">
          <Skeleton class="h-3 w-48 rounded-full" delay={i * 0.1} />
          <Skeleton class="h-2.5 w-72 rounded-full" delay={i * 0.1 + 0.05} />
        </div>
      {/each}
    </div>
  {:else if !data}
    <!-- An empty inventory over a failed read would read as "this instance
         holds nothing" — the most reassuring possible way to be wrong. -->
    <QueryError
      variant="compact"
      error={query.error}
      title="Could not load the secrets inventory"
      onRetry={() => void query.refetch()}
    />
  {:else}
    <div class="space-y-5">
      <RootCard root={data.root} />

      {#if data.counts.unreadable > 0}
        <div class="rounded-md border border-danger/40 p-4">
          <div class="mb-2 flex items-center gap-1.5">
            <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-danger">
              {data.counts.unreadable} unreadable
            </span>
            <span class="ml-auto font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">Irreversible</span>
          </div>
          <p class="mb-3 text-xs leading-relaxed text-muted">
            These were sealed with a key this instance no longer has. If you still have the original root secret
            anywhere, restore it — that recovers them. Clearing deletes them so the value can be entered again;
            everything readable is left alone.
          </p>
          <Button size="sm" variant="danger" onclick={() => void clearAllUnreadable()} disabled={busy}>
            Clear all unreadable
          </Button>
        </div>
      {/if}

      {#if msg}<p class="text-xs text-muted">{msg}</p>{/if}

      {#each GROUP_ORDER as group (group)}
        {@const rows = data.rows.filter((r) => r.group === group)}
        {#if rows.length}
          <section>
            <h3 class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
              {GROUP_LABELS[group]}
            </h3>
            <ul class="divide-y divide-line-subtle">
              {#each rows as r (r.id)}
                <SecretsRow row={r} onClear={(row) => void clearOne(row)} {busy} />
              {/each}
            </ul>
          </section>
        {/if}
      {/each}

      {#if !data.rows.length}
        <p class="text-xs text-muted">
          Nothing configured yet. Secrets appear here as you add them — a provider key on Models, an integration,
          an agent secret.
        </p>
      {/if}
    </div>
  {/if}
</Panel>
