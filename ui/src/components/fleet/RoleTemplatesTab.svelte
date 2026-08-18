<script lang="ts">
  import { Plus, Trash2 } from '@lucide/svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import LibraryPane from '@/components/ui/LibraryPane.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import {
    ROLE_TEMPLATES_KEY,
    deleteRoleTemplate,
    saveRoleTemplate,
    useRoleTemplates,
    type RoleTemplate,
  } from '@/lib/agent-role-templates'
  import { errorMessage } from '@/lib/fetch-json'

  // The role library — the second tab of AGENTS, not a dialog and not Admin.
  // It is part of making an agent, and the person who creates agents is the one
  // who knows what the roles should be. It was briefly a modal behind a toolbar
  // icon, which buried a whole surface in an affordance meant for one action.
  //
  // The two-pane shape is `LibraryPane`, shared with Templates — this file used
  // to own its own copy of it, along with its own fetch, its own loading and
  // error strings, and its own idea of what a picker row looks like.

  const qc = useQueryClient()
  const query = useRoleTemplates()
  const list = listQuery(query, { title: 'Could not load the role library' })
  const templates = $derived(list.rows)

  let selected = $state<string | null>(null)
  let busy = $state(false)
  let err = $state<string | null>(null)

  // The edit buffer. Separate from the list so an unsaved edit is never
  // mistaken for what is stored.
  let draft = $state<{ slug: string; name: string; role: string; department: string; description: string; soul: string } | null>(null)
  /** True when the draft would create a NEW org template (including one that
   *  shadows a built-in) rather than update an existing org one. */
  const isNew = $derived(!!draft && !templates.some((t) => t.slug === draft!.slug && !t.builtIn))

  const refresh = () => qc.invalidateQueries({ queryKey: ROLE_TEMPLATES_KEY })

  const pick = (t: RoleTemplate) => {
    selected = t.slug
    err = null
    // A built-in opens as a DRAFT COPY: editing one means writing your own
    // version under the same slug, which shadows ours. Nothing we ship is
    // mutated, and deleting your copy restores it.
    draft = { slug: t.slug, name: t.name, role: t.role, department: t.department, description: t.description, soul: t.soul }
  }

  const blank = () => {
    selected = null
    err = null
    draft = { slug: '', name: '', role: '', department: '', description: '', soul: '# Role — Title\n\n## Who you are\n\n## Voice & personality\n\n## How you work\n- Keep humans in the loop: create and triage tickets, never assign or close them.\n' }
  }

  const save = async () => {
    if (!draft) return
    busy = true
    err = null
    try {
      await saveRoleTemplate(draft)
      await refresh()
      selected = draft.slug
    } catch (e) {
      err = errorMessage(e)
    } finally {
      busy = false
    }
  }

  const remove = async (slug: string) => {
    busy = true
    err = null
    try {
      await deleteRoleTemplate(slug)
      await refresh()
      if (selected === slug) {
        selected = null
        draft = null
      }
    } catch (e) {
      err = errorMessage(e)
    } finally {
      busy = false
    }
  }

  // Your own first, then what Talaria maintains — the org's roles are the ones
  // being worked on, and the built-ins are the shelf you take a copy from.
  const groups = $derived([
    { label: 'Your organization', items: templates.filter((t) => !t.builtIn), empty: 'None yet — edit a common role to make one.' },
    { label: 'Common roles', items: templates.filter((t) => t.builtIn) },
  ])

  // `draft` drives the editor, not `selected`: a brand-new role has a draft and
  // NO selection, and the pane must show the editor for it rather than the
  // "pick something" empty state.
  const paneSelection = $derived(draft ? (selected ?? '__new__') : null)
</script>

<LibraryPane
  groups={groups}
  idOf={(t: RoleTemplate) => t.slug}
  labelOf={(t: RoleTemplate) => t.name}
  selectedId={paneSelection}
  onSelect={pick}
  title="Roles"
  pending={list.pending}
  notice={list.notice}
  listWidth="w-72"
  class="h-[calc(100vh-16rem)] min-h-[28rem]"
>
  {#snippet action()}
    <Button size="sm" variant="outline" class="w-8 px-0" onclick={blank} title="New role" aria-label="New role">
      <Plus size={14} />
    </Button>
  {/snippet}

  {#snippet rowAction(t: RoleTemplate)}
    <!-- Only the org's own roles can be deleted; a built-in is restored by
         deleting your copy of it, which is the same button on that copy. -->
    {#if !t.builtIn}
      <button
        type="button"
        onclick={() => void remove(t.slug)}
        disabled={busy}
        title="Delete this role"
        aria-label="Delete {t.name}"
        class="rounded-md p-1.5 text-muted transition-colors hover:text-fg"
      >
        <Trash2 size={13} />
      </button>
    {/if}
  {/snippet}

  {#snippet empty()}
    <EmptyState
      title="Pick a role, or add one"
      hint="Talaria maintains the common roles. Editing one saves YOUR version under the same name — yours is used from then on, and deleting it restores ours."
    />
  {/snippet}

  {#snippet detail()}
    {#if draft}
      <div class="min-h-0 flex-1 space-y-4 overflow-y-auto p-6">
        <div class="grid grid-cols-2 gap-4">
          <div>
            <label for="rt-name" class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Name</label>
            <Input id="rt-name" bind:value={draft.name} placeholder="Research Analyst" />
          </div>
          <div>
            <label for="rt-slug" class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Slug</label>
            <Input id="rt-slug" bind:value={draft.slug} placeholder="research-analyst" />
          </div>
        </div>
        <div class="grid grid-cols-2 gap-4">
          <div>
            <label for="rt-role" class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Role title</label>
            <Input id="rt-role" bind:value={draft.role} placeholder="Research Analyst" />
          </div>
          <div>
            <label for="rt-dept" class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Department</label>
            <Input id="rt-dept" bind:value={draft.department} placeholder="research" />
          </div>
        </div>
        <div>
          <label for="rt-desc" class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Description</label>
          <Input id="rt-desc" bind:value={draft.description} placeholder="One line, shown in the picker." />
        </div>
        <div>
          <label for="rt-soul" class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Starter soul</label>
          <Textarea id="rt-soul" bind:value={draft.soul} rows={16} class="font-mono text-xs" />
          <p class="mt-1.5 font-sans text-xs text-muted">
            Markdown. Keep the <span class="font-mono text-fg">## Who you are</span> /
            <span class="font-mono text-fg">## Voice &amp; personality</span> /
            <span class="font-mono text-fg">## How you work</span> sections — the agent designer expects that shape.
          </p>
        </div>
      </div>
      <div class="flex shrink-0 items-center justify-between gap-3 border-t border-line px-6 py-4">
        <span class="min-w-0 truncate font-sans text-xs" style:color={err ? 'var(--theme-danger)' : undefined}>
          {#if err}{err}{:else if isNew && selected}Saves as your organization’s version of “{draft.name}”.{/if}
        </span>
        <div class="flex shrink-0 gap-2">
          <Button size="sm" onclick={() => void save()} disabled={busy || !draft.slug.trim() || !draft.name.trim() || !draft.soul.trim()}>
            {busy ? 'Saving' : isNew ? 'Save as ours' : 'Save'}
          </Button>
        </div>
      </div>
    {/if}
  {/snippet}
</LibraryPane>
