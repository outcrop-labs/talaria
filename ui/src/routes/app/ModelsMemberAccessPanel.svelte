<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSaveBar from '@/components/settings/SettingsSaveBar.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { getJson, putJson } from '@/lib/fetch-json'
  import { useEndpoints } from '@/lib/models'
  import { toastError } from '@/lib/toast.svelte'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import { useModels } from '@/lib/muse.svelte'
  import { anyPending } from '@/components/ui/query-state'
  import CapabilityTags from '@/components/models/CapabilityTags.svelte'
  import ModelName from '@/components/models/ModelName.svelte'
  import { useModelCapabilities } from '@/components/models/fitness-queries'

  // Which models MEMBERS may pick (preferred model, muse drafting). Admins are
  // never restricted; an empty allowlist means everything is open. This is how
  // admins keep the expensive/powerful brains for deliberate use.
  const qc = useQueryClient()
  const catalogQuery = useModels()
  const capsQuery = useModelCapabilities()
  // The registry, for the provider column: which provider serves each model.
  // Advisory, so a failed read leaves the column blank and nothing else.
  const endpointsQuery = useEndpoints()
  const providersOf = $derived.by(() => {
    const byModel = new Map<string, string[]>()
    for (const ep of endpointsQuery.data ?? [])
      for (const m of ep.models) byModel.set(m, [...(byModel.get(m) ?? []), ep.name])
    return byModel
  })
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
  // One switch per MODEL, not per endpoint deployment: the allowlist is
  // judged by bare model id (`modelAllowedFor` — allowing "m" allows "ep/m"
  // on every endpoint that serves it), so model granularity is what a save
  // actually has. The catalog spells each model "<endpoint>/<model>" (a bare
  // id survives only as a multi-endpoint POOL), so collapse: strip the
  // endpoint prefix from qualified ids, keep pool ids as they are, dedupe.
  const models = $derived.by(() => {
    const byId = new Map<string, { id: string; label?: string; blurb?: string }>()
    for (const m of catalogQuery.data?.models ?? []) {
      const bare = m.qualified ? m.id.slice(m.id.indexOf('/') + 1) : m.id
      if (!byId.has(bare)) byId.set(bare, { id: bare, label: m.label, blurb: m.blurb })
    }
    return [...byId.values()].sort((a, b) => a.id.localeCompare(b.id))
  })
  // A bare id's capability facts: the POOL row when one exists (its facts are
  // merged across every endpoint serving the model), else the first qualified
  // row for the same model. Advisory display, same as every other picker.
  const capsRowFor = (id: string) =>
    capsQuery.data?.models.find((c) => !c.qualified && c.id === id) ??
    capsQuery.data?.models.find((c) => c.qualified && c.id.slice(c.id.indexOf('/') + 1) === id)
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

  let saving = $state(false)
  const savedFlash = useSavedFlash()
  const discard = () => {
    modeOverride = null
    draft = null
  }
  const save = async () => {
    saving = true
    try {
      await putJson<{ ok: true }>('/api/admin/settings', { memberModels: effective })
    } catch (e) {
      // Keep the draft: a failed save must not quietly revert the list.
      toastError('Could not save member access', e)
      return
    } finally {
      saving = false
    }
    discard()
    await qc.invalidateQueries({ queryKey: ['admin-settings'] })
    await qc.invalidateQueries({ queryKey: ['gateway-models'] })
    savedFlash.flash()
  }
  const toggle = (id: string) => {
    draft = selection.includes(id) ? selection.filter((x) => x !== id) : [...selection, id]
  }

  const summary = $derived(
    (restricted
      ? selection.length === 0
        ? 'Pick at least one model members may use.'
        : `Members can pick ${selection.length} of ${models.length} models.`
      : 'Members can pick any registered model.') +
      (pruned > 0 ? ` ${pruned} model${pruned === 1 ? ' that is' : 's that are'} no longer registered will drop off on save.` : ''),
  )
</script>

<SettingsSection
  title="Member access"
  description="The models people who are not admins may pick."
  info="This covers a member's preferred model and AI drafting. Keep the expensive models for deliberate, admin-configured use. Each agent's own model is set on the agent and is not affected."
>
  {#if anyPending(catalogQuery, settingsQuery)}
    <!-- The switch and the list both seed from these queries, so hold them
         with skeletons and the switch never flips after load. Both flags are
         read every time (anyPending, query-state.ts says why). -->
    <SettingsGroup>
      <div class="flex min-h-[56px] items-center gap-6 px-5 py-3.5">
        <Skeleton class="h-3 w-56 rounded-full" />
        <span class="ml-auto"></span>
        <Skeleton class="h-5 w-9 rounded-full" />
      </div>
      <div class="px-5 py-4"><SkeletonRows rows={3} /></div>
    </SettingsGroup>
  {:else if !settings}
    <!-- Without the saved allowlist this panel would show the switch off and
         say every model is open, the exact opposite of a restrictive policy
         it failed to read. -->
    <QueryError
      variant="compact"
      error={settingsQuery.error}
      title="Could not load member access"
      onRetry={() => void settingsQuery.refetch()}
    />
  {:else}
    <!-- One card: the switch, the models it limits to, and the Save that
         makes both live. -->
    <SettingsGroup>
      <SettingsRow label="Limit members to selected models" hint={summary}>
        <Toggle checked={restricted} onChange={(checked) => (modeOverride = checked)} title="Limit members to selected models" />
      </SettingsRow>

      {#if restricted}
        {#if models.length === 0}
          <EmptyState variant="inline" class="px-5 py-4" title="No models registered yet." />
        {:else}
          <div class="overflow-x-auto">
            <table class="w-full border-collapse text-left">
              <thead>
                <tr class="border-b border-line-subtle font-sans text-xs font-medium leading-4 text-muted">
                  <th class="w-[38%] px-5 py-2.5 font-medium">Model</th>
                  <th class="px-3 py-2.5 font-medium">Provider</th>
                  <th class="px-3 py-2.5 font-medium">Capabilities</th>
                  <th class="py-2.5 pl-3 pr-5 text-right font-medium">Members</th>
                </tr>
              </thead>
              <tbody>
                {#each models as m (m.id)}
                  <!-- h-10: at this height a catalog of eleven ends with its Save on the first screen. -->
                  <tr class="h-10 border-b border-line-subtle transition-colors last:border-0 hover:bg-hover">
                    <td class="max-w-0 px-5 py-2" title={m.blurb}><ModelName id={m.id} name={m.label} /></td>
                    <td class="whitespace-nowrap px-3 py-2 font-sans text-[13px] leading-5 text-muted">
                      {(providersOf.get(m.id) ?? []).join(', ')}
                    </td>
                    <!-- What is measured about each model members could be
                         handed. A capability recorded FALSE here is the
                         difference between a member picking a model that
                         works and one that quietly returns nothing on every
                         structured surface, so what a model lacks leads, and
                         is the only thing drawn as a chip. -->
                    <td class="px-3 py-2"><CapabilityTags row={capsRowFor(m.id)} max={3} quiet /></td>
                    <td class="py-2 pl-3 pr-5">
                      <Toggle
                        class="justify-end"
                        checked={selection.includes(m.id)}
                        onChange={() => toggle(m.id)}
                        title={`Members can use ${m.label ?? m.id}`}
                      />
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      {/if}

      <!-- SAVED ON THE BUTTON, not on each switch: the list goes live for
           every member the moment it is stored, so a half-made selection
           must not. -->
      <SettingsSaveBar
        label="Save access"
        reason="Goes live for every member when you save."
        {dirty}
        {saving}
        saved={savedFlash.saved}
        disabled={restricted && selection.length === 0}
        onSave={() => void save()}
        onDiscard={discard}
      />
    </SettingsGroup>
  {/if}
</SettingsSection>
