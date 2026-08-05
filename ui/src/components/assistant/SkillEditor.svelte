<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { Trash2 } from '@lucide/svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import InternalEditorModal from '@/components/fleet/InternalEditorModal.svelte'
  import { getJson } from '@/lib/fetch-json'
  import { type Assistant } from '@/lib/assistant'

  // The full workspace editor (history + diff + restore) for one owned skill.
  let {
    assistant,
    name,
    onClose,
    onChanged,
  }: {
    assistant: Assistant
    name: string
    onClose: () => void
    onChanged: () => void
  } = $props()

  let busy = $state(false)
  // This used to hand back whatever the body was, status ignored: a 500 whose
  // body is `{ error }` became `{ content: undefined }` and a non-JSON body
  // became `{ content: '' }` — either way the editor mounted EMPTY, and the
  // first Ctrl+S wrote that emptiness over the real SKILL.md.
  const query = createQuery(() => ({
    queryKey: ['assistant-skill', assistant.slug, name],
    queryFn: (): Promise<{ content: string }> => getJson<{ content: string }>(`/api/skills/${assistant.slug}/${name}`),
  }))

  const save = async (md: string) => {
    busy = true
    try {
      await fetch(`/api/skills/${assistant.slug}/${name}`, {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ content: md }),
      })
      onChanged()
    } finally {
      busy = false
    }
  }

  const remove = async () => {
    if (!(await confirm({ title: 'Remove skill', message: `Remove the "${name}" skill?`, confirmLabel: 'Remove', danger: true }))) return
    await fetch(`/api/skills/${assistant.slug}/${name}`, { method: 'DELETE', credentials: 'same-origin' })
    onChanged()
    onClose()
  }
</script>

<!-- The editor seeds once from `value`, so it can't mount before the content
     arrives — but the click must land NOW: show the same modal shell with a
     prose-bar body, then swap in the real editor. -->
{#if query.data === undefined}
  <Modal open {onClose} title={`${name} · SKILL.md`} width="max-w-6xl">
    <div class="h-[76vh] pt-2">
      {#if query.isError}
        <QueryError error={query.error} title={`Could not open ${name}`} onRetry={() => void query.refetch()} />
      {:else}
        <SkeletonRows rows={8} />
      {/if}
    </div>
  </Modal>
{:else}
  <InternalEditorModal
    open
    {onClose}
    title={`${name} · SKILL.md`}
    subtitle="A playbook your assistant follows. Edits are live on its next run."
    value={query.data.content}
    editable
    saving={busy}
    onSave={save}
    history={{ kind: 'skill', owner: assistant.slug, name }}
    muse={{ kind: 'skill', context: `A skill for ${assistant.displayName}, a personal AI assistant.` }}
  >
    {#snippet footerExtra()}
      <Button variant="ghost" size="sm" onclick={() => void remove()}>
        <Trash2 size={14} class="mr-1.5" /> Delete skill
      </Button>
    {/snippet}
  </InternalEditorModal>
{/if}
