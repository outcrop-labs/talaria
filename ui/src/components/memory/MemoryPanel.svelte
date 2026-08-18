<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { submitOnEnter } from '@/components/ui/control'
  import InternalEditorModal from '@/components/fleet/InternalEditorModal.svelte'
  import { fade } from '@/lib/motion'
  import { appendMemoryNote, memoryKey, saveMemory, useMemory } from '@/lib/memory'

  // WHAT AN AGENT REMEMBERS — one document, read live from its container.
  //
  // A memory is NOT a library: there is one document and nothing to pick, so
  // this is not a LibraryPane. What it does share with the skills work is the
  // shape underneath — one client, one editor path, one set of query keys —
  // because this too existed twice (personal assistant, fleet agent) with
  // divergent keys, divergent error stories, and saves that ignored their own
  // response.
  //
  // The quick-add field came from the fleet copy and the assistant never had
  // it; there is no reason a personal assistant should be harder to teach than
  // a fleet agent, so it is here for both.
  let {
    id,
    label,
    canEdit = true,
    museContext,
    /** Extra sentence for the tooltip — the fleet names the container. */
    note,
    /** The editor opens from inside another modal. */
    nested = false,
    /** Shown INSTEAD of an error when the owner is simply not running, which is
     *  an explainable state rather than a fault. */
    offline,
  }: {
    id: string
    label: string
    canEdit?: boolean
    museContext: string
    note?: string
    nested?: boolean
    offline?: { title: string; hint: string } | null
  } = $props()

  const qc = useQueryClient()
  const query = useMemory(() => id)
  let editing = $state(false)
  let busy = $state(false)
  let noteDraft = $state('')
  let failure = $state<unknown>(null)

  const save = async (content: string) => {
    busy = true
    failure = null
    try {
      await saveMemory(id, content)
      await qc.invalidateQueries({ queryKey: memoryKey(id) })
    } catch (e) {
      // Previously invisible: both copies discarded the response, so a refused
      // write closed the editor as though it had landed.
      failure = e
    } finally {
      busy = false
    }
  }

  const addNote = async () => {
    const n = noteDraft.trim()
    // Appending before the read lands would write over the whole document with
    // one line.
    if (!n || !query.data) return
    busy = true
    failure = null
    try {
      await appendMemoryNote(id, query.data.content, n, new Date().toISOString().slice(0, 10))
      await qc.invalidateQueries({ queryKey: memoryKey(id) })
      noteDraft = ''
    } catch (e) {
      failure = e
    } finally {
      busy = false
    }
  }

  const tip = $derived(
    `${note ? `${note} ` : ''}The agent curates this itself, so a concurrent agent write can win over a manual edit. Every save is snapshotted and revertible.`,
  )
</script>

{#if query.isError && offline}
  <EmptyState icon="◌" title={offline.title} hint={offline.hint} />
{:else if query.isError}
  <QueryError error={query.error} title="Can't reach the agent" onRetry={() => void query.refetch()} />
{:else}
  <div class="space-y-3">
    <div class="flex items-center gap-1.5">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Memory</span>
      <InfoTip text={tip} />
      {#if canEdit}
        <Button size="sm" class="ml-auto shrink-0" disabled={!query.data} onclick={() => (editing = true)}>
          Edit
        </Button>
      {/if}
    </div>

    {#if canEdit}
      <div class="flex items-center gap-2">
        <Input
          size="sm"
          bind:value={noteDraft}
          onkeydown={submitOnEnter(() => void addNote())}
          placeholder="Add a memory — one fact the agent should keep"
          class="flex-1"
          aria-label="Add a memory"
        />
        <Button size="sm" variant="outline" disabled={busy || !query.data || !noteDraft.trim()} onclick={() => void addNote()}>
          Add
        </Button>
      </div>
    {/if}

    {#if !query.data}
      <!-- The document is coming — prose-bar shimmer where it will render. -->
      <div class="space-y-3 pt-1" aria-hidden="true">
        {#each [0, 1, 2, 3, 4, 5] as i (i)}
          <Skeleton class={`h-2.5 rounded-full ${['w-3/4', 'w-full', 'w-5/6', 'w-2/3', 'w-full', 'w-1/2'][i]}`} delay={i * 0.12} />
        {/each}
      </div>
    {:else if query.data.content}
      <div class="max-h-72 overflow-y-auto text-sm">
        <Markdown children={query.data.content} />
      </div>
    {:else}
      <div in:fade={{ duration: 150 }}>
        <EmptyState icon="◌" title="Nothing remembered yet" hint="It writes things down as you work together." />
      </div>
    {/if}

    {#if failure}
      <QueryError variant="inline" error={failure} title="Could not save this memory" />
    {/if}

    {#if editing}
      <InternalEditorModal
        open
        {nested}
        onClose={() => (editing = false)}
        title={`${label} · MEMORY.md`}
        subtitle="The agent maintains this itself; your edits are snapshotted and revertible."
        value={query.data?.content ?? ''}
        editable={canEdit}
        saving={busy}
        onSave={save}
        history={{ kind: 'memory', id }}
        muse={{ kind: 'memory', context: museContext }}
      />
    {/if}
  </div>
{/if}
