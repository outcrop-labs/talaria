<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { errorMessage, getJson, patchJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'

  // A repository's project variables: write-only values, sealed at rest and
  // placed in the container of EVERY agent that works in the repository, so
  // they belong to the repository and not to the agent this opened from.
  // Keys arrive from the server; values never do. Saving sends {set: [...]}
  // for a new pair with a value and {delete: [...]} for removed keys. The
  // key list is the store's own read (GET /api/workbench/env/{repo}); both
  // require agents.manage, and a refusal is said here.
  let {
    open,
    onClose,
    repo,
  }: {
    open: boolean
    onClose: () => void
    /** owner/name. */
    repo: string
  } = $props()

  const qc = useQueryClient()
  const envKey = $derived(['workbench-env', repo])
  const stored = createQuery(() => ({
    queryKey: envKey,
    queryFn: async (): Promise<string[]> => (await getJson<{ keys: string[] }>(`/api/workbench/env/${repo}`)).keys,
    enabled: open,
  }))
  let removed = $state<string[]>([])
  let name = $state('')
  let value = $state('')
  let busy = $state(false)
  let error = $state<string | null>(null)

  const nameOk = $derived(/^[A-Z][A-Z0-9_]*$/.test(name))
  const adding = $derived(name !== '' && value !== '')
  const dirty = $derived(removed.length > 0 || adding)

  const close = () => {
    removed = []
    name = ''
    value = ''
    error = null
    onClose()
  }

  const save = async () => {
    busy = true
    error = null
    try {
      await patchJson(`/api/workbench/env/${repo}`, { set: adding ? [{ key: name, value }] : [], delete: removed })
      await qc.invalidateQueries({ queryKey: envKey })
      close()
    } catch (e) {
      error = errorMessage(e)
    }
    busy = false
  }
</script>

<Modal {open} onClose={close} title="Project variables" width="max-w-lg">
  <div class="space-y-5">
    <p class="font-sans text-[13px] leading-5 text-muted">
      For every agent that works in <span class="font-mono text-fg">{repo}</span>. Values are sealed at rest, placed in the agent's container, and never shown
      again. Project keys only, never org credentials.
    </p>
    {#if stored.isPending}
      <SkeletonRows rows={1} />
    {:else if stored.isError}
      <p class="font-sans text-[13px] leading-5 text-danger" role="alert">Could not read its variables: {errorMessage(stored.error)}</p>
    {:else}
      <div class="flex flex-wrap items-center gap-1.5">
        {#each (stored.data ?? []).filter((k) => !removed.includes(k)) as k (k)}
          <Chip class="font-mono" title="Remove this variable" onRemove={() => (removed = [...removed, k])}>{k}</Chip>
        {:else}
          <span class="font-sans text-[13px] leading-5 text-muted">None yet.</span>
        {/each}
      </div>
    {/if}
    <div class="grid gap-4 sm:grid-cols-2">
      <SettingsField label="Name" for="project-variable-key">
        <Input
          id="project-variable-key"
          size="sm"
          class="font-mono"
          placeholder="KEY"
          value={name}
          oninput={(e) => (name = e.currentTarget.value.toUpperCase().replace(/[^A-Z0-9_]/g, '_'))}
        />
      </SettingsField>
      <SettingsField label="Value" for="project-variable-value">
        <Input id="project-variable-value" size="sm" class="font-mono" type="password" placeholder="value" bind:value />
      </SettingsField>
    </div>
    {#if name && !nameOk}<p class="font-sans text-[13px] leading-5 text-muted">Capital letters, digits and underscores, starting with a letter.</p>{/if}
    {#if error}<p transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-danger" role="alert">{error}</p>{/if}
  </div>
  {#snippet footer()}
    <div class="flex items-center justify-end gap-2">
      <Button size="sm" variant="ghost" onclick={close}>Cancel</Button>
      <Button size="sm" disabled={busy || !dirty || (name !== '' && !nameOk)} onclick={() => void save()}>{busy ? 'Saving' : 'Save variables'}</Button>
    </div>
  {/snippet}
</Modal>
