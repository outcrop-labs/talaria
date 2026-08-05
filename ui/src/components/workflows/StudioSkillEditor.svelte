<script lang="ts">
  // The Studio's skill editor — one SKILL.md in the full workspace editor
  // (rich + Muse drafting + version history). Same contract as the agent-view
  // editor: reads live, Hermes picks up saves on the agent's next run.
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { getJson } from '@/lib/fetch-json'
  import InternalEditorModal from '@/components/fleet/InternalEditorModal.svelte'

  let {
    owner,
    ownerLabel,
    name,
    canEdit,
    onClose,
  }: {
    owner: string
    ownerLabel: string
    name: string
    canEdit: boolean
    onClose: () => void
  } = $props()

  const qc = useQueryClient()
  // 404 is NOT forgiven into a null here: the editor is opened from a library
  // row, so "no such skill" is a real failure worth naming — and the route
  // sends its reason as `{ error }`, which `readJson` lifts into the message.
  const query = createQuery(() => ({
    queryKey: ['skill', owner, name],
    queryFn: (): Promise<{ content: string; files: string[] }> => getJson<{ content: string; files: string[] }>(`/api/skills/${owner}/${name}`),
  }))
  let busy = $state(false)

  const save = async (content: string) => {
    busy = true
    try {
      await fetch(`/api/skills/${owner}/${name}`, {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ content }),
      })
      await qc.invalidateQueries({ queryKey: ['skill-library'] })
      await qc.invalidateQueries({ queryKey: ['skills'] })
      await qc.invalidateQueries({ queryKey: ['skill', owner, name] })
    } finally {
      busy = false
    }
  }
  const remove = async () => {
    if (!(await confirm({ title: 'Delete skill', message: `Delete "${name}"? Workflows bound to it will flag it as missing.`, confirmLabel: 'Delete', danger: true }))) return
    await fetch(`/api/skills/${owner}/${name}`, { method: 'DELETE', credentials: 'same-origin' })
    await qc.invalidateQueries({ queryKey: ['skill-library'] })
    await qc.invalidateQueries({ queryKey: ['skills'] })
    onClose()
  }
</script>

<!-- The editor seeds ONCE from `value` — don't mount it until content is here.
     A failed read must never seed it with '': saving from there would replace
     the real SKILL.md with an empty file. -->
{#if !query.data}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/40" onclick={query.isError ? onClose : undefined}>
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="w-full max-w-3xl space-y-3 rounded-lg border border-line bg-[var(--theme-panel)] p-6" onclick={(e) => e.stopPropagation()}>
      {#if query.isError}
        <QueryError
          variant="compact"
          error={query.error}
          title={`Could not open ${name}`}
          onRetry={() => void query.refetch()}
        />
      {:else}
        <Skeleton class="h-2.5 w-2/3 rounded-full" />
        <Skeleton class="h-2.5 w-full rounded-full" delay={0.12} />
        <Skeleton class="h-2.5 w-3/4 rounded-full" delay={0.24} />
      {/if}
    </div>
  </div>
{:else}
  {#snippet deleteSkill()}
    <Button variant="ghost" size="sm" onclick={() => void remove()}>
      Delete skill
    </Button>
  {/snippet}
  <InternalEditorModal
    open
    {onClose}
    title={`${name} · SKILL.md`}
    subtitle={`${ownerLabel} — read live; agents pick up edits on their next run.`}
    value={query.data.content}
    editable={canEdit}
    saving={busy}
    onSave={save}
    history={{ kind: 'skill', owner, name }}
    muse={{
      kind: 'skill',
      context:
        owner === 'shared'
          ? 'A shared skill available to every agent in the fleet. SKILL.md format: a heading, a "When to use" line, then concrete numbered steps.'
          : `A skill for the "${ownerLabel}" agent. SKILL.md format: a heading, a "When to use" line, then concrete numbered steps.`,
    }}
    footerExtra={canEdit ? deleteSkill : undefined}
  />
{/if}
