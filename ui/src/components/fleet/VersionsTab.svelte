<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { stringify as stringifyYaml } from 'yaml'
  import { Undo2 } from '@lucide/svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Generating from '@/components/ui/Generating.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { cn } from '@/lib/cn'
  import { getList, postJson } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'
  import type { AgentDef } from '@/lib/fleet-defs'
  import { listStagger } from '@/lib/motion'
  import { toastError } from '@/lib/toast.svelte'
  import InternalEditorModal from './InternalEditorModal.svelte'

  interface Version {
    version: number
    note: string | null
    createdBy: string | null
    createdAt: string
  }

  let { def, canManage }: { def: AgentDef; canManage: boolean } = $props()

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
      await postJson<{ ok: true; version: number; created: boolean }>(`/api/fleet/defs/${def.id}/versions`, { revertTo: v })
      await qc.invalidateQueries({ queryKey: ['agent-versions', def.id] })
      await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
    } catch (e) {
      toastError('Revert failed', e)
    } finally {
      busy = null
    }
  }
</script>

<!-- The header row renders in every state: only the list below it swaps
     between skeleton, error, empty, and rows. -->
<div class="space-y-4">
  <div class="flex items-center gap-4">
    <p class="min-w-0 flex-1 font-sans text-[13px] leading-5 text-muted">
      Reverting publishes the old content as a new version, so nothing is lost.
    </p>
    <Button variant="outline" size="sm" class="shrink-0" onclick={() => (configOpen = true)}>Configuration history</Button>
  </div>
  {#if busy !== null}
    <Generating site="fleet/version-revert" label={`Reverting to version ${busy} and publishing it as a new version`} lines={2} />
  {/if}
  {#if configOpen}
    <InternalEditorModal
      open
      nested
      onClose={() => (configOpen = false)}
      title={`${def.displayName} · config`}
      subtitle="The rendered model and tool config of each version. Pick a revision to see what changed."
      value={stringifyYaml(def.latest?.config ?? {})}
      editable={false}
      onSave={() => {}}
      history={{ kind: 'config', id: def.id }}
      mode="plain"
    />
  {/if}
  <QueryState {query} errorTitle="Could not load version history" errorVariant="compact">
    {#snippet skeleton()}<SkeletonRows rows={4} class="py-3" />{/snippet}
    {#snippet empty()}<p class="font-sans text-[13px] leading-5 text-muted">No version history.</p>{/snippet}
    {#snippet children(versions)}
      <div use:listStagger>
        <SettingsGroup>
          {#each versions as v (v.version)}
            <div class="flex min-h-11 items-center gap-4 px-5 py-1.5">
              <span class="w-10 shrink-0 font-mono text-xs text-muted">v{v.version}</span>
              <span class="flex min-w-0 flex-1 items-center gap-2">
                <span class={cn('min-w-0 truncate font-sans text-[13px] leading-5', v.note ? 'text-fg' : 'text-muted')} title={v.note ?? undefined}>
                  {v.note ?? 'No note'}
                </span>
                {#if v.version === def.currentVersion}<Chip>Current</Chip>{/if}
              </span>
              <span class="shrink-0 font-sans text-[13px] leading-5 text-muted">{v.createdBy ?? 'System'} · {relativeTime(v.createdAt)}</span>
              <!-- A fixed column, so the meta beside it ends at one edge whether or not the row has a menu. -->
              <span class="flex w-8 shrink-0 justify-end">
                {#if v.version !== def.currentVersion && canManage}
                  <SettingsRowMenu
                    label={`Actions for version ${v.version}`}
                    items={[{ label: busy === v.version ? 'Reverting' : 'Revert to this version', icon: [Undo2, { size: 14 }], disabled: busy !== null, onSelect: () => void revert(v.version) }]}
                  />
                {/if}
              </span>
            </div>
          {/each}
        </SettingsGroup>
      </div>
    {/snippet}
  </QueryState>
</div>
