<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Plus } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import type { SegmentedOption } from '@/components/ui/segmented'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { memoryKey, saveMemory, useMemory, type MemoryFile } from '@/lib/memory'
  import { getJson } from '@/lib/fetch-json'
  import { entryTitle, joinEntries, removeEntry, replaceEntry, searchEntries, splitEntries } from './memory-entries'
  import { cn } from '@/lib/cn'

  // THE SAME FILE, SEEN AS THE THINGS IN IT.
  //
  // A memory is one document on disk and the whole-file editor beside this one
  // is the honest view of that — but an agent that has been running a month
  // has two hundred entries in it, and "scroll until you find the fact that is
  // wrong" is not a way to curate anything. This view reads the file as the
  // §-delimited list Hermes actually keeps and lets a person search it, fix
  // one entry, drop one entry, add one entry.
  //
  // EVERY WRITE STILL WRITES THE WHOLE FILE, joined by `memory-entries.ts`:
  // Hermes' own memory tool refuses to write at all once the file stops
  // reading back as a clean §-delimited list, so a view that edits pieces of
  // it has exactly one job beyond the editing — hand back a clean list. That
  // is why nothing here builds a string by hand.
  //
  // AND THE AGENT WRITES HERE TOO. There is no etag on this document (the
  // route is a read and a whole-file replace through `docker exec`), so a save
  // replaces the file as it stands on disk at that moment. The note below says
  // so out loud, and an edit whose entry changed under it refuses rather than
  // overwriting a sentence nobody read.
  let {
    id,
    canEdit = true,
    class: className,
  }: {
    id: string
    canEdit?: boolean
    class?: string
  } = $props()

  const FILES: SegmentedOption<MemoryFile>[] = [
    { id: 'memory', label: 'Memory', title: 'MEMORY.md — what the agent remembers about its work' },
    { id: 'profile', label: 'Profile', title: 'USER.md — what the agent has learned about you' },
  ]
  const FILENAME: Record<MemoryFile, string> = { memory: 'MEMORY.md', profile: 'USER.md' }

  let file = $state<MemoryFile>('memory')
  let q = $state('')
  let editing = $state<number | null>(null)
  let draft = $state('')
  // The entry as it was when the edit opened. A write addresses an entry by
  // POSITION, so if the agent rewrites the file while a card is open, that
  // position may now hold something else — this is what lets the save notice.
  let opened = $state('')
  let adding = $state(false)
  let fresh = $state('')
  let busy = $state(false)
  let failure = $state<unknown>(null)

  const qc = useQueryClient()
  const query = useMemory(() => id, () => file)

  const content = $derived(query.data?.content ?? '')
  const entries = $derived(splitEntries(content))
  const hits = $derived(searchEntries(entries, q))
  // Keep the file ending the agent wrote; an empty file gets the one a text
  // file should have.
  const trailingNewline = $derived(content.trim() === '' || content.endsWith('\n'))

  const reset = () => {
    editing = null
    draft = ''
    opened = ''
    adding = false
    fresh = ''
    failure = null
  }

  // A different document is a different list: an open editor, and a filter
  // that matched the other file, both belong to the view we just left.
  const switchFile = (next: MemoryFile) => {
    if (next === file) return
    file = next
    q = ''
    reset()
  }

  /** The one write. Takes the next list, hands the file back as a clean
   *  §-delimited list, and reports whether it landed. */
  /** The file AS IT IS ON DISK NOW — the agent writes here too, and the list
   *  this view was opened from may be minutes old. Every write applies its
   *  edit to this, not to the cached read. */
  const freshEntries = async (target: MemoryFile): Promise<string[]> => {
    const r = await getJson<{ content: string }>(`/api/memory/${id}?file=${target}`)
    return splitEntries(r.content ?? '')
  }

  const write = async (mutate: (fresh: string[]) => string[] | null): Promise<boolean> => {
    // Captured, not read later: the switch may move while this is in flight,
    // and the invalidation has to name the document we actually wrote.
    const target = file
    busy = true
    failure = null
    try {
      const next = mutate(await freshEntries(target))
      if (next === null) {
        failure = new Error('That entry is no longer in the file — the agent changed it. Read it again and redo the edit.')
        return false
      }
      await saveMemory(id, joinEntries(next, trailingNewline), target)
      await qc.invalidateQueries({ queryKey: memoryKey(id, target) })
      return true
    } catch (e) {
      failure = e
      return false
    } finally {
      busy = false
    }
  }

  const startEdit = (i: number) => {
    editing = i
    draft = entries[i] ?? ''
    opened = draft
    failure = null
  }

  const saveEdit = async () => {
    if (editing === null || busy) return
    if (draft.trim() === opened.trim()) return reset()
    // The entry is addressed by the TEXT that was opened, against the file as
    // it is now: an index into a cached list would point at whatever the
    // agent moved into that slot since.
    const text = draft
    const was = opened
    if (await write((fresh) => {
      const at = fresh.indexOf(was)
      return at === -1 ? null : replaceEntry(fresh, at, text)
    })) reset()
  }

  const remove = async (i: number) => {
    const entry = entries[i]
    if (!entry || busy) return
    const ok = await confirm({
      title: 'Forget this',
      message: `${entryTitle(entry)}\n\nThe agent stops remembering this. Every save is snapshotted, so the previous file is recoverable.`,
      confirmLabel: 'Forget it',
      danger: true,
    })
    if (!ok) return
    await write((fresh) => {
      const at = fresh.indexOf(entry)
      return at === -1 ? null : removeEntry(fresh, at)
    })
  }

  const add = async () => {
    const text = fresh.trim()
    if (!text || busy) return
    if (await write((fresh) => [...fresh, text])) reset()
  }
</script>

<!-- THE FRAME STAYS UP THROUGH A FAILED READ, and that is not cosmetic: the
     file switch lives in it, so an error rendered INSTEAD of the frame is a
     dead end — a profile that cannot be read would strand a person with no
     control that takes them back to the memory they can read. The failure
     belongs where the list belongs. -->
<div class={cn('flex flex-col', className)}>
  <div class="flex items-center gap-2 px-4">
    <!-- Two documents, one view: the agent's record and the profile it keeps
         about its person. Both live in the same place and are edited the
         same way, so this is a mode switch, not a tab. -->
    <Segmented options={FILES} value={file} onChange={switchFile} />
    <Input size="sm" bind:value={q} placeholder="Search" aria-label={`Search ${FILENAME[file]}`} class="flex-1" />
    <span class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
      {FILENAME[file]}{query.data ? ` · ${q.trim() ? `${hits.length}/${entries.length}` : entries.length}` : ''}
    </span>
  </div>

  <!-- No etag on this document, and the agent curates it itself — say so
       where the edits happen rather than in a tooltip nobody opens. -->
  <p class="mt-2 px-4 font-sans text-xs leading-5 text-muted">
    The agent writes this file too, so a save replaces it as it stands on disk now. Every save is snapshotted.
  </p>

  {#if failure}
    <QueryError variant="inline" class="mt-2 px-4" error={failure} title="Could not save this memory" />
  {/if}

  <!-- The list scrolls; the controls above it do not. -->
  <div class="mt-3 min-h-0 flex-1 overflow-y-auto px-4 pb-4">
    {#if query.isError}
      <QueryError error={query.error} title={`Can't read ${FILENAME[file]}`} onRetry={() => void query.refetch()} />
    {:else if !query.data}
      <SkeletonRows rows={6} />
    {:else if !entries.length}
      <EmptyState
        variant="compact"
        icon="◇"
        title="Nothing remembered yet"
        hint={canEdit
          ? 'Add an entry, or let the agent write here as it works.'
          : 'The agent writes here as it works.'}
      />
    {:else if !hits.length}
      <EmptyState variant="compact" icon="◇" title="Nothing matches" hint={`No entry in ${FILENAME[file]} mentions “${q.trim()}”.`} />
    {:else}
      <div class="space-y-2">
        {#each hits as i (i)}
          {@const entry = entries[i] ?? ''}
          <!-- An inset well, not a panel fill: this list renders inside a
               modal and inside a settings panel, both panel-filled, and a
               card the colour of its container is a bare outline. -->
          <Panel class="bg-surface p-3">
            {#if editing === i}
              <Textarea
                bind:value={draft}
                autoGrow
                rows={4}
                aria-label="Entry text"
                class="max-h-80 font-sans text-sm"
                disabled={busy}
              />
              <div class="mt-2 flex items-center gap-2">
                <Button size="sm" onclick={() => void saveEdit()} disabled={busy || !draft.trim()}>
                  {busy ? 'Saving' : 'Save'}
                </Button>
                <Button size="sm" variant="ghost" onclick={reset} disabled={busy}>Cancel</Button>
              </div>
            {:else}
              <div class="flex items-start gap-3">
                <div class="min-w-0 flex-1">
                  <div class="truncate font-sans text-sm text-fg">{entryTitle(entry)}</div>
                  {#if entry.includes('\n')}
                    <p class="mt-1 line-clamp-4 whitespace-pre-wrap font-sans text-xs leading-5 text-muted">{entry}</p>
                  {/if}
                </div>
                {#if canEdit}
                  <div class="flex shrink-0 items-center gap-3">
                    <Button variant="link" size="sm" onclick={() => startEdit(i)} disabled={busy}>Edit</Button>
                    <!-- Quiet red link, never a button (UI-CONVENTIONS). -->
                    <DangerLink onClick={() => void remove(i)} disabled={busy}>Remove</DangerLink>
                  </div>
                {/if}
              </div>
            {/if}
          </Panel>
        {/each}
      </div>
    {/if}

    {#if canEdit && query.data}
      {#if adding}
        <Panel class="mt-2 bg-surface p-3">
          <Textarea
            bind:value={fresh}
            autoGrow
            rows={3}
            aria-label="New entry"
            placeholder="One thing the agent should remember"
            class="max-h-80 font-sans text-sm"
            disabled={busy}
          />
          <div class="mt-2 flex items-center gap-2">
            <Button size="sm" onclick={() => void add()} disabled={busy || !fresh.trim()}>
              {busy ? 'Saving' : 'Save'}
            </Button>
            <Button size="sm" variant="ghost" onclick={reset} disabled={busy}>Cancel</Button>
          </div>
        </Panel>
      {:else}
        <Button size="sm" variant="ghost" class="mt-2" onclick={() => (adding = true)}>
          <Plus size={13} aria-hidden="true" /> Add entry
        </Button>
      {/if}
    {/if}
  </div>
</div>
