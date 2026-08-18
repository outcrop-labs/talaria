<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { stringify as stringifyYaml } from 'yaml'
  import { RotateCcw } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Generating from '@/components/ui/Generating.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { cn } from '@/lib/cn'
  import { getList } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import type { AgentDef } from '@/lib/fleet-defs'
  import { listStagger } from '@/lib/motion'
  import InternalEditorModal from './InternalEditorModal.svelte'

  interface Version {
    version: number
    note: string | null
    createdBy: string | null
    createdAt: string
  }

  let { def, isAdmin }: { def: AgentDef; isAdmin: boolean } = $props()

  const qc = useQueryClient()
  // 404 stays a failure: this tab only renders for a def we are already showing,
  // so "no such agent" means something is wrong, not "no history".
  const query = createQuery(() => ({
    queryKey: ['agent-versions', def.id],
    queryFn: (): Promise<Version[]> => getList<Version>(`/api/fleet/defs/${def.id}/versions`, 'versions'),
  }))
  let busy = $state<number | null>(null)
  let configOpen = $state(false)
  const revert = async (v: number) => {
    if (!(await confirm({ title: 'Revert version', message: `Revert ${def.displayName} to v${v}? This publishes it as a new version.`, confirmLabel: 'Revert' }))) return
    busy = v
    try {
      await fetch(`/api/fleet/defs/${def.id}/versions`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ revertTo: v }) })
      await qc.invalidateQueries({ queryKey: ['agent-versions', def.id] })
      await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
    } finally {
      busy = null
    }
  }
</script>

<!-- The header row (History + Config history) renders in every state — only
     the list below it swaps between skeleton, error, empty, and rows. -->
<div class="divide-y divide-line">
  {#if busy !== null}
    <div class="pb-2.5">
      <Generating site="fleet/version-revert" label={`Reverting to v${busy}, publishing as a new version`} lines={2} />
    </div>
  {/if}
  <div class="flex items-center gap-1.5 pb-2.5">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">History</span>
    <InfoTip text="Reverting never destroys anything — the old content is published as a NEW version on top." />
    <Button variant="outline" size="sm" class="ml-auto" onclick={() => (configOpen = true)}>
      Config history
    </Button>
  </div>
  {#if configOpen}
    <InternalEditorModal
      open
      nested
      onClose={() => (configOpen = false)}
      title={`${def.displayName} · config`}
      subtitle="The rendered model/tool config per version. Click a revision to see what changed."
      value={stringifyYaml(def.latest?.config ?? {})}
      editable={false}
      onSave={() => {}}
      history={{ kind: 'config', id: def.id }}
      mode="plain"
    />
  {/if}
  <QueryState {query} errorTitle="Could not load version history" errorVariant="compact">
    {#snippet skeleton()}<SkeletonRows rows={4} class="py-3" />{/snippet}
    {#snippet empty()}<div class="py-2.5 text-sm text-muted">No version history.</div>{/snippet}
    {#snippet children(versions)}
      <div use:listStagger>
      {#each versions as v (v.version)}
        <div class="flex items-center gap-3 py-2.5 text-sm transition-colors dither-fill">
          <span class={cn('w-12 shrink-0 font-mono', v.version === def.currentVersion ? 'text-accent' : 'text-muted')}>v{v.version}</span>
          <span class="min-w-0 flex-1 truncate font-sans text-fg">{v.note ?? '—'}</span>
          <span class="shrink-0 font-mono text-[11px] text-muted">{v.createdBy ?? 'system'} · {relativeTime(v.createdAt)}</span>
          {#if isAdmin && v.version !== def.currentVersion}
            <Button variant="ghost" size="xs" class="shrink-0 gap-1 hover:text-accent" disabled={busy !== null} onclick={() => void revert(v.version)}>
              <RotateCcw size={12} /> {busy === v.version ? 'reverting' : 'revert'}
            </Button>
          {/if}
          {#if v.version === def.currentVersion}<span class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-success">current</span>{/if}
        </div>
      {/each}
      </div>
    {/snippet}
  </QueryState>
</div>
