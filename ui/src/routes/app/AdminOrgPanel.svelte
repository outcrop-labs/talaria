<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SaveAction from '@/components/admin/SaveAction.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import { putJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'
  import { useAdminSettings } from './admin'

  // The business every agent works for: its name and what it does. Saving
  // rewrites each agent's identity in place, so the two fields share one
  // explicit Save: a typo must not reach the whole fleet when a field loses
  // focus.
  const qc = useQueryClient()
  const query = useAdminSettings()
  const data = $derived(query.data)
  let name = $state<string | null>(null)
  let about = $state<string | null>(null)
  const savedFlash = useSavedFlash()
  const nameVal = $derived(name ?? data?.org.name ?? '')
  const aboutVal = $derived(about ?? data?.org.about ?? '')
  const dirty = $derived(nameVal !== (data?.org.name ?? '') || aboutVal !== (data?.org.about ?? ''))

  let busy = $state(false)

  const save = async () => {
    if (!dirty || busy) return
    busy = true
    try {
      await putJson<{ ok: true }>('/api/admin/settings', { org: { name: nameVal, about: aboutVal } })
    } catch (e) {
      // Keep the edited identity in the form — flashing "Saved" over a failed
      // PUT would pretend every agent's identity had just been rewritten.
      toastError('Save failed', e)
      return
    } finally {
      busy = false
    }
    name = null
    about = null
    savedFlash.flash()
    await qc.invalidateQueries({ queryKey: ['admin-settings'] })
  }
</script>

<!-- QueryState's error branch holds a failed read: the identity form used to
     render BLANK there. Typing one word into it made it dirty, and a save then
     wrote an empty business name and description over the real ones. -->
<QueryState query={query} errorTitle="Could not load your organization" errorVariant="compact">
  {#snippet skeleton()}
    <SettingsRow label="Business name"><Skeleton class="h-9 w-[280px]" /></SettingsRow>
    <SettingsRow label="What the business does" stacked><Skeleton class="h-16 w-full" /></SettingsRow>
  {/snippet}
  {#snippet children(_data)}
    <SettingsRow
      label="Business name"
      hint="What your agents call the business they work for."
      info="Agents pick up a change from their next new conversation. A conversation already under way keeps the name it started with."
      as="label"
    >
      <Input
        size="sm"
        class="w-[280px]"
        value={nameVal}
        oninput={(e) => (name = e.currentTarget.value)}
        placeholder="Outcrop Labs"
      />
    </SettingsRow>
    <SettingsRow
      label="What the business does"
      hint="One or two sentences your agents use to understand their work."
      stacked
      as="label"
    >
      <Textarea
        rows={2}
        value={aboutVal}
        oninput={(e) => (about = e.currentTarget.value)}
        placeholder="A ten-person freight brokerage moving truckload freight across the Mountain West."
        class="w-full text-sm"
      />
    </SettingsRow>
    <div class="flex min-h-[60px] items-center gap-3 px-5 py-3">
      <span class="min-w-0 flex-1 font-sans text-[13px] leading-5 text-muted">
        {#if savedFlash.saved}<span class="text-success">Saved.</span>{:else}Saving rewrites every agent's identity, so these two save together.{/if}
      </span>
      <SaveAction {dirty} {busy} onclick={() => void save()}>Save</SaveAction>
    </div>
  {/snippet}
</QueryState>
