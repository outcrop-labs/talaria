<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Sparkles } from '@lucide/svelte'
  import AutoHeight from '@/components/ui/AutoHeight.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import GeneratingSplash from '@/components/ui/GeneratingSplash.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import Input from '@/components/ui/Input.svelte'
  import {
    adoptDesign,
    clearDesign,
    currentDesign,
    refineDesign,
    setModalOpen,
    startDesign,
  } from '@/lib/agent-design.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import RichEditor from '@/components/ui/RichEditor.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import { hireFleetAgent, type AgentDef } from '@/lib/fleet-defs'
  import { useRoleTemplates } from '@/lib/agent-role-templates'
  import { fade, listStagger, slide, staggerIn } from '@/lib/motion'
  import type { AgentDraft } from '@/lib/muse.svelte'
  import RefineBar from './RefineBar.svelte'
  import SkillPreviewRow from './SkillPreviewRow.svelte'

  // Spin up a brand-new agent two ways: DESCRIBE it (the AI designs the whole
  // agent — identity, soul, starter skills — for review before anything is
  // created) or start from a ROLE TEMPLATE and adjust.
  //
  // TWO DIFFERENT "TEMPLATES", and they are different axes:
  //   • ROLE template — what the agent is FOR. Prefills name, handle, role,
  //     department and a starter soul from a common business role (Talaria
  //     maintains a set; an org adds its own). This is the one that helps on a
  //     fresh install, where there is no existing agent to copy. Offered on
  //     the DESCRIBE step — it is an entry path, a peer of describing.
  //   • CHASSIS template — what it RUNS ON. Clones an existing agent's model
  //     tiers, tools and plugins with the identity re-stamped. Unavailable
  //     until at least one agent exists, which is exactly why it could never
  //     be the only answer. Stays on the review step.
  //
  // Create does not boot anything: it enqueues an agent-hire RUN and closes.
  // The roster's hiring strip shows the phases from there.
  //
  // TALA-11 part 2: the generation survives the modal. The whole draftAgent()
  // orchestration — fields, chat window, elapsed tick, refine receipt — lives
  // in lib/agent-design.svelte (module state, next to the toast store),
  // because this component REMOUNTS per open (Agents.svelte renders it under
  // `{#if creating}`) and a background run must not die with its window.
  // Closing mid-generation leaves the store running; reopening re-enters it.
  let {
    open,
    onClose,
    templates,
    templateId: preselect,
  }: {
    open: boolean
    onClose: () => void
    templates: AgentDef[]
    /** Preselect a template (e.g. "Duplicate" from a specific agent) — skips the describe step. */
    templateId?: string
  } = $props()

  const qc = useQueryClient()
  // svelte-ignore state_referenced_locally -- reason: opening step chosen from the preselect/store once at mount; the modal remounts per open
  let step = $state<'describe' | 'review'>(preselect ? 'review' : 'describe')

  // The store's design, or null. Every reactive read below flows from here —
  // the modal owns NO generation state of its own any more.
  const design = $derived(currentDesign())
  const generating = $derived(design?.status === 'generating')
  const genSeconds = $derived(design?.genSeconds ?? 0)
  const genErr = $derived(design?.status === 'error' ? design.error : null)
  const generated = $derived((design?.chat.length ?? 0) > 0)
  // svelte-ignore state_referenced_locally -- reason: seeded once at mount from the store's purpose; the modal remounts per open, and editing must not fight the store
  let purpose = $state(design?.purpose ?? '')
  // Report open/closed to the store — it gates the "the design landed" toast
  // (the person standing in the modal already knows). setModalOpen on mount
  // AND close, because the component unmounts when the modal goes away.
  setModalOpen(true)
  $effect(() => {
    return () => setModalOpen(false)
  })

  // A design that landed while the modal was closed: reopen straight onto the
  // review step, fields applied — the same applyDraft path as before, just
  // already-applied by the store. An error lands the person back on describe
  // svelte-ignore state_referenced_locally -- reason: opening step chosen from the store once at mount (the modal remounts per open); later status changes are handled reactively via `generating`
  if (design?.status === 'ready') step = 'review'
  // Review fields, bound to the store's fields. Hand edits write straight
  // through to the store (that is what the refine's `current` reads), except
  // templateId/start/busy/err, which are genuinely per-hire local state.
  const displayName = $derived(design?.name ?? '')
  const slug = $derived(design?.handle ?? '')
  const department = $derived(design?.department ?? '')
  const role = $derived(design?.role ?? '')
  const soul = $derived(design?.soul ?? '')
  const soulRev = $derived(design?.soulRev ?? 0)
  const skills = $derived(design?.skills ?? [])
  const lastChange = $derived(design?.lastChange ?? null)

  const set = (patch: Partial<{ name: string; handle: string; department: string; role: string; soul: string; skills: AgentDraft['skills'] }>) => {
    const d = currentDesign()
    if (!d) return
    Object.assign(d, patch)
  }

  const onName = (v: string) => {
    const d = currentDesign()
    if (!d) return
    const prev = d.name
    d.name = v
    if (!d.handle || d.handle === prev.toLowerCase().replace(/[^a-z0-9]/g, '')) {
      d.handle = v.toLowerCase().replace(/[^a-z0-9]/g, '')
    }
  }
  // svelte-ignore state_referenced_locally -- reason: chassis pick seeded from the preselect/roster once at mount; the modal remounts per open
  let templateId = $state(preselect ?? templates[0]?.id ?? '') // '' = platform defaults
  // Role templates, as a query (cached across opens, no hand-rolled fetch to
  // silently fail). Choosing one FILLS the fields rather than binding to them
  // — every value stays editable, and a template is a starting point, not a
  // mode you are stuck in.
  const roleTemplatesQuery = useRoleTemplates()
  const roleTemplates = $derived(roleTemplatesQuery.data ?? [])
  let roleSlug = $state('')
  let start = $state(true)
  let busy = $state(false)
  let err = $state<string | null>(null)

  const applyRole = (slugPicked: string) => {
    roleSlug = slugPicked
    const t = roleTemplates.find((x) => x.slug === slugPicked)
    if (!t) return
    // A role pick is a manual start, not a design: it seeds the store with
    // a ready record and no muse run, so the review step's store bindings
    // and the refine's `current` read it like any other starting point.
    adoptDesign({
      name: t.name,
      handle: t.slug.replace(/-/g, ''),
      department: t.department,
      role: t.role,
      soul: t.soul,
    })
    step = 'review'
  }

  // Enqueue the hire and close. The boot (render, up, health wait — minutes on
  // a cold pull) is the run's work, not the modal's: holding the modal open
  // for it is what made creation feel broken — a stuck spinner, a proxy
  // timeout, an agent that only existed after a refresh. The roster strip
  // shows the phases; the only error that belongs HERE is one the open form
  // can fix (a taken handle). The design is claimed — the store's run is
  // over and the hire's phases take over the story.
  const create = async () => {
    err = null
    busy = true
    try {
      const r = await hireFleetAgent({
        slug,
        department,
        displayName,
        role: role.trim() || null,
        ...(templateId ? { templateId } : {}),
        ...(soul.trim() ? { soul } : {}),
        ...(skills.length ? { skills } : {}),
        start,
      })
      if (r.error) {
        err = r.error
        return
      }
      await qc.invalidateQueries({ queryKey: ['fleet-hires'] })
      clearDesign()
      onClose()
    } catch (e) {
      err = (e as Error).message
    } finally {
      busy = false
    }
  }
</script>

{#if step === 'describe'}
  <!-- ── Step 1: describe ─────────────────────────────────────────────────── -->
  <Modal {open} {onClose} title="New agent" width="max-w-lg">
    <!-- The body's two faces — the describe form, and the full-panel splash
         the moment generation starts (TALA-11). Keyed on `generating` with
         the stagger as the entrance (ANIMATIONS.md step grammar), and the
         AutoHeight wrapper is load-bearing: the splash is far taller than
         the form, and a resize the user watches must glide. The footer row
         stays outside the key so the Designing button (its WaitingMark) and
         Cancel remain reachable for the whole wait — and Cancel now really
         is an exit, not a cancel: the generation keeps running in the store
         and the roster's status row re-enters it. -->
    <AutoHeight>
      {#key generating}
        {#if generating}
          <!-- No token stream to show: an agent design is a JSON contract, so
               this is the same progress moment the inline block carried, at
               the scale of the thing being made. Drift says working, never
               how far along; the elapsed count (once it is genuinely taking
               a while) is what separates "working, slowly" from "wedged". -->
          <div use:staggerIn>
            <!-- What was asked for, kept visible on a reopen: the splash says
                 the work is running, the purpose line says what it is for. -->
            {#if design?.purpose}
              <p class="mb-3 truncate font-sans text-xs text-muted">{design.purpose}</p>
            {/if}
            <GeneratingSplash
              label="Designing the agent: identity, soul, and starter skills"
              seconds={genSeconds}
              site="fleet/agent-design"
            />
          </div>
        {:else}
          <div class="space-y-5" use:staggerIn>
            <p class="text-sm leading-relaxed text-muted">
              Describe what this agent should do: its job, what it watches, what it produces. The AI designs the whole
              agent (identity, soul, starter skills) for you to review before anything is created.
            </p>
            <div class="flex items-end gap-2.5">
              <Sparkles size={14} class="mb-3 shrink-0 text-accent" />
              <Textarea
                autoGrow
                rows={3}
                bind:value={purpose}
                placeholder="e.g. “A release manager that tracks our deploy trains, chases sign-offs before each cut, and posts a go/no-go summary.”"
                class="max-h-48 text-sm"
                autofocus
              />
            </div>
            <!-- The other entry path, a peer of describing: pick a role, the fields
                 fill, the review step opens. Nothing is bound — everything stays
                 editable over there. Hidden while generating (the splash owns the
                 body), so its old `!generating` guard now lives in the branch. -->
            {#if roleTemplates.length}
              <div class="flex items-center gap-2.5">
                <span class="shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">or start from a role</span>
                <Select
                  bind:value={roleSlug}
                  onchange={() => roleSlug && applyRole(roleSlug)}
                  class="min-w-0 flex-1"
                  aria-label="Start from a role template"
                >
                  <option value="">Pick a role…</option>
                  {#if roleTemplates.some((t) => !t.builtIn)}
                    <optgroup label="Your organization">
                      {#each roleTemplates.filter((t) => !t.builtIn) as t (t.slug)}
                        <option value={t.slug}>{t.name}</option>
                      {/each}
                    </optgroup>
                  {/if}
                  <optgroup label="Common roles">
                    {#each roleTemplates.filter((t) => t.builtIn) as t (t.slug)}
                      <option value={t.slug}>{t.name}</option>
                    {/each}
                  </optgroup>
                </Select>
              </div>
            {/if}
          </div>
        {/if}
      {/key}
    </AutoHeight>
    {#if genErr}<p transition:slide={{ duration: 150 }} class="text-xs text-danger">{genErr}</p>{/if}
    <div class="flex items-center gap-3 border-t border-line pt-4">
      <button type="button" class="text-xs text-muted hover:text-fg" onclick={() => (step = 'review')}>
        Configure manually →
      </button>
      <span class="ml-auto"></span>
      <Button variant="ghost" size="sm" onclick={onClose}>
        Cancel
      </Button>
      <Button onclick={() => (generating ? undefined : startDesign(purpose))} disabled={generating || !purpose.trim()}>
        {#if generating}<WaitingMark site="fleet/agent-create" size={12} />{/if}
        {generating ? 'Designing' : 'Design agent'}
      </Button>
    </div>
  </Modal>
{:else}
  <!-- ── Step 2: review + create ──────────────────────────────────────────── -->
  <Modal {open} {onClose} title="New agent" takeover>
    <div class="space-y-5">
      <div class="grid grid-cols-2 gap-4">
        <div>
          <label for="cam-name" class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Name</label>
          <Input id="cam-name" value={displayName} oninput={(e) => onName(e.currentTarget.value)} placeholder="Research Analyst" autofocus={!generated} />
        </div>
        <div>
          <label for="cam-handle" class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Handle</label>
          <Input id="cam-handle" value={slug} oninput={(e) => set({ handle: e.currentTarget.value })} placeholder="analyst" />
        </div>
      </div>
      <div class="grid grid-cols-2 gap-4">
        <div>
          <label for="cam-role" class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Role</label>
          <Input id="cam-role" value={role} oninput={(e) => set({ role: e.currentTarget.value })} placeholder="Research Analyst" />
        </div>
        <div>
          <label for="cam-department" class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Department</label>
          <Input id="cam-department" value={department} oninput={(e) => set({ department: e.currentTarget.value })} placeholder="research" />
        </div>
      </div>
      <p class="-mt-2 font-sans text-xs text-muted">
        Role is the roster title; department is the routing/mount key. The fleet model id becomes
        <span class="font-mono text-fg">{slug || 'handle'}-{department || 'department'}</span>.
      </p>

      <div>
        <label for="cam-template" class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
          Chassis template <span class="normal-case">(model tiers, tools, and plugins carry over)</span>
        </label>
        <Select id="cam-template" bind:value={templateId} class="w-full">
          <option value="">Platform defaults: chassis + first local model</option>
          {#each templates as t (t.id)}
            <option value={t.id}>{t.displayName} · {t.department} (v{t.currentVersion})</option>
          {/each}
        </Select>
      </div>

      <div>
        <span class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Soul</span>
        {#if soul.trim()}
          <!-- Rich like the post-creation soul editor; autosave keeps `soul`
               fresh for create + refine, reseeded whenever muse redrafts. -->
          <div class="max-h-72 overflow-y-auto">
            {#key soulRev}
              <RichEditor value={soul} onSave={(md) => set({ soul: md })} autosave minHeight="9rem" />
            {/key}
          </div>
        {:else}
          <p class="text-xs text-muted">Starts from a scaffold you edit after creation, or go back and describe the agent to have one designed.</p>
        {/if}
      </div>

      {#if skills.length > 0}
        <div in:fade={{ duration: 150 }}>
          <span class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Starter skills</span>
          <ul class="divide-y divide-line rounded-lg border border-line" use:listStagger>
            {#each skills as s (s.name)}
              <SkillPreviewRow skill={s} onRemove={() => set({ skills: skills.filter((x) => x.name !== s.name) })} />
            {/each}
          </ul>
        </div>
      {/if}

      {#if generated}
        <RefineBar
          busy={generating}
          preview={null}
          error={genErr}
          onRefine={(text) => refineDesign(text)}
        />
      {/if}

      {#if lastChange}
        <!-- THE REFINED RECEIPT: what the accepted draft changed, in place of
             the silent field swap this step used to do. -->
        <div
          transition:slide={{ duration: 150 }}
          class="flex items-center gap-2 rounded-md border border-line-subtle bg-surface px-2.5 py-1.5 text-[11px] text-muted"
          data-refine-applied="visible"
        >
          <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Refine applied</span>
          <span class="truncate">
            {#if lastChange.fields.length > 0}
              {lastChange.fields.map((f) => f.label).join(', ')}
              {#if !lastChange.soul.oversized}
                · {lastChange.soul.text}
              {/if}
            {:else if lastChange.soul.text !== 'no text changes'}
              {lastChange.soul.text}
            {:else}
              no changes to the design
            {/if}
          </span>
        </div>
      {/if}

      <Checkbox checked={start} onChange={(checked) => (start = checked)} label="Start the container now" class="gap-2 text-sm text-fg" />
      {#if err}<div transition:slide={{ duration: 150 }} class="text-sm text-danger">{err}</div>{/if}
      <div class="flex items-center gap-2 border-t border-line pt-4">
        {#if !preselect}
          <button type="button" class="text-xs text-muted hover:text-fg" onclick={() => (step = 'describe')}>
            ← Describe instead
          </button>
        {/if}
        <span class="ml-auto"></span>
        <Button variant="ghost" size="sm" onclick={onClose}>
          Cancel
        </Button>
        <Button onclick={() => void create()} disabled={busy || generating || !slug || !department || !displayName}>
          {busy ? 'Hiring' : 'Create agent'}
        </Button>
      </div>
    </div>
  </Modal>
{/if}
