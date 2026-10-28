<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Generating from '@/components/ui/Generating.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { submitOnEnter } from '@/components/ui/control'
  import InternalEditorModal from '@/components/fleet/InternalEditorModal.svelte'
  import TargetRow from '@/components/fleet/TargetRow.svelte'
  import { fade, listStagger, slide } from '@/lib/motion'
  import { pushToast } from '@/lib/toast.svelte'
  import { COMPACTION_BOUNDS, saveAgentEdit, type AgentDef, type LlmEndpoint, type ModelTarget } from '@/lib/fleet-defs'

  type AliasRow = ModelTarget & { name: string }

  // The agent config surface — soul, main model, alias tiers, fallback chain —
  // as an embeddable form. Saving creates a NEW version; "apply" re-renders,
  // and restarts the managed container only when the render moved it (a
  // model, a tier, a key Hermes reads at start — a soul edit restarts
  // nothing). The Config tab of the unified manage modal.
  let { def, endpoints, onSaved }: { def: AgentDef; endpoints: LlmEndpoint[]; onSaved?: () => void } = $props()

  const qc = useQueryClient()
  // Version drafts: the form seeds from the loaded version once, then Save
  // creates a NEW version — never re-seeding mid-edit is the contract, so a
  // new `def` object from the apply-refetch must not clobber in-progress work.
  // svelte-ignore state_referenced_locally -- reason: version drafts seeded once; saving creates a new version, refetch must not clobber in-progress edits
  const cfg = def.latest?.config
  // svelte-ignore state_referenced_locally -- reason: version drafts seeded once; saving creates a new version, refetch must not clobber in-progress edits
  let soul = $state(def.latest?.soul ?? '')
  // svelte-ignore state_referenced_locally -- reason: version drafts seeded once; saving creates a new version, refetch must not clobber in-progress edits
  let main = $state<ModelTarget>(cfg?.main ?? { endpoint: endpoints[0]?.name ?? '', model: '' })
  let aliases = $state<AliasRow[]>(cfg?.aliases ?? [])
  let fallbacks = $state<ModelTarget[]>(cfg?.fallbacks ?? [])
  // Compaction, seeded from the def's own projection of `raw.compression`
  // (a def-level field, not a member of `latest.config`). Blank IS a value
  // here — `null` clears the key and hands the decision back to Hermes — so
  // these are `number | null` rather than numbers with a sentinel.
  // svelte-ignore state_referenced_locally -- reason: version drafts seeded once; saving creates a new version, refetch must not clobber in-progress edits
  let threshold = $state<number | null>(def.compression?.threshold ?? null)
  // svelte-ignore state_referenced_locally -- reason: version drafts seeded once; saving creates a new version, refetch must not clobber in-progress edits
  let thresholdTokens = $state<number | null>(def.compression?.thresholdTokens ?? null)
  const touched = $state({ threshold: false, tokens: false })
  let note = $state('')
  let busy = $state<false | 'save' | 'apply'>(false)
  let soulOpen = $state(false)
  let err = $state<string | null>(null)

  const save = async (apply: boolean) => {
    err = null
    // A ratio outside the range is a typo, not an instruction: Hermes would
    // read 50 as "fifty times the context" and never compact. Caught here
    // rather than at the api so the form keeps the draft.
    const B = COMPACTION_BOUNDS
    if (threshold !== null && (!Number.isFinite(threshold) || threshold < B.threshold.min || threshold > B.threshold.max)) {
      err = `Compaction threshold is a ratio between ${B.threshold.min} and ${B.threshold.max}. Leave it blank for the default.`
      return
    }
    // The same window the route enforces — and whole tokens, as the route's
    // int reader demands — so a typo is caught with the draft still in the
    // form rather than as a 400 after the round trip.
    if (
      thresholdTokens !== null &&
      (!Number.isFinite(thresholdTokens) || !Number.isInteger(thresholdTokens) || thresholdTokens < B.tokens.min || thresholdTokens > B.tokens.max)
    ) {
      err = `Token ceiling is a whole number of tokens between ${B.tokens.min.toLocaleString()} and ${B.tokens.max.toLocaleString()}. Leave it blank for no ceiling.`
      return
    }
    busy = apply ? 'apply' : 'save'
    try {
      const r = await saveAgentEdit(def.id, {
        soul,
        main,
        aliases,
        fallbacks,
        // Only a knob the person TOUCHED travels: absent leaves the stored
        // value alone, which is what keeps an ordinary model-change save from
        // clearing a ceiling the wire could not project (a stored string or a
        // float the projection reads as null). A touched blank is a clear.
        compression: {
          ...(touched.threshold ? { threshold } : {}),
          ...(touched.tokens ? { thresholdTokens } : {}),
        },
        note: note || undefined,
        apply,
      })
      if (r.error) {
        err = r.error
        return
      }
      // A toast rather than a line in the form, because `onSaved` is the
      // manage modal's close: anything rendered in here is gone a frame later.
      const said = r.notes ?? []
      if (said.length > 0) pushToast({ title: `${def.displayName} · config saved`, body: said.join(' · ') })
      await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
      onSaved?.()
    } finally {
      busy = false
    }
  }
</script>

<div class="space-y-5">
  <section>
    <div class="mb-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Main model</div>
    <TargetRow {endpoints} value={main} onChange={(t) => (main = t)} allowEffort />
  </section>

  <section>
    <div class="mb-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Model tiers (aliases)</div>
    <div class="space-y-1.5" use:listStagger>
      <!-- Index-keyed rows: entries fade in; no exit — an outro would fade the
           LAST index while the remaining rows' contents shift, showing a
           duplicate. Instant removal is the honest form here. -->
      {#each aliases as a, i (i)}
        <div in:fade={{ duration: 150 }}>
          <TargetRow
            {endpoints}
            value={a}
            name={a.name}
            onName={(name) => (aliases = aliases.map((x, j) => (j === i ? { ...x, name } : x)))}
            namePlaceholder="alias"
            onChange={(t) => (aliases = aliases.map((x, j) => (j === i ? { ...x, ...t } : x)))}
            onRemove={() => (aliases = aliases.filter((_, j) => j !== i))}
            allowEffort
          />
        </div>
      {/each}
      <Button variant="outline" size="sm" onclick={() => (aliases = [...aliases, { name: '', endpoint: endpoints[0]?.name ?? '', model: '' }])}>
        + Add tier
      </Button>
    </div>
  </section>

  <section>
    <div class="mb-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Fallbacks (when the model above is down)</div>
    <div class="space-y-1.5" use:listStagger>
      {#each fallbacks as f, i (i)}
        <div in:fade={{ duration: 150 }}>
          <TargetRow
            {endpoints}
            value={f}
            onChange={(t) => (fallbacks = fallbacks.map((x, j) => (j === i ? t : x)))}
            onRemove={() => (fallbacks = fallbacks.filter((_, j) => j !== i))}
          />
        </div>
      {/each}
      <Button variant="outline" size="sm" onclick={() => (fallbacks = [...fallbacks, { endpoint: endpoints[0]?.name ?? '', model: '' }])}>
        + Add fallback
      </Button>
    </div>
  </section>

  <section>
    <div class="mb-1.5 flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
      Compaction
      <InfoTip
        text="Hermes folds the earlier part of a pinned session once the thread passes the threshold. The threshold is a ratio of the routed model's context window, and Hermes raises anything under 0.75 to 0.75 for models with less than 512K of context. The token ceiling is an absolute bound: whichever is reached first fires. Blank leaves Hermes' own default in place."
      />
    </div>
    <div class="flex flex-wrap items-center gap-x-4 gap-y-2">
      <label class="flex items-center gap-2">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-muted">Threshold</span>
        <Input
          size="sm"
          type="number"
          min={COMPACTION_BOUNDS.threshold.min}
          max={COMPACTION_BOUNDS.threshold.max}
          step={COMPACTION_BOUNDS.threshold.step}
          bind:value={threshold}
          oninput={() => (touched.threshold = true)}
          placeholder="default"
          title="Fraction of the model's context window at which the session is compacted (0.1 to 1); blank for Hermes' default"
          class="w-24 font-[var(--font-mono)]"
        />
      </label>
      <label class="flex items-center gap-2">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-muted">Token ceiling</span>
        <Input
          size="sm"
          type="number"
          min={COMPACTION_BOUNDS.tokens.min}
          max={COMPACTION_BOUNDS.tokens.max}
          step={COMPACTION_BOUNDS.tokens.step}
          bind:value={thresholdTokens}
          oninput={() => (touched.tokens = true)}
          placeholder="none"
          title="Compact once the session passes this many tokens, whatever the ratio says (1,000 to 10,000,000); blank for no ceiling"
          class="w-32 font-[var(--font-mono)]"
        />
      </label>
    </div>
  </section>

  <section>
    <div class="mb-1.5 flex items-center font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
      Soul
      <Button variant="outline" size="sm" class="ml-auto" onclick={() => (soulOpen = true)}>
        Open workspace
      </Button>
    </div>
    <!-- Documents render as ground-inset wells (spec §8 code treatment). -->
    <pre class="max-h-40 overflow-y-auto whitespace-pre-wrap rounded-md border border-line bg-surface p-3 font-mono text-xs text-muted">{soul || 'No soul yet.'}</pre>
    {#if soulOpen}
      <InternalEditorModal
        open
        nested
        onClose={() => (soulOpen = false)}
        title={`${def.displayName} · SOUL.md`}
        subtitle="Who the agent is. Saving publishes a soul-only version on top of the last saved config."
        value={soul}
        editable
        saving={!!busy}
        onSave={async (md) => {
          // Soul-only publish: model targets come from the LAST SAVED
          // version, not the form — pending model edits stay drafts.
          const base = def.latest?.config
          const r = await saveAgentEdit(def.id, {
            soul: md,
            main: base?.main ?? main,
            aliases: base?.aliases ?? aliases,
            fallbacks: base?.fallbacks ?? fallbacks,
            note: 'soul edited',
            apply: true,
          })
          if (r.error) throw new Error(r.error)
          soul = md
          await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
        }}
        history={{ kind: 'soul', id: def.id }}
        muse={{ kind: 'soul', context: `${def.displayName} — ${def.role ?? def.department} agent (slug "${def.slug}").` }}
      />
    {/if}
  </section>

  {#if err}<div transition:slide={{ duration: 150 }} class="text-sm text-danger">{err}</div>{/if}

  {#if busy === 'apply'}
    <Generating
      site="fleet/agent-apply"
      label={`Applying to ${def.displayName}: the change lands in place, and the agent restarts only if the edit needs it (offline for about a minute)`}
      lines={2}
    />
  {/if}
  <div class="flex items-center gap-2 border-t border-line pt-3">
    <Input bind:value={note} onkeydown={submitOnEnter(() => !busy && void save(false))} placeholder="version note (optional)" size="sm" class="min-w-0 flex-1" />
    <Button variant="outline" size="sm" onclick={() => void save(false)} disabled={!!busy}>
      Save version
    </Button>
    {#if def.managed}
      <Button size="sm" onclick={() => void save(true)} disabled={!!busy}>
        {busy === 'apply' ? 'Applying' : 'Save & apply'}
      </Button>
    {/if}
  </div>
</div>
