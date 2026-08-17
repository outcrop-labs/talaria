<script lang="ts">
  import { Plus, Trash2 } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import { listRoleTemplates, saveRoleTemplate, deleteRoleTemplate, type RoleTemplate } from '@/lib/agent-role-templates'
  import { errorMessage } from '@/lib/fetch-json'
  import { listStagger } from '@/lib/motion'
  import { cn } from '@/lib/cn'

  // The role library — the second tab of AGENTS, not a dialog and not Admin.
  // It is part of making an agent, and the person who creates agents is the one
  // who knows what the roles should be. It was briefly a modal behind a toolbar
  // icon, which buried a whole surface in an affordance meant for one action.
  //
  // Two-pane rather than one long column: a role carries a whole soul document,
  // and stacking that under six other fields is the shape that scrolls forever.
  // List picks, pane edits.

  let templates = $state<RoleTemplate[]>([])
  let loading = $state(true)
  let loadErr = $state<string | null>(null)
  let selected = $state<string | null>(null)
  let busy = $state(false)
  let err = $state<string | null>(null)

  // The edit buffer. Separate from the list so an unsaved edit is never
  // mistaken for what is stored.
  let draft = $state<{ slug: string; name: string; role: string; department: string; description: string; soul: string } | null>(null)
  /** True when the draft would create a NEW org template (including one that
   *  shadows a built-in) rather than update an existing org one. */
  const isNew = $derived(!!draft && !templates.some((t) => t.slug === draft!.slug && !t.builtIn))

  const load = async () => {
    loading = true
    loadErr = null
    try {
      templates = await listRoleTemplates()
    } catch (e) {
      loadErr = errorMessage(e)
    } finally {
      loading = false
    }
  }

  $effect(() => {
    void load()
  })

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
      await load()
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
      await load()
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

  const own = $derived(templates.filter((t) => !t.builtIn))
  const builtIn = $derived(templates.filter((t) => t.builtIn))
</script>

<!-- Fixed height so the two panes scroll independently and the page does not
     grow a second scrollbar underneath them. -->
<Panel class="flex h-[calc(100vh-16rem)] min-h-[28rem] overflow-hidden p-0">
    <!-- ── The library ────────────────────────────────────────────────────── -->
    <div class="flex w-72 shrink-0 flex-col border-r border-line">
      <div class="flex items-center justify-between border-b border-line-subtle px-4 py-3">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Roles</span>
        <Button size="sm" variant="outline" class="w-8 px-0" onclick={blank} title="New role" aria-label="New role">
          <Plus size={14} />
        </Button>
      </div>
      <div class="min-h-0 flex-1 overflow-y-auto p-2">
        {#if loadErr}
          <p class="px-2 py-3 text-xs" style:color="var(--theme-danger)">{loadErr}</p>
        {:else if loading}
          <p class="px-2 py-3 font-mono text-[10px] uppercase tracking-[0.08em] text-muted">Loading</p>
        {:else}
          {#if own.length}
            <div class="px-2 pb-1 pt-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Your organization</div>
            <div use:listStagger>
              {#each own as t (t.slug)}
                <div class="group flex items-center gap-1">
                  <button
                    type="button"
                    onclick={() => pick(t)}
                    class={cn(
                      'min-w-0 flex-1 truncate rounded-md px-2 py-1.5 text-left font-sans text-[13px] transition-colors',
                      selected === t.slug ? 'bg-hover text-fg' : 'text-muted hover:bg-hover hover:text-fg',
                    )}
                  >
                    {t.name}
                  </button>
                  <button
                    type="button"
                    onclick={() => void remove(t.slug)}
                    disabled={busy}
                    title="Delete this role"
                    aria-label="Delete {t.name}"
                    class="shrink-0 rounded-md p-1.5 text-muted opacity-0 transition-opacity hover:text-fg group-hover:opacity-100"
                  >
                    <Trash2 size={13} />
                  </button>
                </div>
              {/each}
            </div>
          {/if}
          <div class="px-2 pb-1 pt-3 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Common roles</div>
          <div use:listStagger>
            {#each builtIn as t (t.slug)}
              <button
                type="button"
                onclick={() => pick(t)}
                class={cn(
                  'block w-full truncate rounded-md px-2 py-1.5 text-left font-sans text-[13px] transition-colors',
                  selected === t.slug ? 'bg-hover text-fg' : 'text-muted hover:bg-hover hover:text-fg',
                )}
              >
                {t.name}
              </button>
            {/each}
          </div>
        {/if}
      </div>
    </div>

    <!-- ── The editor ─────────────────────────────────────────────────────── -->
    <div class="flex min-h-0 flex-1 flex-col">
      {#if !draft}
        <div class="grid flex-1 place-items-center p-8">
          <EmptyState
            title="Pick a role, or add one"
            hint="Talaria maintains the common roles. Editing one saves YOUR version under the same name — yours is used from then on, and deleting it restores ours."
          />
        </div>
      {:else}
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
    </div>
</Panel>
