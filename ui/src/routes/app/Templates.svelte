<script lang="ts">
  import { createRawSnippet, type Snippet } from 'svelte'
  import { searchParams } from 'sv-router'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Plus } from '@lucide/svelte'
  import { navigate } from '@/router'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import Tabs from '@/components/ui/Tabs.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Materialize from '@/components/ui/Materialize.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import ViewHeader from '@/components/ui/ViewHeader.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import { useContextMenu, copyAppLink } from '@/components/ui/context-menu.svelte'
  import { cn } from '@/lib/cn'
  import { fly, staggerIn } from '@/lib/motion'
  import { useSession } from '@/lib/session'
  import { createTemplate, deleteTemplate, useTemplates, type Template, type TemplateKind } from '@/lib/templates'
  import TemplateDetail from './templates/TemplateDetail.svelte'

  // Templates — the skeletons work starts from, managed in one place. One tab
  // per surface that consumes them (tickets, plans); the body edits in the
  // full workspace editor (rich + Muse prompt-editing + version history).
  const TEMPLATE_TABS: { id: TemplateKind; label: string; blurb: string }[] = [
    { id: 'ticket', label: 'Tickets', blurb: 'Seeds every ticket description: board defaults, agent bindings, and the QA judge scores against its sections.' },
    { id: 'plan', label: 'Plans', blurb: 'Seeds a new plan’s living document and shapes how the agent rewrites it as the conversation grows.' },
  ]

  const sessionQuery = useSession()
  const session = $derived(sessionQuery.data)
  const qc = useQueryClient()
  const templatesQuery = useTemplates()
  const templates = $derived(templatesQuery.data ?? [])
  const isLoading = $derived(templatesQuery.isLoading)
  // One read backs BOTH the list and the per-tab counters, and the `= []`
  // default turned its failure into a confident "Tickets 0 / Plans 0" —
  // a count is an assertion, so it must not be printed from a read that failed.
  const failed = $derived(templatesQuery.isError && templatesQuery.data === undefined)
  // /templates?tab=plan&t=<id> deep-links a tab + template.
  const tab = $derived.by((): TemplateKind => {
    const raw = searchParams.get('tab')
    return TEMPLATE_TABS.some((v) => v.id === raw) ? (raw as TemplateKind) : 'ticket'
  })
  const selectedId = $derived.by((): string | null => {
    const t = searchParams.get('t')
    return t == null || t === '' ? null : String(t)
  })
  const setTab = (t: TemplateKind) => void navigate('/templates', { search: t === 'ticket' ? {} : { tab: t } })
  const select = (id: string | null) => void navigate('/templates', { search: { ...(tab !== 'ticket' ? { tab } : {}), ...(id ? { t: id } : {}) } })
  const menu = useContextMenu()

  let newName = $state('')
  let editorOpen = $state(false)

  const list = $derived(templates.filter((t) => t.kind === tab))
  const selected = $derived(templates.find((t) => t.id === selectedId && t.kind === tab) ?? null)
  const meta = $derived(TEMPLATE_TABS.find((t) => t.id === tab)!)

  const refresh = () => qc.invalidateQueries({ queryKey: ['templates'] })
  const create = async () => {
    const name = newName.trim()
    if (!name) return
    const { template } = await createTemplate({ name, kind: tab })
    newName = ''
    await refresh()
    if (template) select(template.id)
  }
  const remove = async (t: Template) => {
    if (!(await confirm({ title: 'Delete template', message: `Delete "${t.name}"? Boards and agents bound to it fall back down the template chain.`, confirmLabel: 'Delete', danger: true }))) return
    await deleteTemplate(t.id)
    if (selectedId === t.id) select(null)
    await refresh()
  }

  // Tab labels carry the per-kind count — bridged into the zero-arg snippet
  // Tabs.svelte expects (the React version passed a <span> inline).
  // An em dash where a number would be: unknown, not zero. A
  // count is an assertion — never print one from a read that
  // hasn't landed or has failed.
  const tabLabel = (label: string, count: string): Snippet =>
    createRawSnippet(() => ({
      render: () => `<span>${label}<span class="ml-1.5 text-ink-dim">${count}</span></span>`,
    }))
  const tabItems = $derived(
    TEMPLATE_TABS.map((t) => ({
      id: t.id,
      label: tabLabel(t.label, isLoading || failed ? '—' : String(templates.filter((x) => x.kind === t.id).length)),
    })),
  )
</script>

{#if session && session.role !== 'admin'}
  <EmptyState icon="▣" title="Admins only" />
{:else}
  <div class="h-full overflow-y-auto p-8">
    <!-- Page content entrance: header row → tab strip → list+detail pane rise
         in sequence (ANIMATIONS.md). The keyed pane keeps its own fly on kind
         switch and stays unstaggered inside — one level only. -->
    <div use:staggerIn class="mx-auto max-w-5xl space-y-6">
      <ViewHeader
        title="Templates"
        info="The markdown skeletons work starts from. Resolution order everywhere: explicit pick → agent binding → board default → freeform."
      />

      <Tabs items={tabItems} value={tab} onChange={setTab} />

      <!-- Tab-pane grammar: the whole list+detail pane rises in on a kind
           switch (no exit). Safe to key: the new-template input's state and
           `editorOpen` live above; the detail is already keyed by selection.
           The sidebar list's own cascade belongs to Materialize's content
           branch — row-shaped skeletons materialize into the template rows. -->
      {#key tab}
      <div in:fly={{ y: 6, duration: 200 }} class="grid gap-6 lg:grid-cols-[16rem_minmax(0,1fr)]">
        <aside class="space-y-3">
          <!-- Materialize direct (no QueryState here): this site already keys
               everything off `isLoading`/`failed`, so error and empty stay in
               the resolved branch and only the loading swap changes shape. -->
          <Materialize loading={isLoading} count={5} class="space-y-0.5">
            {#snippet skeleton(i)}
              <!-- One sidebar row's silhouette: same rounded-md px-2.5 py-2
                   frame, name bar at a width that varies by index. -->
              <div aria-hidden="true" class="rounded-md px-2.5 py-2">
                <div class="flex h-5 items-center">
                  <Skeleton class={`h-3 rounded-full ${['w-32', 'w-24', 'w-40', 'w-28', 'w-36'][i % 5]}`} delay={i * 0.12} />
                </div>
              </div>
            {/snippet}
            {#if failed}
              <QueryError
                variant="compact"
                error={templatesQuery.error}
                title="Could not load templates"
                onRetry={() => void templatesQuery.refetch()}
              />
            {:else}
              {#each list as t (t.id)}
                <button
                  type="button"
                  onclick={() => select(t.id)}
                  oncontextmenu={(e) =>
                    menu.openMenu(e, [
                      { label: 'Open', onSelect: () => select(t.id) },
                      { label: 'Copy link', onSelect: () => copyAppLink(`/templates?tab=${t.kind}&t=${t.id}`) },
                      'sep',
                      { label: 'Delete', danger: true, onSelect: () => void remove(t) },
                    ])}
                  class={cn(
                    'flex w-full items-center gap-2 rounded-md px-2.5 py-2 text-left text-sm transition-colors',
                    selected?.id === t.id ? 'bg-raised text-fg' : 'text-muted hover:bg-hover hover:text-fg',
                  )}
                >
                  <span class="min-w-0 flex-1 truncate font-sans">{t.name}</span>
                </button>
              {/each}
              {#if list.length === 0}<div class="px-2.5 py-2 text-xs text-muted">None yet.</div>{/if}
            {/if}
          </Materialize>
          <div class="flex items-center gap-1.5 border-t border-line pt-3">
            <Input size="sm" bind:value={newName} placeholder={`New ${tab} template`} onkeydown={(e) => e.key === 'Enter' && void create()} />
            <Button size="sm" variant="outline" disabled={!newName.trim()} onclick={() => void create()}>
              <Plus size={14} />
            </Button>
          </div>
        </aside>

        {#if selected}
          {#key selected.id}
            <TemplateDetail template={selected} blurb={meta.blurb} onChanged={refresh} onDelete={() => selected && void remove(selected)} {editorOpen} setEditorOpen={(v) => (editorOpen = v)} />
          {/key}
        {:else}
          <Panel>
            <!-- "Create the first one" is the same zero-claim in prose. -->
            <EmptyState
              icon="▣"
              title={`No ${tab} template selected`}
              hint={failed
                ? 'The template list could not be loaded — retry on the left.'
                : list.length
                  ? 'Pick one on the left, or create a new one.'
                  : 'Create the first one on the left.'}
            />
          </Panel>
        {/if}
      </div>
      {/key}
    </div>
    <ContextMenu {menu} />
  </div>
{/if}
