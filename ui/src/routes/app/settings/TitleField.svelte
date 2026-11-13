<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import { inlineEditKeys } from '@/components/ui/control'
  import { putJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'
  import { useProfilePrefs } from '@/lib/muse.svelte'

  // The person's job title — shown under their name on the Comms profile
  // drawer. Blank clears it (the server trims, ≤ 80 chars).
  const qc = useQueryClient()
  const prefsQuery = useProfilePrefs()
  const saved = $derived(prefsQuery.data?.title ?? '')
  const autosave = useAutosave()
  let title = $state('')
  let busy = $state(false)

  $effect(() => {
    title = saved
  })

  const dirty = $derived(title.trim() !== saved)

  const save = async () => {
    if (!dirty) return
    busy = true
    try {
      await putJson<{ ok: true }>('/api/me', { title: title.trim() || null })
      await qc.invalidateQueries({ queryKey: ['profile-prefs'] })
      await qc.invalidateQueries({ queryKey: ['users'] })
      autosave.flash()
    } catch (e) {
      toastError('Could not save your title', e)
    } finally {
      busy = false
    }
  }
</script>

<SettingsRow label="Title" hint="Shown on your profile in Comms." as="label">
  {#if prefsQuery.isPending}
    <Skeleton class="h-9 w-[280px]" />
  {:else if prefsQuery.isError}
    <QueryError
      variant="inline"
      error={prefsQuery.error}
      title="Could not load your title"
      onRetry={() => void prefsQuery.refetch()}
    />
  {:else}
    <Input
      size="sm"
      class="w-[280px]"
      bind:value={title}
      maxlength={80}
      disabled={busy}
      onblur={() => void save()}
      onkeydown={inlineEditKeys(() => (title = saved))}
      placeholder="Product designer"
    />
  {/if}
</SettingsRow>
