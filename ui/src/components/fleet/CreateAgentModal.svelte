<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Sparkles } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Generating from '@/components/ui/Generating.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import RichEditor from '@/components/ui/RichEditor.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import { createFleetAgent, type AgentDef } from '@/lib/fleet-defs'
  import { listRoleTemplates, type RoleTemplate } from '@/lib/agent-role-templates'
  import { fade, listStagger, slide } from '@/lib/motion'
  import { draftAgent, type AgentDraft } from '@/lib/muse.svelte'
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
  //     fresh install, where there is no existing agent to copy.
  //   • CHASSIS template — what it RUNS ON. Clones an existing agent's model
  //     tiers, tools and plugins with the identity re-stamped. Unavailable
  //     until at least one agent exists, which is exactly why it could never
  //     be the only answer.
  // Talaria allocates the key, writes v1, renders, starts.
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
  let step = $state<'describe' | 'review'>(preselect ? 'review' : 'describe')

  // Describe → generate. There is no token stream to show any more: an agent
  // design is a JSON contract, so the server parses and validates it (and gets
  // a repair turn when a small model fumbles the shape) and answers with the
  // finished draft. What used to be a live preview of raw JSON scrolling past
  // is a progress label — the thing worth watching was never the braces.
  let purpose = $state('')
  let generating = $state(false)
  let chat = $state<Array<{ role: 'user' | 'assistant'; content: string }>>([])
  let genErr = $state<string | null>(null)

  // Review fields (filled by generation or by hand)
  let displayName = $state('')
  let slug = $state('')
  let department = $state('')
  let role = $state('')
  let soul = $state('')
  let soulRev = $state(0)
  let skills = $state<AgentDraft['skills']>([])
  let templateId = $state(preselect ?? templates[0]?.id ?? '') // '' = platform defaults
  // Role templates: fetched once the dialog is open. Choosing one FILLS the
  // fields rather than binding to them — every value stays editable, and a
  // template is a starting point, not a mode you are stuck in.
  let roleTemplates = $state<RoleTemplate[]>([])
  let roleSlug = $state('')
  $effect(() => {
    if (!open) return
    void listRoleTemplates()
      .then((t) => (roleTemplates = t))
      .catch(() => (roleTemplates = []))
  })
  const applyRole = (slugPicked: string) => {
    roleSlug = slugPicked
    const t = roleTemplates.find((x) => x.slug === slugPicked)
    if (!t) return
    displayName = t.name
    slug = t.slug.replace(/-/g, '')
    department = t.department
    role = t.role
    soul = t.soul
    soulRev += 1
  }
  let start = $state(true)
  let busy = $state(false)
  let err = $state<string | null>(null)

  const applyDraft = (d: AgentDraft) => {
    displayName = d.name
    slug = d.handle
    department = d.department
    role = d.role
    soul = d.soul
    soulRev += 1 // reseed the editor with the new draft
    skills = d.skills
  }

  const currentDraftJson = () =>
    JSON.stringify({ name: displayName, handle: slug, department, role, soul, skills })

  const generate = async (instruction: string, refining: boolean) => {
    if (!instruction.trim()) return
    generating = true
    genErr = null
    try {
      const draft = await draftAgent({
        instruction: instruction.trim(),
        ...(refining ? { current: currentDraftJson() } : {}),
        chat,
      })
      // The turn the model sees on the next refine is the VALIDATED draft, not
      // whatever text it happened to emit — so a reply that needed a repair turn
      // does not teach the model its own broken shape on the way round again.
      chat = [...chat.slice(-8), { role: 'user', content: instruction.trim() }, { role: 'assistant', content: JSON.stringify(draft) }]
      applyDraft(draft)
      step = 'review'
    } catch (e) {
      genErr = (e as Error).message
    } finally {
      generating = false
    }
  }

  const onName = (v: string) => {
    const prev = displayName
    displayName = v
    if (!slug || slug === prev.toLowerCase().replace(/[^a-z0-9]/g, '')) {
      slug = v.toLowerCase().replace(/[^a-z0-9]/g, '')
    }
  }

  const create = async () => {
    err = null
    busy = true
    try {
      const r = await createFleetAgent({
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
      if (start && r.healthy === false) err = 'created, but the container is not healthy yet. Check /agents'
      await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
      await qc.invalidateQueries({ queryKey: ['fleet-containers'] })
      if (!r.error) onClose()
    } finally {
      busy = false
    }
  }

  const generated = $derived(chat.length > 0)
</script>

{#if step === 'describe'}
  <!-- ── Step 1: describe ─────────────────────────────────────────────────── -->
  <Modal {open} {onClose} title="New agent" width="max-w-lg">
    <div class="space-y-5">
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
      {#if generating}
        <Generating site="fleet/agent-design" label="Designing the agent: identity, soul, and starter skills" lines={3} />
      {/if}
      {#if genErr}<p transition:slide={{ duration: 150 }} class="text-xs text-danger">{genErr}</p>{/if}
      <div class="flex items-center gap-3 border-t border-line pt-4">
        <button type="button" class="text-xs text-muted hover:text-fg" onclick={() => (step = 'review')}>
          Configure manually →
        </button>
        <span class="ml-auto"></span>
        <Button variant="ghost" size="sm" onclick={onClose}>
          Cancel
        </Button>
        <Button onclick={() => void generate(purpose, false)} disabled={generating || !purpose.trim()}>
          {#if generating}<WaitingMark site="fleet/agent-create" size={12} />{/if}
          {generating ? 'Designing' : 'Design agent'}
        </Button>
      </div>
    </div>
  </Modal>
{:else}
  <!-- ── Step 2: review + create ──────────────────────────────────────────── -->
  <Modal {open} {onClose} title="New agent" takeover>
    <div class="space-y-5">
      <!-- Start from a role. Fills the fields below and leaves every one of
           them editable — a template is a starting point, not a mode. -->
      {#if roleTemplates.length}
        <div>
          <label for="role-template" class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
            Start from a role
          </label>
          <Select id="role-template" bind:value={roleSlug} onchange={() => applyRole(roleSlug)} class="w-full">
            <option value="">Blank — fill it in myself</option>
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
          {#if roleSlug}
            <p class="mt-1.5 font-sans text-xs text-muted">
              {roleTemplates.find((t) => t.slug === roleSlug)?.description}
            </p>
          {/if}
        </div>
      {/if}
      <div class="grid grid-cols-2 gap-4">
        <div>
          <label class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Name</label>
          <Input value={displayName} oninput={(e) => onName(e.currentTarget.value)} placeholder="Research Analyst" autofocus={!generated} />
        </div>
        <div>
          <label class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Handle</label>
          <Input bind:value={slug} placeholder="analyst" />
        </div>
      </div>
      <div class="grid grid-cols-2 gap-4">
        <div>
          <label class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Role</label>
          <Input bind:value={role} placeholder="Research Analyst" />
        </div>
        <div>
          <label class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Department</label>
          <Input bind:value={department} placeholder="research" />
        </div>
      </div>
      <p class="-mt-2 font-sans text-xs text-muted">
        Role is the roster title; department is the routing/mount key. The fleet model id becomes
        <span class="font-mono text-fg">{slug || 'handle'}-{department || 'department'}</span>.
      </p>

      <div>
        <label class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
          Chassis template <span class="normal-case">(model tiers, tools, and plugins carry over)</span>
        </label>
        <Select bind:value={templateId} class="w-full">
          <option value="">Platform defaults: chassis + first local model</option>
          {#each templates as t (t.id)}
            <option value={t.id}>{t.displayName} · {t.department} (v{t.currentVersion})</option>
          {/each}
        </Select>
      </div>

      <div>
        <label class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Soul</label>
        {#if soul.trim()}
          <!-- Rich like the post-creation soul editor; autosave keeps `soul`
               fresh for create + refine, reseeded whenever muse redrafts. -->
          <div class="max-h-72 overflow-y-auto">
            {#key soulRev}
              <RichEditor value={soul} onSave={(md) => (soul = md)} autosave minHeight="9rem" />
            {/key}
          </div>
        {:else}
          <p class="text-xs text-muted">Starts from a scaffold you edit after creation, or go back and describe the agent to have one designed.</p>
        {/if}
      </div>

      {#if skills.length > 0}
        <div in:fade={{ duration: 150 }}>
          <label class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Starter skills</label>
          <ul class="divide-y divide-line rounded-lg border border-line" use:listStagger>
            {#each skills as s (s.name)}
              <SkillPreviewRow skill={s} onRemove={() => (skills = skills.filter((x) => x.name !== s.name))} />
            {/each}
          </ul>
        </div>
      {/if}

      {#if generated}
        <RefineBar
          busy={generating}
          preview={null}
          error={genErr}
          onRefine={(text) => void generate(text, true)}
        />
      {/if}

      <label class="flex cursor-pointer items-center gap-2 text-sm text-fg">
        <input type="checkbox" bind:checked={start} class="accent-accent" />
        Start the container now
      </label>
      {#if busy}
        <Generating
          site="fleet/agent-hire"
          label={start
            ? `Hiring ${displayName || slug}: rendering the config, starting the container, waiting for health`
            : `Creating ${displayName || slug}`}
          lines={3}
        />
      {/if}
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
          {busy ? 'Creating' : 'Create agent'}
        </Button>
      </div>
    </div>
  </Modal>
{/if}
