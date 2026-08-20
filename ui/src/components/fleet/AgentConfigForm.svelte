<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Generating from '@/components/ui/Generating.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { submitOnEnter } from '@/components/ui/control'
  import InternalEditorModal from '@/components/fleet/InternalEditorModal.svelte'
  import TargetRow from '@/components/fleet/TargetRow.svelte'
  import { fade, listStagger, slide } from '@/lib/motion'
  import { saveAgentEdit, type AgentDef, type LlmEndpoint, type ModelTarget } from '@/lib/fleet-defs'

  type AliasRow = ModelTarget & { name: string }

  // The agent config surface — soul, main model, alias tiers, fallback chain —
  // as an embeddable form. Saving creates a NEW version; "apply" re-renders and
  // restarts the managed container. The Config tab of the unified manage modal.
  let { def, endpoints, onSaved }: { def: AgentDef; endpoints: LlmEndpoint[]; onSaved?: () => void } = $props()

  const qc = useQueryClient()
  const cfg = def.latest?.config
  let soul = $state(def.latest?.soul ?? '')
  let main = $state<ModelTarget>(cfg?.main ?? { endpoint: endpoints[0]?.name ?? '', model: '' })
  let aliases = $state<AliasRow[]>(cfg?.aliases ?? [])
  let fallbacks = $state<ModelTarget[]>(cfg?.fallbacks ?? [])
  let note = $state('')
  let busy = $state<false | 'save' | 'apply'>(false)
  let soulOpen = $state(false)
  let err = $state<string | null>(null)

  const save = async (apply: boolean) => {
    err = null
    busy = apply ? 'apply' : 'save'
    try {
      const r = await saveAgentEdit(def.id, { soul, main, aliases, fallbacks, note: note || undefined, apply })
      if (r.error) {
        err = r.error
        return
      }
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
      label={`Applying to ${def.displayName}: new container rolling up beside the old one, health check, then traffic cuts over`}
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
