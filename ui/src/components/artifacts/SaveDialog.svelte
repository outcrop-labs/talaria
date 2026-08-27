<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { useFolders } from '@/lib/artifacts'
  import { errorMessage, postJson } from '@/lib/fetch-json'
  import { p } from '@/router'

  let { src, onClose }: { src: string; onClose: () => void } = $props()

  // A failed folder read used to leave "No folder (root)" as the only option,
  // so the image was filed at the root and the owner had no idea their folders
  // were simply unreadable at that moment.
  const folders = listQuery(useFolders(), { title: 'Could not load your folders', variant: 'inline' })
  const url = new URL(src, window.location.origin)
  const path = url.searchParams.get('path') ?? ''
  const model = decodeURIComponent(url.pathname.split('/').pop() ?? '')
  let title = $state(path.split('/').pop() ?? 'image')
  let folderId = $state('')
  let busy = $state(false)
  let error = $state<string | null>(null)
  let savedId = $state<string | null>(null)

  const save = async () => {
    busy = true
    error = null
    try {
      const j = await postJson<{ artifact?: { id: string } }>(`/api/agent-media/${encodeURIComponent(model)}/save`, {
        path,
        title: title.trim() || undefined,
        folderId: folderId || null,
      })
      if (!j.artifact) throw new Error('save failed')
      savedId = j.artifact.id
    } catch (e) {
      error = errorMessage(e)
    } finally {
      busy = false
    }
  }
</script>

<Modal open {onClose} title="Save to Files">
  {#if savedId}
    <div class="space-y-3">
      <p class="text-sm text-fg">
        Saved. It's a durable file now: versioned, shareable, folder-organized.
      </p>
      <div class="flex justify-end gap-2 border-t border-line-subtle pt-3">
        <a href={`${p('/artifacts')}?a=${savedId}`} class="text-sm text-accent hover:underline" onclick={onClose}>
          Open Files ↗
        </a>
        <Button size="sm" onclick={onClose}>
          Done
        </Button>
      </div>
    </div>
  {:else}
    <div class="space-y-3">
      <div>
        <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Title</label>
        <Input size="sm" autofocus bind:value={title} />
      </div>
      <div>
        <label class="mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Folder</label>
        {#if folders.pending}
          <Skeleton class="h-9 w-full" />
        {:else}
          <Select size="sm" bind:value={folderId} class="w-full">
            <option value="">No folder (root)</option>
            {#each folders.rows as f (f.id)}
              <option value={f.id}>{f.name}</option>
            {/each}
          </Select>
        {/if}
        {#if folders.notice}<QueryError {...folders.notice} />{/if}
      </div>
      {#if error}<div class="text-xs text-danger">{error}</div>{/if}
      <div class="flex justify-end gap-2 border-t border-line-subtle pt-3">
        <Button variant="ghost" size="sm" onclick={onClose}>
          Cancel
        </Button>
        <Button size="sm" onclick={() => void save()} disabled={busy}>
          {busy ? 'Saving' : 'Save'}
        </Button>
      </div>
    </div>
  {/if}
</Modal>
