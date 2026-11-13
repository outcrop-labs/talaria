<script lang="ts">
  import { untrack } from 'svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import AutoHeight from '@/components/ui/AutoHeight.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import GeneratingSplash from '@/components/ui/GeneratingSplash.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import Input from '@/components/ui/Input.svelte'
  import {
    DESIGN_KEY,
    EMPTY_FIELDS,
    didNotFinish,
    discardDesign,
    elapsedSeconds,
    isConflict,
    keepaliveBody,
    keepsTypedInput,
    landingKey,
    leftTheSlot,
    openingStep,
    pendingDesign,
    refineDesign,
    refineReceipt,
    saveDesign,
    stashUnsaved,
    startDesign,
    takeUnsaved,
    useAgentDesign,
    dismissReturned,
    watchDesign,
    type DraftFields,
  } from '@/lib/agent-design'
  import { cn } from '@/lib/cn'
  import { errorMessage } from '@/lib/fetch-json'
  import { useSession } from '@/lib/session'
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
  // Duplicate looks at no design, so it must not hold the bell back either.
  $effect(() => (preselect ? undefined : watchDesign()))

  const read = $derived(designQuery.data)
  // DUPLICATE IS ITS OWN FORM: a preselected chassis never reads, writes or
  // spends the person's waiting design. A design on a hire is out of the
  // slot altogether, so New agent is open for the next one meanwhile.
  const slot = $derived(preselect ? null : pendingDesign(read?.design))
  // A design a failed hire handed back into the slot while the person was
  // typing into an empty New agent is OFFERED, never taken in over the
  // typing (`keepsTypedInput`): the form stays theirs, and every verb here
  // acts on `design`, the one the form belongs to — none. Opening it is the
  // person's choice; a describe or a manual start sets it aside to wait.
  let offered = $state<string | null>(null)
  const design = $derived(slot && slot.id !== offered ? slot : null)
  const arrived = $derived(slot && slot.id === offered ? slot : null)
  // Designs a failed hire handed back while this one was open: each waits
  // for the slot, and can be dismissed.
  const returned = $derived(preselect ? [] : (read?.returned ?? []))
  const session = useSession()
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
    const key = landingKey(slot)
    const d = slot
    const loaded = read !== undefined
    untrack(() => {
      if (!loaded) return
      // The offered design left the slot (set aside by what was typed here,
      // discarded, or hired elsewhere): nothing is offered any more.
      if (offered && d?.id !== offered) offered = null
      if (seededFrom === null) {
        // The first read, for a modal opened before it was in (or the read
        // after a 409 brought the current design — or none: then the form's
        // fields belong to nothing any more, and go).
        if (!d && conflictReseed) resetForm(null)
        conflictReseed = false
        seededFrom = key
        if (d?.draft) {
          form = { ...EMPTY_FIELDS, ...d.draft }
          soulRev += 1
        }
        return
      }
      if (!d) {
        // The design this modal showed left the slot: a hire took it (another
        // tab's Create), or it ended elsewhere. Its fields go with it, so no
        // keystroke here saves them as a manual copy of a design being hired.
        const left = leftTheSlot(seededFrom.split(':')[0] ?? '', read)
        if (left) resetForm(left.hiredAs)
        return
      }
      if (d.id === offered || key === seededFrom) return
      // A FAILED HIRE'S DESIGN, handed back into what the server saw as an
      // empty slot, while the person types into this empty New agent: it is
      // offered beside the typing, never put in its place (NN12).
      if (keepsTypedInput(d, seededFrom, $state.snapshot(form), purpose)) {
        offered = d.id
        return
      }
      const wasGenerating = seededFrom.endsWith(':-')
      const prevId = seededFrom.split(':')[0]
      seededFrom = key
      // ANOTHER DESIGN arrived — handed back by a failed hire into an empty
      // slot with nothing typed here, or started in another tab — and not by
      // this modal's own first save of a manual start: take its draft, manual
      // or designed, and open where it stands.
      if (d.id !== prevId && !(prevId === '' && ownSave)) {
        form = { ...EMPTY_FIELDS, ...(d.draft ?? {}) }
        soulRev += 1
        if (d.status !== 'generating') step = openingStep(d, false)
        return
      }
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

  // A design that left for a hire (from another tab) is named on the empty
  // describe step this modal falls back to.
  let leftForHire = $state<string | null>(null)
  let conflictReseed = false
  function resetForm(hiredAs: string | null) {
    if (saveTimer) clearTimeout(saveTimer)
    saveTimer = null
    form = { ...EMPTY_FIELDS }
    soulRev += 1
    purpose = ''
    seededFrom = ''
    leftForHire = hiredAs
    step = 'describe'
  }

  // THE EDIT THAT COULD NOT LEAVE WITH THE PAGE is offered once, when New
  // agent opens — cold or warm cache — and only if the server has not moved
  // past it (`takeUnsaved`). Never later: a 409's reseed must not bring back
  // a stash over edits made since.
  let stashChecked = false
  $effect(() => {
    const loaded = read !== undefined
    const uid = session.data?.id
    untrack(() => {
      if (stashChecked || !loaded || !uid || preselect) return
      stashChecked = true
      restoreUnsaved(uid, design?.id ?? null, design?.draft ?? null)
    })
  })

  // Hand edits: the form now, the record shortly after (one save per pause).
  // SAVED AS YOU TYPE means no edit is ever lost: a pending save is FLUSHED,
  // never dropped — on close and on Create as an ordinary save (the app
  // outlives the modal), and as the page goes away with `keepalive`. A
  // keepalive body is capped at 64 KiB, so an edit too large for it sends
  // the small fields and is kept in this browser (`stashUnsaved`) until New
  // agent next opens on the design; it is never reported saved.
  let saveTimer: ReturnType<typeof setTimeout> | null = null
  let saveErr = $state<string | null>(null)
  let restored = $state(false)
  // A save is on the wire: what pagehide must still keep, and (for a manual
  // start) the record its answer creates is this modal's own.
  let ownSave = false
  // The record this form's nameless save created (a manual start): the one
  // a Create right after it must spend, not leave behind in New agent.
  let made: string | null = null
  // The save on the wire, if any. A flush waits for it as well as for a
  // pending one: a Create that read the design id while a manual start's
  // save was still out went without it, and left a copy of the agent it
  // hired behind in New agent.
  let inFlight: Promise<boolean> | null = null
  /** Resolves true when the edit reached the server. */
  const save = (): Promise<boolean> => {
    // A run is writing the record (a refine): its landing reseeds the form,
    // and the edits being typed rode in with the refine.
    if (generating || preselect) return Promise.resolve(true)
    ownSave = true
    const id = design?.id
    const sent = saveDesign(qc, $state.snapshot(form), id)
      .then((d) => {
        if (!id) made = d.id
        saveErr = null
        return true
      })
      .finally(() => {
        ownSave = false
      })
      .catch(async (e: unknown) => {
        saveErr = errorMessage(e)
        // The design changed elsewhere (or a hire holds it): show the one
        // the server holds now, rather than keep typing into a ghost.
        if (isConflict(e)) {
          // Reseed from whatever the read brings (set first: the effect
          // runs as the read lands).
          conflictReseed = true
          seededFrom = null
          await qc.refetchQueries({ queryKey: DESIGN_KEY })
        }
        return false
      })
    inFlight = sent
    void sent.then(() => {
      if (inFlight === sent) inFlight = null
    })
    return sent
  }
  /** The pending save sent now, or the one already on the wire, answered. */
  const flush = (): Promise<boolean> => {
    if (!saveTimer) return inFlight ?? Promise.resolve(true)
    clearTimeout(saveTimer)
    saveTimer = null
    return save()
  }
  // The page is going away: the one save that must outlive it.
  const flushAway = () => {
    // Pending (the timer has not fired) or already on the wire (it has, and
    // the page may not wait for the answer): either way it is kept here.
    if ((!saveTimer && !ownSave) || generating || preselect) return
    const uid = session.data?.id
    if (saveTimer) clearTimeout(saveTimer)
    const pending = saveTimer !== null
    saveTimer = null
    const now = $state.snapshot(form)
    const base = design?.draft ?? null
    const { fields } = pending ? keepaliveBody(now, base) : { fields: null }
    // Kept here first: a reload can outrun its own keepalive save, and an
    // edit over the cap cannot ride it at all.
    if (uid) stashUnsaved(uid, { id: design?.id ?? null, form: now, base, sent: fields, at: Date.now() })
    if (fields) void saveDesign(qc, fields, design?.id, { keepalive: true }).catch(() => {})
  }
  // An edit that could not leave with the page, offered back on this design.
  function restoreUnsaved(uid: string, id: string | null, server: DraftFields | null) {
    if (preselect) return
    const back = takeUnsaved(uid, id, server)
    if (!back) return
    form = { ...EMPTY_FIELDS, ...back }
    soulRev += 1
    restored = true
    if (step === 'describe' && id) step = 'review'
    persist()
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
    window.addEventListener('pagehide', flushAway)
    return () => {
      window.removeEventListener('pagehide', flushAway)
      // The modal closing (Cancel, the X, a navigation) saves what was
      // typed, as an ordinary save: no size cap, and the app is still here.
      void flush()
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
    leftForHire = null
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
        conflictReseed = true
        seededFrom = null
        await qc.refetchQueries({ queryKey: DESIGN_KEY })
      }
    }
  }

  // Discard: the record goes, a run still generating is cancelled, and the
  // bell row about it is read. Start over from an empty describe.
  const discard = async () => {
    startErr = null
    // A pending save would recreate what is being thrown away.
    if (saveTimer) clearTimeout(saveTimer)
    saveTimer = null
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

  // The offered design (see `offered`), taken in because the person chose
  // it: what they typed here gives way to it, as any open would.
  const openOffered = () => {
    const d = arrived
    if (!d) return
    if (saveTimer) clearTimeout(saveTimer)
    saveTimer = null
    offered = null
    seededFrom = landingKey(d)
    form = { ...EMPTY_FIELDS, ...(d.draft ?? {}) }
    soulRev += 1
    step = openingStep(d, false)
  }
  // …or thrown away, leaving what is typed here where it is.
  const discardOffered = () => {
    const d = arrived
    if (d) discardDesign(qc, d.id).catch((e: unknown) => (startErr = errorMessage(e)))
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
      // The design this form belongs to, named BEFORE the flush: a flush
      // that meets a newer design (another tab or device) refetches it, and
      // its id must never ride out with this form. Such a flush stops the
      // Create; the person sees the current design and decides again.
      const designId = design?.id
      made = null
      if (!(await flush())) {
        err = saveErr ?? 'The design changed before it could be hired; check it and create again.'
        return
      }
      // A manual start the flush itself saved is this form's own record:
      // spent on the hire like any design, not left in New agent behind it.
      const spend = designId ?? made
      const f = form
      const r = await hireFleetAgent({
        slug: f.handle,
        department: f.department,
        displayName: f.name,
        role: f.role.trim() || null,
        ...(templateId ? { templateId } : {}),
        ...(f.soul.trim() ? { soul: f.soul } : {}),
        ...(f.skills.length ? { skills: f.skills } : {}),
        ...(spend ? { designId: spend } : {}),
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
        // A 409 may be the design's own (replaced elsewhere, or a hire from
        // another tab holds it): read it again and show what is current.
        if (r.status === 409 && spend) {
          // Reseed from whatever the read brings (set first: the effect
          // runs as the read lands).
          conflictReseed = true
          seededFrom = null
          await qc.refetchQueries({ queryKey: DESIGN_KEY })
        }
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

{#snippet returnedNotes()}
  <!-- A failed hire handed its design back while this one was open: it
       waits (and the bell said so) rather than overwrite this one, and comes
       back here once this one is hired or discarded. Nothing is lost. -->
  {#each returned as r (r.designId)}
    <p transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-muted" data-testid="design-returned">
      {didNotFinish(r.name, r.reason)}
      Its design comes back here once this one is hired or discarded.
      <button type="button" class="ml-1 underline hover:text-danger" onclick={() => dismissReturned(qc, r.designId).catch((e: unknown) => (startErr = errorMessage(e)))}>Discard it</button>
    </p>
  {/each}
  <!-- A failed hire's design came back into New agent while the person was
       typing into it: offered beside what is typed, never put in its place. -->
  {#if arrived}
    <p transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-muted" data-testid="design-arrived">
      {didNotFinish(arrived.draft?.name.trim() || 'the designed agent', arrived.error)}
      Its design is back:
      <button type="button" class="underline hover:text-fg" onclick={openOffered}>Open it</button>
      in place of what you have typed here, or carry on, and it waits until this one is hired or discarded.
      <button type="button" class="ml-1 underline hover:text-danger" onclick={discardOffered}>Discard it</button>
    </p>
  {/if}
{/snippet}

{#snippet quietLink(label: string, onclick: () => void, danger = false, testid?: string)}
  <button
    type="button"
    class={cn('font-sans text-[13px] leading-5 text-muted transition-colors', danger ? 'hover:text-danger' : 'hover:text-fg')}
    {onclick}
    data-testid={testid}
  >
    {label}
  </button>
{/snippet}

{#if step === null}
  <!-- The first read of the design is still out (the roster usually has it
       cached, so this is rare): say so rather than guess a step. -->
  <Modal {open} {onClose} title="New agent" width="max-w-lg">
    <div class="flex items-center gap-2 py-6 font-sans text-sm text-muted">
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
         the form, and a resize the user watches must glide. The footer
         stays outside the key so the Designing button (its WaitingMark) and
         Cancel remain reachable for the whole wait — and Cancel now really
         is an exit, not a cancel: the generation keeps running on the
         server, and the roster's row, the New agent mark and the bell all
         lead back to it. Discard is the one that stops it. -->
    <div class="space-y-4">
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
                <p class="mb-3 truncate font-sans text-[13px] leading-5 text-muted">{design.purpose}</p>
              {/if}
              <GeneratingSplash
                label="Designing the agent: identity, instructions and starter skills"
                seconds={genSeconds}
                site="fleet/agent-design"
              />
            </div>
          {:else}
            <div class="space-y-5" use:staggerIn>
              <SettingsField label="What should this agent do?" hint="Muse designs the whole agent for you to review before anything is created.">
                <Textarea
                  autoGrow
                  rows={4}
                  bind:value={purpose}
                  placeholder="e.g. “A release manager that tracks our deploy trains, chases sign-offs before each cut, and posts a go/no-go summary.”"
                  class="max-h-48"
                  autofocus
                />
              </SettingsField>
              <!-- The other entry path, a peer of describing: pick a role, the fields
                   fill, the review step opens. Nothing is bound — everything stays
                   editable over there. Hidden while generating (the splash owns the
                   body), so its old `!generating` guard now lives in the branch. -->
              {#if roleTemplates.length}
                <SettingsField label="Or start from a role" for="cam-role-template">
                  <Select id="cam-role-template" size="sm" bind:value={roleSlug} onchange={() => roleSlug && applyRole(roleSlug)} class="w-full">
                    <option value="">Pick a role</option>
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
                </SettingsField>
              {/if}
            </div>
          {/if}
        {/key}
      </AutoHeight>
      {#if genErr || startErr}<p transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-danger" role="alert">{genErr ?? startErr}</p>{/if}
      {@render returnedNotes()}
      {#if leftForHire}
        <p transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-muted" data-testid="design-left-for-hire">
          {leftForHire} is being hired from another tab. The roster shows its progress.
        </p>
      {/if}
      {#if readErr}
        <p transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-danger" data-testid="design-read-error">
          Could not check for a design in progress: {readErr}
          <button type="button" class="ml-1 underline hover:text-fg" onclick={() => void designQuery.refetch()}>Retry</button>
        </p>
      {/if}
    </div>
    {#snippet footer()}
      <div class="flex items-center gap-2">
        {#if generating}
          {@render quietLink('Discard design', () => void discard(), true, 'design-discard')}
        {:else}
          {@render quietLink('Configure manually', () => (step = 'review'))}
        {/if}
        <span class="ml-auto"></span>
        <Button variant="ghost" size="sm" onclick={onClose}>
          {generating ? 'Close' : 'Cancel'}
        </Button>
        <Button size="sm" onclick={() => (generating ? undefined : void describe())} disabled={generating || !purpose.trim()}>
          {#if generating}<WaitingMark site="fleet/agent-create" size={12} />{/if}
          {generating ? 'Designing' : 'Design agent'}
        </Button>
      </div>
    {/snippet}
  </Modal>
{:else}
  <!-- ── Step 2: review + create ──────────────────────────────────────────── -->
  <Modal {open} {onClose} title="New agent" width="max-w-3xl">
    <div class="space-y-5">
      <div class="grid grid-cols-2 gap-x-4 gap-y-5">
        <SettingsField label="Name" for="cam-name">
          <Input id="cam-name" size="sm" value={form.name} oninput={(e) => onName(e.currentTarget.value)} placeholder="Research Analyst" autofocus={!generated} />
        </SettingsField>
        <SettingsField label="Handle" for="cam-handle">
          <Input id="cam-handle" size="sm" class="font-mono" value={form.handle} oninput={(e) => set({ handle: e.currentTarget.value })} placeholder="analyst" />
        </SettingsField>
        <SettingsField label="Role" for="cam-role" hint="The title shown on the roster.">
          <Input id="cam-role" size="sm" value={form.role} oninput={(e) => set({ role: e.currentTarget.value })} placeholder="Research Analyst" />
        </SettingsField>
        <SettingsField label="Department" for="cam-department" hint="Names its container and memory. Each agent has its own.">
          <Input id="cam-department" size="sm" class="font-mono" value={form.department} oninput={(e) => set({ department: e.currentTarget.value })} placeholder="research" aria-invalid={departmentHolder ? 'true' : undefined} />
          {#if departmentHolder}
            <p transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-danger" data-testid="department-taken">
              Taken by {departmentHolder.displayName}. Pick another.
            </p>
          {/if}
        </SettingsField>
      </div>
      <p class="font-sans text-[13px] leading-5 text-muted">
        Its id becomes <span class="font-mono text-xs text-fg">{form.handle || 'handle'}-{form.department || 'department'}</span>.
      </p>

      <SettingsField label="Copy settings from" for="cam-template" hint="Model tiers, tools and plugins carry over.">
        <Select id="cam-template" size="sm" bind:value={templateId} class="w-full">
          <option value="">Platform defaults</option>
          {#each templates as t (t.id)}
            <option value={t.id}>{t.displayName} · {t.department} (v{t.currentVersion})</option>
          {/each}
        </Select>
      </SettingsField>

      <div class="space-y-1.5">
        <div class="font-sans text-[13px] font-medium leading-5 text-fg">Instructions</div>
        {#if form.soul.trim()}
          <!-- Rich like the post-creation editor; autosave keeps `soul`
               fresh for create + refine, reseeded whenever muse redrafts. -->
          <div class="max-h-72 overflow-y-auto">
            {#key soulRev}
              <RichEditor value={form.soul} onSave={(md) => set({ soul: md })} autosave minHeight="9rem" />
            {/key}
          </div>
        {:else}
          <p class="font-sans text-[13px] leading-5 text-muted">It starts from a scaffold you can edit once it is created. Or go back and describe the agent to have them written.</p>
        {/if}
      </div>

      {#if form.skills.length > 0}
        <div in:fade={{ duration: 150 }} class="space-y-1.5">
          <div class="font-sans text-[13px] font-medium leading-5 text-fg">Starter skills</div>
          <ul class="overflow-hidden rounded-lg border border-line [&>*+*]:border-t [&>*+*]:border-line-subtle" use:listStagger>
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
        <p transition:slide={{ duration: 150 }} class="truncate font-sans text-[13px] leading-5 text-muted" data-refine-applied="visible">
          <span class="font-medium text-fg">Refined.</span>
          {#if lastChange.fields.length > 0}
            {lastChange.fields.map((f) => f.label).join(', ')}
            {#if !lastChange.soul.oversized}
              · {lastChange.soul.text}
            {/if}
          {:else if lastChange.soul.text !== 'no text changes'}
            {lastChange.soul.text}
          {:else}
            No changes to the design.
          {/if}
        </p>
      {/if}

      <Checkbox checked={start} onChange={(checked) => (start = checked)} label="Start it as soon as it is created" class="gap-2 text-sm text-fg" />
      {@render returnedNotes()}
      {#if hireErr}<div transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-danger" data-testid="design-hire-error">{hireErr}</div>{/if}
      {#if restored}<div transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-muted" data-testid="design-restored">Restored edits that had not reached the server when the page closed. They are being saved now.</div>{/if}
      {#if err || saveErr}<div transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-danger" role="alert">{err ?? `Your edits were not saved: ${saveErr}`}</div>{/if}
    </div>
    {#snippet footer()}
      <div class="flex items-center gap-4">
        {#if !preselect}
          {@render quietLink('Describe instead', () => (step = 'describe'))}
        {/if}
        {#if design && !preselect}
          {@render quietLink('Discard design', () => void discard(), true, 'design-discard')}
        {/if}
        <span class="ml-auto"></span>
        <Button variant="ghost" size="sm" onclick={onClose}>Cancel</Button>
        <Button size="sm" onclick={() => void create()} disabled={busy || generating || !form.handle || !form.department || !form.name || !!departmentHolder}>
          {busy ? 'Hiring' : 'Create agent'}
        </Button>
      </div>
    {/snippet}
  </Modal>
{/if}
