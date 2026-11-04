<script lang="ts">
  import { untrack } from 'svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Sparkles } from '@lucide/svelte'
  import AutoHeight from '@/components/ui/AutoHeight.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import GeneratingSplash from '@/components/ui/GeneratingSplash.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import Input from '@/components/ui/Input.svelte'
  import {
    DESIGN_KEY,
    EMPTY_FIELDS,
    discardDesign,
    elapsedSeconds,
    isConflict,
    landingKey,
    openingStep,
    pendingDesign,
    refineDesign,
    refineReceipt,
    saveDesign,
    startDesign,
    useAgentDesign,
    watchDesign,
    type DraftFields,
  } from '@/lib/agent-design'
  import { errorMessage } from '@/lib/fetch-json'
  import Modal from '@/components/ui/Modal.svelte'
  import RichEditor from '@/components/ui/RichEditor.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import { hireFleetAgent, useFleetDefs, type AgentDef } from '@/lib/fleet-defs'
  import { useRoleTemplates } from '@/lib/agent-role-templates'
  import { fade, listStagger, slide, staggerIn } from '@/lib/motion'
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
  // THE DESIGN IS THE SERVER'S (talaria-agent-design, lib/agent-design.ts).
  // A describe or a refine is a run; the record — status, chat, draft, error,
  // the start time — lives in Redis. This component reads it and keeps only
  // view state: the step, the form as it is typed, a clock for the elapsed
  // count. Closing the modal, reloading, a second tab: the next open resumes
  // where the design stands. Hand edits are saved to the record as they are
  // typed, so they survive the same things.
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
  const designQuery = useAgentDesign()
  // This modal is looking: reads say so, and a landing skips the bell.
  $effect(() => watchDesign())

  const read = $derived(designQuery.data)
  // DUPLICATE IS ITS OWN FORM: a preselected chassis never reads, writes or
  // spends the person's waiting design. And a design a hire is spending is
  // not offered either (the hiring strip has it).
  const design = $derived(preselect ? null : pendingDesign(read?.design))
  const generating = $derived(design?.status === 'generating')
  const genErr = $derived(design?.status === 'error' ? design.error : null)
  // A design handed back by a failed hire says why on the review step.
  const hireErr = $derived(design?.status === 'ready' ? design.error : null)
  const generated = $derived((design?.chatLength ?? 0) > 0)
  const lastChange = $derived(refineReceipt(design))
  // The first read failed (a 500, a deploy in progress): say so, offer a
  // retry, and leave describe and "Configure manually" open meanwhile.
  const readErr = $derived(!preselect && designQuery.isError && !read ? errorMessage(designQuery.error) : null)

  // The step: chosen from the record once it is known (the roster usually
  // read it already, so this is the first frame), then moved by the person
  // and by a landing. Null only while the very first read is out.
  // svelte-ignore state_referenced_locally -- reason: the opening step is chosen once from the preselect and the cached record; the modal remounts per open
  let step = $state<'describe' | 'review' | null>(preselect ? 'review' : read ? openingStep(pendingDesign(read.design), false) : null)
  $effect(() => {
    if (step !== null) return
    if (read) step = openingStep(pendingDesign(read.design), false)
    else if (designQuery.isError) step = 'describe'
  })

  // The elapsed count, on the server's clock: the same number in every tab
  // and after a reload. A 1 s tick only while something generates.
  let clock = $state(Date.now())
  $effect(() => {
    if (!generating) return
    const t = setInterval(() => (clock = Date.now()), 1000)
    return () => clearInterval(t)
  })
  const genSeconds = $derived(generating ? elapsedSeconds(design, read?.skewMs ?? 0, clock) : 0)

  // svelte-ignore state_referenced_locally -- reason: seeded once at mount from the record's purpose; editing must not fight later reads
  let purpose = $state(design?.purpose ?? '')
  let startErr = $state<string | null>(null)

  // THE FORM — view state over the record's draft. Seeded from the server
  // when a run lands (or at mount), never on the answer to a hand edit's
  // save: that would overwrite what is being typed.
  // svelte-ignore state_referenced_locally -- reason: seeded once at mount from the record's draft; later landings reseed through the effect below
  let form = $state<DraftFields>({ ...EMPTY_FIELDS, ...(design?.draft ?? {}) })
  // The landing the form was seeded from; null until the first read is in.
  // svelte-ignore state_referenced_locally -- reason: read once at mount; the effect below moves it
  let seededFrom = $state<string | null>(read ? landingKey(design) : null)
  let soulRev = $state(0)
  $effect(() => {
    const key = landingKey(design)
    const d = design
    const loaded = read !== undefined
    untrack(() => {
      if (!loaded) return
      if (seededFrom === null) {
        // The first read, for a modal opened before it was in.
        seededFrom = key
        if (d?.draft) {
          form = { ...EMPTY_FIELDS, ...d.draft }
          soulRev += 1
        }
        return
      }
      if (!d || key === seededFrom) return
      const wasGenerating = seededFrom.endsWith(':-')
      seededFrom = key
      // A run starting changes nothing on the form, and a record no run
      // wrote is the answer to the person's own save.
      if (d.status === 'generating' || !d.runId) return
      if (d.draft) {
        form = { ...EMPTY_FIELDS, ...d.draft }
        soulRev += 1
      }
      // A describe that lands while the modal is open moves it to review;
      // a failure stays where its sentence shows.
      if (wasGenerating && d.status === 'ready' && step === 'describe') step = 'review'
    })
  })

  // Hand edits: the form now, the record shortly after (one save per pause).
  // SAVED AS YOU TYPE means no edit is ever lost: a pending save is FLUSHED,
  // never dropped — on close, on Create, and as the page goes away (with
  // `keepalive`, so it outlives the tab).
  let saveTimer: ReturnType<typeof setTimeout> | null = null
  let saveErr = $state<string | null>(null)
  const save = (keepalive = false): Promise<void> => {
    // A run is writing the record (a refine): its landing reseeds the form,
    // and the edits being typed rode in with the refine.
    if (generating || preselect) return Promise.resolve()
    return saveDesign(qc, $state.snapshot(form), design?.id, keepalive ? { keepalive: true } : undefined)
      .then(() => {
        saveErr = null
      })
      .catch(async (e: unknown) => {
        saveErr = errorMessage(e)
        // The design changed elsewhere (or a hire holds it): show the one
        // the server holds now, rather than keep typing into a ghost.
        if (isConflict(e)) {
          await qc.refetchQueries({ queryKey: DESIGN_KEY })
          seededFrom = null
        }
      })
  }
  const flush = (keepalive = false): Promise<void> => {
    if (!saveTimer) return Promise.resolve()
    clearTimeout(saveTimer)
    saveTimer = null
    return save(keepalive)
  }
  const persist = () => {
    // Duplicate holds its own form, and a look at "Configure manually" that
    // never named anything is not a design worth keeping for three days.
    if (preselect || generating) return
    if (!design && !form.name.trim() && !form.handle.trim()) return
    if (saveTimer) clearTimeout(saveTimer)
    saveTimer = setTimeout(() => {
      saveTimer = null
      void save()
    }, 600)
  }
  $effect(() => {
    const away = () => void flush(true)
    window.addEventListener('pagehide', away)
    return () => {
      window.removeEventListener('pagehide', away)
      // The modal closing (Cancel, the X, a navigation) saves what was typed.
      void flush(true)
    }
  })

  const set = (patch: Partial<DraftFields>) => {
    // An editor's autosave of the value it was seeded with is not an edit.
    if ((Object.keys(patch) as Array<keyof DraftFields>).every((k) => JSON.stringify(form[k]) === JSON.stringify(patch[k]))) return
    form = { ...form, ...patch }
    persist()
  }

  const onName = (v: string) => {
    const prev = form.name
    const handle = !form.handle || form.handle === prev.toLowerCase().replace(/[^a-z0-9]/g, '') ? v.toLowerCase().replace(/[^a-z0-9]/g, '') : form.handle
    set({ name: v, handle })
  }

  // A department is ONE agent's: it names the container and the memory
  // volume, so the hire door refuses one another agent holds (409). Said the
  // moment it is typed, from the whole roster — the page's own cached read,
  // retired agents included, because a retired agent keeps its volume and so
  // its department (`templates` may be the enabled ones only). A hirer reads
  // the whole fleet. A role template or a Muse design can propose a
  // department somebody already has.
  const rosterQuery = useFleetDefs(true)
  const departmentHolder = $derived((rosterQuery.data?.defs ?? templates).find((t) => t.department === form.department.trim()) ?? null)

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
    // A role pick is a manual start, not a design: the server holds it as a
    // ready record with no run behind it, so it survives a reload too.
    form = { name: t.name, handle: t.slug.replace(/-/g, ''), department: t.department, role: t.role, soul: t.soul, skills: [] }
    soulRev += 1
    persist()
    step = 'review'
  }

  const describe = async () => {
    startErr = null
    try {
      const d = await startDesign(qc, purpose)
      seededFrom = landingKey(d)
    } catch (e) {
      startErr = errorMessage(e)
    }
  }

  const refine = async (text: string) => {
    startErr = null
    if (!design) return
    // The pending edits ride in the refine's `current`; a save fired into
    // the run it starts would only be refused.
    if (saveTimer) clearTimeout(saveTimer)
    saveTimer = null
    try {
      const d = await refineDesign(qc, design.id, text, $state.snapshot(form))
      seededFrom = landingKey(d)
    } catch (e) {
      startErr = errorMessage(e)
      if (isConflict(e)) {
        await qc.refetchQueries({ queryKey: DESIGN_KEY })
        seededFrom = null
      }
    }
  }

  // Discard: the record goes, a run still generating is cancelled, and the
  // bell row about it is read. Start over from an empty describe.
  const discard = async () => {
    startErr = null
    try {
      await discardDesign(qc, design?.id)
      form = { ...EMPTY_FIELDS }
      seededFrom = ''
      purpose = ''
      roleSlug = ''
      step = 'describe'
    } catch (e) {
      startErr = errorMessage(e)
    }
  }

  // Enqueue the hire and close. The boot (render, up, health wait — minutes on
  // a cold pull) is the run's work, not the modal's: holding the modal open
  // for it is what made creation feel broken — a stuck spinner, a proxy
  // timeout, an agent that only existed after a refresh. The roster strip
  // shows the phases; the only error that belongs HERE is one the open form
  // can fix (a taken handle). The hire names the design: the server spends
  // it on this hire (a second one is refused), consumes it when the hire
  // finishes, and hands it back here if the hire fails.
  const create = async () => {
    err = null
    busy = true
    try {
      // The latest edits reach the record first, so a refusal below leaves
      // the design exactly as the form shows it.
      await flush()
      const f = form
      const r = await hireFleetAgent({
        slug: f.handle,
        department: f.department,
        displayName: f.name,
        role: f.role.trim() || null,
        ...(templateId ? { templateId } : {}),
        ...(f.soul.trim() ? { soul: f.soul } : {}),
        ...(f.skills.length ? { skills: f.skills } : {}),
        ...(design ? { designId: design.id } : {}),
        start,
      })
      if (r.error) {
        err = r.error
        // Any refusal refetches the roster. The one that matters is the one
        // the hint could not see coming (the department was taken after this
        // page read the roster, a 409 at the door): with the fresh roster the
        // field says who holds it and Create waits. For any other refusal it
        // is one extra read.
        void qc.invalidateQueries({ queryKey: ['fleet-defs'] })
        return
      }
      await qc.invalidateQueries({ queryKey: ['fleet-hires'] })
      void qc.invalidateQueries({ queryKey: DESIGN_KEY })
      onClose()
    } catch (e) {
      err = (e as Error).message
    } finally {
      busy = false
    }
  }
</script>

{#if step === null}
  <!-- The first read of the design is still out (the roster usually has it
       cached, so this is rare): say so rather than guess a step. -->
  <Modal {open} {onClose} title="New agent" width="max-w-lg">
    <div class="flex items-center gap-2 py-6 text-sm text-muted">
      <WaitingMark site="fleet/agent-design" size={12} />
      Checking for a design in progress
    </div>
  </Modal>
{:else if step === 'describe'}
  <!-- ── Step 1: describe ─────────────────────────────────────────────────── -->
  <Modal {open} {onClose} title="New agent" width="max-w-lg">
    <!-- The body's two faces — the describe form, and the full-panel splash
         the moment generation starts (TALA-11). Keyed on `generating` with
         the stagger as the entrance (ANIMATIONS.md step grammar), and the
         AutoHeight wrapper is load-bearing: the splash is far taller than
         the form, and a resize the user watches must glide. The footer row
         stays outside the key so the Designing button (its WaitingMark) and
         Cancel remain reachable for the whole wait — and Cancel now really
         is an exit, not a cancel: the generation keeps running on the
         server, and the roster's row, the New agent mark and the bell all
         lead back to it. Discard is the one that stops it. -->
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
    {#if genErr || startErr}<p transition:slide={{ duration: 150 }} class="text-xs text-danger">{genErr ?? startErr}</p>{/if}
    {#if readErr}
      <p transition:slide={{ duration: 150 }} class="text-xs text-danger" data-testid="design-read-error">
        Could not check for a design in progress: {readErr}
        <button type="button" class="ml-1 underline hover:text-fg" onclick={() => void designQuery.refetch()}>Retry</button>
      </p>
    {/if}
    <div class="flex items-center gap-3 border-t border-line pt-4">
      {#if generating}
        <button type="button" class="text-xs text-muted hover:text-danger" onclick={() => void discard()} data-testid="design-discard">
          Discard design
        </button>
      {:else}
        <button type="button" class="text-xs text-muted hover:text-fg" onclick={() => (step = 'review')}>
          Configure manually →
        </button>
      {/if}
      <span class="ml-auto"></span>
      <Button variant="ghost" size="sm" onclick={onClose}>
        {generating ? 'Close' : 'Cancel'}
      </Button>
      <Button onclick={() => (generating ? undefined : void describe())} disabled={generating || !purpose.trim()}>
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
          <Input id="cam-name" value={form.name} oninput={(e) => onName(e.currentTarget.value)} placeholder="Research Analyst" autofocus={!generated} />
        </div>
        <div>
          <label for="cam-handle" class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Handle</label>
          <Input id="cam-handle" value={form.handle} oninput={(e) => set({ handle: e.currentTarget.value })} placeholder="analyst" />
        </div>
      </div>
      <div class="grid grid-cols-2 gap-4">
        <div>
          <label for="cam-role" class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Role</label>
          <Input id="cam-role" value={form.role} oninput={(e) => set({ role: e.currentTarget.value })} placeholder="Research Analyst" />
        </div>
        <div>
          <label for="cam-department" class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Department</label>
          <Input id="cam-department" value={form.department} oninput={(e) => set({ department: e.currentTarget.value })} placeholder="research" aria-invalid={departmentHolder ? 'true' : undefined} />
          {#if departmentHolder}
            <p transition:slide={{ duration: 150 }} class="mt-1 text-xs text-danger" data-testid="department-taken">
              Taken by {departmentHolder.displayName} — pick another.
            </p>
          {/if}
        </div>
      </div>
      <p class="-mt-2 font-sans text-xs text-muted">
        Role is the roster title; department is the agent's own key — it names its container and its memory, so no two
        agents share one. The fleet model id becomes
        <span class="font-mono text-fg">{form.handle || 'handle'}-{form.department || 'department'}</span>.
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
        {#if form.soul.trim()}
          <!-- Rich like the post-creation soul editor; autosave keeps `soul`
               fresh for create + refine, reseeded whenever muse redrafts. -->
          <div class="max-h-72 overflow-y-auto">
            {#key soulRev}
              <RichEditor value={form.soul} onSave={(md) => set({ soul: md })} autosave minHeight="9rem" />
            {/key}
          </div>
        {:else}
          <p class="text-xs text-muted">Starts from a scaffold you edit after creation, or go back and describe the agent to have one designed.</p>
        {/if}
      </div>

      {#if form.skills.length > 0}
        <div in:fade={{ duration: 150 }}>
          <span class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Starter skills</span>
          <ul class="divide-y divide-line rounded-lg border border-line" use:listStagger>
            {#each form.skills as s (s.name)}
              <SkillPreviewRow skill={s} onRemove={() => set({ skills: form.skills.filter((x) => x.name !== s.name) })} />
            {/each}
          </ul>
        </div>
      {/if}

      {#if generated}
        <RefineBar
          busy={generating}
          preview={null}
          error={genErr ?? startErr}
          onRefine={(text) => void refine(text)}
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
      {#if hireErr}<div transition:slide={{ duration: 150 }} class="text-sm text-danger" data-testid="design-hire-error">{hireErr}</div>{/if}
      {#if err || saveErr}<div transition:slide={{ duration: 150 }} class="text-sm text-danger">{err ?? `Your edits were not saved: ${saveErr}`}</div>{/if}
      <div class="flex items-center gap-2 border-t border-line pt-4">
        {#if !preselect}
          <button type="button" class="text-xs text-muted hover:text-fg" onclick={() => (step = 'describe')}>
            ← Describe instead
          </button>
        {/if}
        {#if design && !preselect}
          <button type="button" class="text-xs text-muted hover:text-danger" onclick={() => void discard()} data-testid="design-discard">
            Discard design
          </button>
        {/if}
        <span class="ml-auto"></span>
        <Button variant="ghost" size="sm" onclick={onClose}>
          Cancel
        </Button>
        <Button onclick={() => void create()} disabled={busy || generating || !form.handle || !form.department || !form.name || !!departmentHolder}>
          {busy ? 'Hiring' : 'Create agent'}
        </Button>
      </div>
    </div>
  </Modal>
{/if}
