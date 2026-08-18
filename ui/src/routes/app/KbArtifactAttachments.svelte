<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Paperclip } from '@lucide/svelte'
  import CloseButton from '@/components/ui/CloseButton.svelte'
  import Combobox from '@/components/ui/Combobox.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { attachArtifact, detachArtifact, useArtifacts, useTargetArtifacts } from '@/lib/artifacts'

  // Attach any artifact to a KB doc (the "attach an artifact to anything" spec).
  let { docId }: { docId: string } = $props()

  const qc = useQueryClient()
  // Two defaults, two different lies: an empty `attached` says this document
  // has nothing attached to it, and an empty `all` says there is nothing in
  // the org to attach.
  const attachedList = listQuery(useTargetArtifacts('kb-doc', () => docId), { title: 'Could not load this document’s attachments', variant: 'inline' })
  const allList = listQuery(useArtifacts(), { title: 'Could not load your artifacts', variant: 'inline' })
  const attached = $derived(attachedList.rows)
  const all = $derived(allList.rows)
  const loading = $derived(attachedList.pending || allList.pending)
  const attachedIds = $derived(new Set(attached.map((a) => a.id)))
  const options = $derived(all.filter((a) => !attachedIds.has(a.id)).map((a) => ({ value: a.id, label: a.title, sub: a.kind })))
  const refresh = () => qc.invalidateQueries({ queryKey: ['artifacts-for', 'kb-doc', docId] })
</script>

{#if loading}
  <div aria-hidden="true" class="mx-auto max-w-[var(--read-width)] px-6 pb-10">
    <div class="border-t border-line-subtle pt-4">
      <div class="mb-2 flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
        <Paperclip size={12} /> Attachments
      </div>
      <div class="flex flex-wrap gap-2">
        <Skeleton class="h-7 w-36" />
        <Skeleton class="h-7 w-28" />
      </div>
    </div>
  </div>
{:else}
  <div class="mx-auto max-w-[var(--read-width)] px-6 pb-10">
    <div class="border-t border-line-subtle pt-4">
      <div class="mb-2 flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
        <Paperclip size={12} /> Attachments
      </div>
      {#if attachedList.notice}<QueryError {...attachedList.notice} />{/if}
      {#if allList.notice}<QueryError {...allList.notice} />{/if}
      {#if attached.length > 0}
        <div class="mb-2 flex flex-wrap gap-2">
          {#each attached as a (a.id)}
            <span class="flex items-center gap-1.5 rounded-md border border-line px-2 py-1 text-xs text-muted">
              <span>{a.icon ?? '◆'}</span>
              <span class="max-w-[14rem] truncate text-fg">{a.title}</span>
              <span class="font-mono text-[10px] uppercase tracking-[0.05em]">{a.kind}</span>
              <CloseButton
                size={11}
                label="Detach"
                onClick={async () => {
                  await detachArtifact(a.id, 'kb-doc', docId)
                  await refresh()
                }}
                class="p-0 hover:bg-transparent hover:text-danger"
              />
            </span>
          {/each}
        </div>
      {/if}
      <Combobox
        {options}
        selected={[]}
        onChange={async (v) => {
          if (v[0]) {
            await attachArtifact(v[0], 'kb-doc', docId)
            await refresh()
          }
        }}
        placeholder="Attach a file"
        size="sm"
        class="max-w-xs"
      />
    </div>
  </div>
{/if}
