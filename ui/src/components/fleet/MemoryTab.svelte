<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { submitOnEnter } from '@/components/ui/control'
  import { getJson } from '@/lib/fetch-json'
  import type { AgentDef } from '@/lib/fleet-defs'
  import { fade } from '@/lib/motion'
  import InternalEditorModal from './InternalEditorModal.svelte'

  let { def, isAdmin }: { def: AgentDef; isAdmin: boolean } = $props()

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['memory', def.id],
    // GET /api/memory/:id is 200 `{ content, container }`, or 400/403 `{ error }`
    // when the container can't be reached — every non-2xx is a real failure and
    // already had a real error branch below; this just routes it through the
    // one door so the message is the server's own sentence.
    queryFn: async (): Promise<{ content: string; container: string }> => {
      const j = await getJson<{ content?: string; container?: string }>(`/api/memory/${def.id}`)
      return { content: j.content ?? '', container: j.container ?? '' }
    },
    retry: false,
  }))
  let editing = $state(false)
  let busy = $state(false)
  let note = $state('')
  const save = async (content: string) => {
    busy = true
    try {
      const r = await fetch(`/api/memory/${def.id}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ content }) })
      const j = (await r.json()) as { error?: string }
      if (!j.error) await qc.invalidateQueries({ queryKey: ['memory', def.id] })
    } finally {
      busy = false
    }
  }
  // Quick-add: append a dated note without opening the full editor.
  const addNote = async () => {
    const n = note.trim()
    if (!n || query.isLoading) return // appending before the fetch lands would clobber the doc
    const stamp = new Date().toISOString().slice(0, 10)
    const base = (query.data?.content ?? '').replace(/\s+$/, '')
    await save(`${base ? `${base}\n` : ''}- ${n} _(added by hand, ${stamp})_\n`)
    note = ''
  }
</script>

{#if !def.managed}
  <EmptyState icon="❖" title="Not managed" hint="Memory reads through the managed container. Migrate this agent first." />
{:else if query.isError}
  <QueryError error={query.error} title="Can't reach the agent" onRetry={() => void query.refetch()} />
{:else}
  <div class="space-y-3">
    <div class="flex items-center gap-1.5">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Memory</span>
      <InfoTip text={`Lives in the agent's own container (${query.data?.container ?? ''}) — the agent curates it too, so a concurrent agent write can win over a manual edit. Every save is snapshotted and revertible.`} />
      {#if isAdmin}
        <Button size="sm" class="ml-auto shrink-0" disabled={query.isLoading} onclick={() => (editing = true)}>
          Edit
        </Button>
      {/if}
    </div>
    {#if isAdmin}
      <div class="flex items-center gap-2">
        <Input
          size="sm"
          bind:value={note}
          onkeydown={submitOnEnter(() => void addNote())}
          placeholder="Add a memory — one fact the agent should keep"
          class="flex-1"
        />
        <Button size="sm" variant="outline" disabled={busy || query.isLoading || !note.trim()} onclick={() => void addNote()}>
          Add
        </Button>
      </div>
    {/if}
    {#if query.isLoading}
      <!-- The document is coming — prose-bar shimmer where it will render. -->
      <div class="space-y-3 pt-1" aria-hidden="true">
        <Skeleton class="h-2.5 w-3/4 rounded-full" />
        <Skeleton class="h-2.5 w-full rounded-full" delay={0.12} />
        <Skeleton class="h-2.5 w-5/6 rounded-full" delay={0.24} />
        <Skeleton class="h-2.5 w-2/3 rounded-full" delay={0.36} />
        <Skeleton class="h-2.5 w-full rounded-full" delay={0.48} />
        <Skeleton class="h-2.5 w-1/2 rounded-full" delay={0.6} />
      </div>
    {:else if query.data?.content}
      <div class="text-sm">
        <Markdown children={query.data.content} />
      </div>
    {:else}
      <div in:fade={{ duration: 150 }}>
        <EmptyState icon="❖" title="No memory yet" hint="The agent hasn't written anything down." />
      </div>
    {/if}
    {#if editing}
      <InternalEditorModal
        open
        nested
        onClose={() => (editing = false)}
        title={`${def.displayName} · MEMORY.md`}
        subtitle="The agent maintains this itself; your edits are snapshotted and revertible."
        value={query.data?.content ?? ''}
        editable={isAdmin}
        saving={busy}
        onSave={save}
        history={{ kind: 'memory', id: def.id }}
        muse={{ kind: 'memory', context: `The memory of the "${def.slug}" agent (${def.role ?? def.department}).` }}
      />
    {/if}
  </div>
{/if}
