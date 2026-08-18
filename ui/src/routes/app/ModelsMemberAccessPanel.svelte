<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { getJson } from '@/lib/fetch-json'
  import { useModels } from '@/lib/muse.svelte'
  import CapabilityTags from '@/components/models/CapabilityTags.svelte'
  import { useModelCapabilities } from '@/components/models/fitness-queries'

  // Which models MEMBERS may pick (preferred model, muse drafting). Admins are
  // never restricted; an empty allowlist means everything is open. This is how
  // admins keep the expensive/powerful brains for deliberate use.
  const qc = useQueryClient()
  const catalogQuery = useModels()
  const capsQuery = useModelCapabilities()
  // `['admin-settings']` is ONE cache entry shared with Admin.svelte's Organization
  // and Settings panels. Declare the whole payload, not a private slice of it,
  // so all three readers agree on what that entry holds (Admin.svelte carries the
  // same interface next to its `useAdminSettings` hook).
  interface AdminSettings {
    auditRetentionDays: number
    org: { name: string; about: string }
    memberModels: string[]
  }
  const settingsQuery = createQuery(() => ({
    queryKey: ['admin-settings'],
    queryFn: (): Promise<AdminSettings> => getJson<AdminSettings>('/api/admin/settings'),
  }))
  const settings = $derived(settingsQuery.data)
  const models = $derived((catalogQuery.data?.models ?? []).filter((m) => !m.qualified))
  const rawSaved = $derived(settings?.memberModels ?? [])
  // Models removed from the registry drop out of the list here (and get
  // persisted out on the next save) — the allowlist tracks reality. Guarded
  // on the catalog having loaded so a slow fetch can't wipe the selection.
  const registered = $derived(new Set(models.map((m) => m.id)))
  const saved = $derived(models.length ? rawSaved.filter((id) => registered.has(id)) : rawSaved)
  const pruned = $derived(rawSaved.length - saved.length)
  // Restriction MODE is its own state — it can't derive from the selection,
  // or toggling it on with nothing selected could never stick.
  let modeOverride = $state<boolean | null>(null)
  let draft = $state<string[] | null>(null)
  const restricted = $derived(modeOverride ?? saved.length > 0)
  const selection = $derived(draft ?? saved)
  // What would be saved right now: the selection when limiting, [] when open.
  const effective = $derived(restricted ? selection : [])
  const dirty = $derived(JSON.stringify([...effective].sort()) !== JSON.stringify([...rawSaved].sort()))

  const save = async () => {
    await fetch('/api/admin/settings', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ memberModels: effective }),
    })
    modeOverride = null
    draft = null
    await qc.invalidateQueries({ queryKey: ['admin-settings'] })
    await qc.invalidateQueries({ queryKey: ['gateway-models'] })
  }
  const toggle = (id: string) => {
    draft = selection.includes(id) ? selection.filter((x) => x !== id) : [...selection, id]
  }
</script>

<Panel>
  <SectionHeader
    title="Member access"
    info="Which models non-admins may pick for AI drafting and as their preferred model. Keep the expensive ones for deliberate, admin-configured use. Agents' own brains are set per agent and unaffected."
  />
  {#if catalogQuery.isPending || settingsQuery.isPending}
    <!-- The restrict toggle and the list both seed from these queries — hold
         them with skeletons so the checkbox never flips after load. -->
    <div class="space-y-3">
      <div class="flex items-center gap-2">
        <Skeleton class="h-4 w-4" />
        <Skeleton class="h-3 w-56 rounded-full" />
      </div>
      <SkeletonRows rows={3} />
    </div>
  {:else if !settings}
    <!-- Without the saved allowlist this panel renders an unchecked "Limit
         members" box and says "All registered models are available to
         members" — the exact opposite of a restrictive policy it failed to read. -->
    <QueryError
      variant="compact"
      error={settingsQuery.error}
      title="Could not load member access"
      onRetry={() => void settingsQuery.refetch()}
    />
  {:else}
    <Checkbox
      class="mb-3 gap-2 text-sm text-fg"
      checked={restricted}
      onChange={(checked) => (modeOverride = checked)}
      label="Limit members to selected models"
    />
    {#if restricted}
      <div class="space-y-1 rounded-lg border border-line p-2">
        {#each models as m (m.id)}
          <label class="flex cursor-pointer items-start gap-2 rounded-md px-2 py-1.5 transition-colors hover:dither-fill">
            <input
              type="checkbox"
              checked={selection.includes(m.id)}
              onchange={() => toggle(m.id)}
              class="mt-1 shrink-0 accent-accent"
            />
            <span class="min-w-0">
              <span class="block truncate font-sans text-sm text-fg">{m.label ?? m.id}</span>
              <span class="block truncate font-sans text-xs text-muted">{m.blurb || m.id}</span>
              <!-- What is measured about each model members could be handed.
                   A capability recorded FALSE here is the difference between a
                   member picking a brain that works and one that quietly
                   returns nothing on every structured surface. -->
              <CapabilityTags class="mt-0.5" row={capsQuery.data?.models.find((c) => c.id === m.id)} />
            </span>
          </label>
        {/each}
        {#if models.length === 0}<EmptyState variant="inline" class="px-2 py-1.5" title="No models registered yet." />{/if}
      </div>
    {/if}
    <div class="mt-3 flex items-center gap-2">
      <span class="text-xs text-muted">
        {restricted
          ? selection.length === 0
            ? 'Pick at least one model members may use'
            : `${selection.length} model${selection.length === 1 ? '' : 's'} available to members`
          : 'All registered models are available to members'}{pruned > 0 ? ` · ${pruned} unregistered model${pruned === 1 ? '' : 's'} will drop off on save` : ''}
      </span>
      <span class="ml-auto"></span>
      <Button size="sm" onclick={() => void save()} disabled={!dirty || (restricted && selection.length === 0)}>
        Save
      </Button>
    </div>
  {/if}
</Panel>
