<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import { putJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'
  import { useProfilePrefs } from '@/lib/muse.svelte'

  // The person's job title — shown under their name on the Comms profile
  // drawer. Blank clears it (the server trims, ≤ 80 chars).
  const qc = useQueryClient()
  const prefsQuery = useProfilePrefs()
  const saved = $derived(prefsQuery.data?.title ?? '')
  const savedFlash = useSavedFlash()
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
      savedFlash.flash()
    } catch (e) {
      toastError('Could not save your title', e)
    } finally {
      busy = false
    }
  }
</script>

<div class="mt-5 border-t border-line-subtle pt-4">
  <label for="settings-title" class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Title</label>
  {#if prefsQuery.isPending}
    <div class="flex max-w-md items-center gap-2">
      <Skeleton class="h-11 flex-1" />
      <Skeleton class="h-11 w-20" />
    </div>
  {:else if prefsQuery.isError}
    <QueryError
      variant="inline"
      error={prefsQuery.error}
      title="Could not load your title"
      onRetry={() => void prefsQuery.refetch()}
    />
  {:else}
    <div class="flex max-w-md items-center gap-2">
      <Input
        id="settings-title"
        bind:value={title}
        maxlength={80}
        onkeydown={(e) => e.key === 'Enter' && void save()}
        placeholder="What you do, e.g. Product designer"
      />
      <Button onclick={() => void save()} disabled={busy || !dirty}>Save</Button>
    </div>
  {/if}
  {#if savedFlash.saved}<div class="mt-2 text-xs text-success">Saved</div>{/if}
  <p class="mt-1 text-xs text-muted">Shown on your profile in Comms.</p>
</div>
