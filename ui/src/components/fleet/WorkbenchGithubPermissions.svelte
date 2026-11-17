<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Select from '@/components/ui/Select.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import {
    addOptions,
    fromOf,
    heldSentence,
    heldState,
    impactNote,
    setRows,
    withPermission,
    withoutPermission,
    type AgentGithubPermissions,
    type HostPermissions,
    type PermissionScope,
  } from '@/lib/agent-github-permissions'
  import { getJson, putJsonOr } from '@/lib/fetch-json'
  import type { PermissionLevel, PermissionMap } from '@/lib/git-permissions'
  import { pushToast, toastError } from '@/lib/toast.svelte'

  // What this agent's GitHub token carries on each host: its WHOLE set there
  // (talaria-git-hosts' permissions), applying to every repo it is granted on
  // that host, or the default (contents and pull requests to write, metadata
  // to read). An admin sets any key GitHub's App catalog knows, at any level
  // GitHub takes it at; the agent's managers read it. Nothing here is a gate:
  // what the installation does not hold is said, with why, and still
  // settable. Each change is the whole next set, written as a compare-and-set
  // from what this panel read, and applied on change.
  let { agentId }: { agentId: string } = $props()

  const qc = useQueryClient()
  const queryKey = $derived(['workbench-github-permissions', agentId])
  const query = createQuery(() => ({
    queryKey,
    queryFn: (): Promise<AgentGithubPermissions> => getJson<AgentGithubPermissions>(`/api/workbench/github-permissions/${agentId}`),
  }))

  interface Saved {
    ok?: true
    notes?: string[]
    /** A 409's: what the agent carries now. */
    current?: PermissionMap
    error?: string
  }

  let saving = $state<string | null>(null)
  const write = async (h: HostPermissions, to: PermissionMap | null) => {
    saving = h.host
    try {
      const res = await putJsonOr<Saved>(`/api/workbench/github-permissions/${agentId}`, { host: h.host, from: fromOf(h), to }, [409])
      if (res.current) {
        pushToast({
          title: 'These permissions changed since you read them',
          body: 'Showing what is set now. Make your change again if it still applies.',
        })
      } else if (res.notes?.length) {
        pushToast({ title: `Saved GitHub permissions on ${h.name}`, body: res.notes.join(' ') })
      }
    } catch (e) {
      toastError('Could not save GitHub permissions', e)
    } finally {
      // Busy until the re-read lands: an edit made against the set from
      // before this write would send a stale `from` and come back a 409.
      await qc.invalidateQueries({ queryKey })
      saving = null
    }
  }

  const SCOPES: Array<[PermissionScope, string]> = [
    ['repository', 'Repository'],
    ['organization', 'Organization'],
  ]
</script>

<QueryState {query} errorTitle="Could not load GitHub permissions" errorVariant="inline">
  {#snippet skeleton()}<div class="mt-3"><SkeletonRows rows={2} /></div>{/snippet}
  {#snippet children(d)}
    {#each d.hosts as h (h.host)}
      {@const rows = setRows(h.permissions, d.catalog)}
      {@const count = Object.keys(h.permissions ?? {}).length}
      {@const busy = saving === h.host}
      <div class="mt-3 space-y-2 rounded-xl border border-line-subtle p-3">
        <div class="flex items-center gap-1.5">
          <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{h.kind === 'github' ? 'GitHub' : 'Git'} permissions on {h.name}</span>
          {#if h.narrowing === 'perRepo'}
            <InfoTip
              text="What this agent's GitHub token carries on every repo it may work on this host: git push, and the GitHub API it calls with the same token. Only what the App's installation holds rides on the token."
            />
          {/if}
          {#if h.isDefault}<Chip class="normal-case">default</Chip>{/if}
          {#if d.canEdit && h.narrowing === 'perRepo' && !h.isDefault}
            <Button variant="link" size="sm" class="ml-auto" disabled={busy} onclick={() => void write(h, null)}>Reset to default</Button>
          {/if}
        </div>
        {#if h.narrowing === 'hostWide'}
          <p class="text-xs text-muted">This host's own token decides: {h.hostWide}</p>
        {:else}
          {#each SCOPES as [scope, label] (scope)}
            {#if rows[scope].length > 0}
              <div class="space-y-1">
                <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{label}</span>
                <div class="flex flex-wrap items-center gap-1.5">
                  {#each rows[scope] as r (r.key)}
                    {@const state = heldState(h, r.key, r.level)}
                    {@const impact = impactNote(r.key)}
                    <span class="inline-flex items-center gap-1 rounded-md border border-line-subtle bg-raised/40 px-1.5 py-0.5">
                      <Chip
                        class="normal-case"
                        tone={state.kind === 'held' || state.kind === 'unknown' ? 'neutral' : 'warn'}
                        title={heldSentence(state) ?? 'The installation holds this.'}
                        onRemove={d.canEdit && count > 1 && !busy ? () => void write(h, withoutPermission(h.permissions ?? {}, r.key)) : undefined}
                      >
                        {r.key}
                      </Chip>
                      {#if d.canEdit && r.levels.length > 1}
                        <Select
                          size="sm"
                          class="w-20 text-xs"
                          value={r.level}
                          disabled={busy}
                          onchange={(e) => void write(h, withPermission(h.permissions ?? {}, r.key, (e.currentTarget as HTMLSelectElement).value as PermissionLevel))}
                        >
                          {#each r.levels as l (l)}<option value={l}>{l}</option>{/each}
                        </Select>
                      {:else}
                        <span class="font-mono text-[11px] text-muted">{r.level}</span>
                      {/if}
                      {#if impact}<InfoTip text={impact} />{/if}
                    </span>
                  {/each}
                </div>
              </div>
            {/if}
          {/each}
          <!-- Why a key does not ride, said once per key: no gate, just what
               GitHub still has to do. -->
          {#each [...rows.repository, ...rows.organization] as r (r.key)}
            {@const state = heldState(h, r.key, r.level)}
            {#if state.kind === 'appMissing' || state.kind === 'awaitingApproval' || state.kind === 'notHeld'}
              <p class="text-xs text-warning">
                <span class="font-mono">{r.key}: {r.level}</span>. {heldSentence(state)}
                {#if state.kind !== 'appMissing'}
                  {#each state.installations.filter((i) => i.url) as i (i.id)}
                    <a href={i.url} target="_blank" rel="noreferrer" class="ml-1 text-accent hover:underline">{i.account || i.id} on GitHub</a>
                  {/each}
                {/if}
              </p>
            {/if}
          {/each}
          {#if [...rows.repository, ...rows.organization].some((r) => heldState(h, r.key, r.level).kind === 'unknown')}
            <p class="text-xs text-muted">{heldSentence({ kind: 'unknown' })}</p>
          {/if}
          {#if d.canEdit}
            {@const options = addOptions(h, d.catalog)}
            {#if options.length > 0}
              <Combobox
                size="sm"
                class="w-56"
                disabled={busy}
                placeholder="Add permission"
                triggerLabel="Add permission"
                options={options.map((o) => ({ value: o.key, label: o.key, sub: o.note }))}
                selected={[]}
                onChange={(next) => {
                  const picked = options.find((o) => o.key === next[0])
                  if (picked) void write(h, withPermission(h.permissions ?? {}, picked.key, picked.level))
                }}
              />
            {/if}
          {:else}
            <p class="text-xs text-muted">An admin sets these.</p>
          {/if}
        {/if}
      </div>
    {/each}
  {/snippet}
</QueryState>
