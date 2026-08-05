<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { getJson } from '@/lib/fetch-json'
  import { fade, fly, QUICK } from '@/lib/motion'
  import InternalEditorModal from './InternalEditorModal.svelte'

  let { slug, name, isAdmin, onClose }: { slug: string; name: string; isAdmin: boolean; onClose: () => void } = $props()

  const qc = useQueryClient()
  // A 404 here is NOT a legitimate null: this editor is only reachable from a
  // row in the list above (or straight after a create), so "no such skill" is
  // as much of a failure as a 500 — and the route already sends the reason as
  // `{ error }`, which `readJson` turns into the line the panel shows.
  const query = createQuery(() => ({
    queryKey: ['skill', slug, name],
    queryFn: (): Promise<{ content: string; files: string[] }> => getJson<{ content: string; files: string[] }>(`/api/skills/${slug}/${name}`),
  }))
  let busy = $state(false)
  const save = async (content: string) => {
    busy = true
    try {
      await fetch(`/api/skills/${slug}/${name}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ content }) })
      await qc.invalidateQueries({ queryKey: ['skills'] })
      await qc.invalidateQueries({ queryKey: ['skill', slug, name] })
    } finally {
      busy = false
    }
  }
  const remove = async () => {
    if (!(await confirm({ title: 'Delete skill', message: `Delete the "${name}" skill?`, confirmLabel: 'Delete', danger: true }))) return
    await fetch(`/api/skills/${slug}/${name}`, { method: 'DELETE' })
    await qc.invalidateQueries({ queryKey: ['skills'] })
    onClose()
  }
</script>

{#snippet footerExtra()}
  <Button variant="ghost" size="sm" onclick={() => void remove()}>Delete skill</Button>
{/snippet}

<!-- Don't mount the editor until the content is here — it seeds ONCE from
     `value`; mounting during the fetch would show an empty skill forever.
     But the CLICK must land instantly: slide in the editor-shaped shell
     (same surface InternalEditorModal uses in nested mode) with a skeleton
     body, then swap to the real editor when the content arrives. -->
{#if !query.data}
  <!-- The slide promised above: same surface + travel as InternalEditorModal's
       nested entrance. |global — this block is true the moment the component
       mounts, so local legs would be suppressed (ANIMATIONS.md). -->
  <div in:fly|global={{ x: '100%', duration: 180, opacity: 1 }} out:fade|global={QUICK} class="absolute inset-0 z-30 flex flex-col bg-[var(--theme-panel)]">
    <div class="flex shrink-0 items-center gap-2 border-b border-line-subtle px-6 py-3.5">
      <div class="text-sm font-semibold text-fg">{name} · SKILL.md</div>
      <Button variant="ghost" size="sm" class="ml-auto" onclick={onClose}>
        Close
      </Button>
    </div>
    {#if query.isError}
      <!-- Never seed the editor from a failed read: mounting it with '' and
           then letting someone hit Save would overwrite the real SKILL.md
           with an empty file. -->
      <QueryError error={query.error} title="Could not open this skill" onRetry={() => void query.refetch()} />
    {:else}
      <div class="space-y-3 p-6" aria-hidden="true">
        <Skeleton class="h-2.5 w-2/3 rounded-full" />
        <Skeleton class="h-2.5 w-full rounded-full" delay={0.12} />
        <Skeleton class="h-2.5 w-5/6 rounded-full" delay={0.24} />
        <Skeleton class="h-2.5 w-3/4 rounded-full" delay={0.36} />
        <Skeleton class="h-2.5 w-1/2 rounded-full" delay={0.48} />
      </div>
    {/if}
  </div>
{:else}
  <InternalEditorModal
    open
    nested
    {onClose}
    title={`${name} · SKILL.md`}
    subtitle="Read live. The agent picks up edits on its next run."
    value={query.data.content}
    editable={isAdmin}
    saving={busy}
    onSave={save}
    history={{ kind: 'skill', owner: slug, name }}
    muse={{ kind: 'skill', context: `A skill for the "${slug}" agent.` }}
    footerExtra={isAdmin ? footerExtra : undefined}
  />
{/if}
