<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
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

  // What the draft was opened from. A draft with no origin is a new role, and
  // one that matches its origin has nothing to save.
  const origin = $derived(selected ? templates.find((t) => t.slug === selected) : undefined)
  const DRAFT_FIELDS = ['slug', 'name', 'role', 'department', 'description', 'soul'] as const
  const changed = $derived(!!draft && (!origin || DRAFT_FIELDS.some((k) => draft![k] !== origin[k])))

  const refresh = () => qc.invalidateQueries({ queryKey: ROLE_TEMPLATES_KEY })

  const pick = (t: RoleTemplate) => {
    selected = t.slug
    err = null
    // A built-in opens as a DRAFT COPY: editing one means writing your own
    // version under the same slug, which shadows ours. Nothing we ship is
    // mutated, and deleting your copy restores it.
    draft = { slug: t.slug, name: t.name, role: t.role, department: t.department, description: t.description, soul: t.soul }
  }

  /** The slug a name implies. Editable in the form afterwards — this only has
   *  to be a good default, and a role called "Research Analyst" wanting to be
   *  `research-analyst` is not a guess. */
  const slugify = (name: string) =>
    name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '')

  // Creation is NAMED now, like every other library in the app: the pane's
  // create control asks for the role's name and hands it here. The button used
  // to open a wholly blank form instead, which is the same act with nothing
  // filled in and no statement of what was about to be made.
  //
  // The draft is unsaved on purpose — a role is not a row until it has a soul,
  // so this opens the editor rather than writing anything.
  const blank = (name: string) => {
    selected = null
    err = null
    draft = {
      slug: slugify(name),
      name,
      role: name,
      department: '',
      description: '',
      soul: `# Role — ${name}\n\n## Who you are\n\n## Voice & personality\n\n## How you work\n- Keep humans in the loop: create and triage tickets, never assign or close them.\n- Never do dev work in chat: code changes ship through a ticket and a workbench job, however small, and instructions given in chat land on that ticket as comments.\n`,
    }
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
    { label: 'Your organization', items: templates.filter((t) => !t.builtIn), empty: 'None yet. Edit a common role to make one.' },
    { label: 'Common roles', items: templates.filter((t) => t.builtIn) },
  ])

  // `draft` drives the editor, not `selected`: a brand-new role has a draft and
  // NO selection, and the pane must show the editor for it rather than the
  // "pick something" empty state.
  const paneSelection = $derived(draft ? (selected ?? '__new__') : null)

  // The pane never rests on nothing: with no draft open (first load, or after
  // a delete) it opens the first role in the picker's own order.
  $effect(() => {
    if (draft) return
    const first = groups.flatMap((g) => g.items)[0]
    if (first) pick(first)
  })

  /** The stored org role the editor has open, which is the one Delete removes. */
  const stored = $derived(selected ? templates.find((t) => t.slug === selected && !t.builtIn) : undefined)
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
  class="h-full min-h-[28rem]"
  onCreate={blank}
  createLabel="New role"
>
  {#snippet empty()}
    <EmptyState title="No roles yet" hint="Add one with New role, under the list." />
  {/snippet}

  {#snippet detail()}
    {#if draft}
      <div class="flex min-h-11 shrink-0 items-center gap-2 border-b border-line-subtle px-6 py-2">
        <span class="line-clamp-2 min-w-0 font-sans text-[15px] font-semibold leading-5 text-fg" title={draft.name || undefined}>{draft.name || 'New role'}</span>
        <!-- The list already says whose a stored role is. Only a role that is nowhere yet is marked. -->
        {#if !selected}<Chip class="shrink-0">Not saved yet</Chip>{/if}
      </div>
      <div class="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto p-6">
        <div class="grid shrink-0 grid-cols-2 gap-x-4 gap-y-5">
          <SettingsField label="Name" for="rt-name">
            <Input id="rt-name" size="sm" bind:value={draft.name} placeholder="Research Analyst" />
          </SettingsField>
          <SettingsField label="Slug" for="rt-slug">
            <Input id="rt-slug" size="sm" class="font-mono" bind:value={draft.slug} placeholder="research-analyst" />
          </SettingsField>
          <SettingsField label="Role title" for="rt-role">
            <Input id="rt-role" size="sm" bind:value={draft.role} placeholder="Research Analyst" />
          </SettingsField>
          <SettingsField label="Department" for="rt-dept">
            <Input id="rt-dept" size="sm" bind:value={draft.department} placeholder="research" />
          </SettingsField>
          <SettingsField label="Description" for="rt-desc" class="col-span-2">
            <Input id="rt-desc" size="sm" bind:value={draft.description} placeholder="One line, shown when someone picks a role." />
          </SettingsField>
        </div>
        <!-- The instructions take the height the fields leave, so the form
             fits the window and only the text scrolls. -->
        <div class="flex min-h-[14rem] flex-1 flex-col gap-1.5">
          <label for="rt-soul" class="font-sans text-[13px] font-medium leading-5 text-fg">Starting instructions</label>
          <Textarea id="rt-soul" bind:value={draft.soul} class="min-h-0 flex-1 font-mono text-xs leading-5" />
          <p class="font-sans text-[13px] leading-5 text-muted">Markdown. Keep the three section headings, the agent designer expects them.</p>
        </div>
      </div>
      <!-- The foot the other pick-and-edit pages have: Delete at its left, Save at its right, as tall as the
           picker's own foot so the two hairlines meet. -->
      <div class="flex h-[53px] shrink-0 items-center gap-4 border-t border-line px-6">
        {#if stored}
          <DangerLink onClick={() => void remove(stored.slug)} disabled={busy} class="shrink-0">Delete role</DangerLink>
        {/if}
        <span class={['min-w-0 flex-1 truncate text-right font-sans text-[13px] leading-5', err ? 'text-danger' : 'text-muted']}>
          {#if err}{err}{:else if isNew && selected}Saves as your organization’s version of “{draft.name}”.{/if}
        </span>
        <!-- Gold once there is an edit to save. An untouched common role can still be copied as it stands. -->
        <Button
          size="sm"
          variant={changed ? 'primary' : 'outline'}
          class="shrink-0"
          onclick={() => void save()}
          disabled={busy || (!changed && !isNew) || !draft.slug.trim() || !draft.name.trim() || !draft.soul.trim()}
        >
          {busy ? 'Saving' : isNew ? 'Save as ours' : 'Save'}
        </Button>
      </div>
    {/if}
  {/snippet}
</LibraryPane>
