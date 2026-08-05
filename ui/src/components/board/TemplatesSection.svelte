<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { p } from '@/router'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import type { Board } from '@/lib/boards.svelte'
  import { setBoardTemplates, useBoardTemplates, useTemplates } from '@/lib/templates'

  // The ticket templates this board uses: bind from the org library, mark one as
  // the default (it seeds bare tickets and shapes agent drafting on this board).
  let { board }: { board: Board } = $props()

  const qc = useQueryClient()
  // Same class as the tabs below: "No ticket templates in the library yet" is
  // a statement about the org library, and `= []` let a failed read make it.
  // The bindings read matters more — its `[]` says "this board binds none",
  // and saving from that state would UNBIND every template it never read.
  const templatesQuery = useTemplates()
  const bindingsQuery = useBoardTemplates(() => board.id)
  const templates = $derived(templatesQuery.data ?? [])
  const bindings = $derived(bindingsQuery.data ?? [])
  const templatesLoading = $derived(templatesQuery.isLoading)
  const bindingsLoading = $derived(bindingsQuery.isLoading)
  const templatesFailed = $derived(
    (templatesQuery.isError && templatesQuery.data === undefined) ||
      (bindingsQuery.isError && bindingsQuery.data === undefined),
  )
  const ticketTemplates = $derived(templates.filter((t) => t.kind === 'ticket'))
  const bound = $derived(new Set(bindings.map((b) => b.templateId)))
  const defaultId = $derived(bindings.find((b) => b.isDefault)?.templateId ?? null)

  const save = async (ids: string[], def: string | null) => {
    await setBoardTemplates(board.id, ids, def && ids.includes(def) ? def : (ids[0] ?? null))
    await qc.invalidateQueries({ queryKey: ['board-templates', board.id] })
  }
  const toggle = (id: string) => {
    const ids = bound.has(id) ? [...bound].filter((x) => x !== id) : [...bound, id]
    void save(ids, defaultId)
  }
</script>

<div>
  <div class="mb-1 flex items-center">
    <!-- svelte-ignore a11y_label_has_associated_control -->
    <label class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Ticket templates</label>
    <a href={p('/templates')} class="ml-auto text-xs text-accent hover:underline">
      Manage library →
    </a>
  </div>
  {#if templatesFailed}
    <!-- No checkboxes at all while a read is missing: an unchecked box here
         is a binding you could destroy by pressing Save. -->
    <QueryError
      variant="compact"
      title="Could not load ticket templates"
      error={templatesQuery.error ?? bindingsQuery.error}
      onRetry={() => {
        void templatesQuery.refetch()
        void bindingsQuery.refetch()
      }}
    />
  {:else if templatesLoading || bindingsLoading}
    <SkeletonRows rows={3} class="rounded-lg border border-line p-2" />
  {:else if ticketTemplates.length === 0}
    <div class="font-sans text-xs text-muted">No ticket templates in the library yet. Create one to templatize this board's tickets.</div>
  {:else}
    <div class="space-y-1 rounded-lg border border-line p-2">
      {#each ticketTemplates as t (t.id)}
        <div class="flex items-center gap-2 font-sans text-sm">
          <input
            type="checkbox"
            checked={bound.has(t.id)}
            onchange={() => toggle(t.id)}
            class="shrink-0 accent-[var(--theme-accent)]"
          />
          <span class="min-w-0 flex-1 truncate text-fg">{t.name}</span>
          {#if bound.has(t.id)}
            <label class="flex shrink-0 items-center gap-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
              <input
                type="radio"
                name={`default-template-${board.id}`}
                checked={defaultId === t.id}
                onchange={() => void save([...bound], t.id)}
                class="accent-[var(--theme-accent)]"
              />
              default
            </label>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
  <div class="mt-1 font-sans text-[11px] text-muted">
    The default seeds new tickets and formats agent-drafted ones on this board (an agent's own template binding wins).
  </div>
</div>
