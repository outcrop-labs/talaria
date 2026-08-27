<script lang="ts">
  // One proposal, room to breathe: title + meta on top, full-width description,
  // and a "blocked by" row referencing sibling proposals (created as real
  // ticket dependencies).
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Select from '@/components/ui/Select.svelte'
  import RichEditor from '@/components/ui/RichEditor.svelte'
  import type { Proposal } from './plan-modal'
  import type { Effort, Priority } from '@/lib/task-const'

  let {
    index,
    proposal: p,
    all,
    onPatch,
  }: {
    index: number
    proposal: Proposal
    all: Proposal[]
    onPatch: (patch: Partial<Proposal>) => void
  } = $props()

  const short = (s: string, n = 36) => (s.length > n ? `${s.slice(0, n)}…` : s)
  const addable = $derived(
    all.map((x, j) => ({ x, j })).filter(({ j }) => j !== index && !p.dependsOn.includes(j)),
  )

  const priorities: Priority[] = ['low', 'medium', 'high', 'urgent']
  const efforts: Effort[] = ['xs', 's', 'm', 'l', 'xl']
</script>

<div class={`rounded-lg border border-line bg-panel p-4 ${p.include ? '' : 'opacity-50'}`}>
  <div class="flex items-center gap-2">
    <Checkbox
      bare
      title="Include this proposal"
      checked={p.include}
      onChange={(checked) => onPatch({ include: checked })}
      class="shrink-0"
    />
    <span class="w-7 shrink-0 text-right text-xs text-muted">#{index + 1}</span>
    <Input size="sm" value={p.title} oninput={(e) => onPatch({ title: e.currentTarget.value })} class="flex-1" />
    <Select value={p.priority} size="sm" onchange={(e) => onPatch({ priority: e.currentTarget.value as Priority })} class="w-24 shrink-0">
      {#each priorities as x (x)}
        <option value={x}>
          {x}
        </option>
      {/each}
    </Select>
    <Select
      value={p.effort ?? ''}
      size="sm"
      onchange={(e) => onPatch({ effort: (e.currentTarget.value || null) as Effort | null })}
      class="w-20 shrink-0"
    >
      <option value="">—</option>
      {#each efforts as x (x)}
        <option value={x}>
          {x.toUpperCase()}
        </option>
      {/each}
    </Select>
  </div>
  {#if p.include}
    <!-- Draft descriptions are ticket markdown — edit them the way the
         ticket will show them. Seeded per draft; autosave patches up. -->
    <div class="mt-3 max-h-64 overflow-y-auto">
      <RichEditor value={p.description} onSave={(md) => onPatch({ description: md })} autosave minHeight="8rem" />
    </div>
    <div class="mt-2 flex flex-wrap items-center gap-1.5">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Blocked by</span>
      {#each p.dependsOn as d (d)}
        <span class="inline-flex items-center gap-1 rounded-md border border-line bg-raised px-2 py-0.5 text-xs text-fg">
          #{d + 1} {short(all[d]?.title ?? '')}
          <button
            type="button"
            class="text-muted hover:text-fg"
            onclick={() => onPatch({ dependsOn: p.dependsOn.filter((x) => x !== d) })}
            aria-label={`remove dependency on #${d + 1}`}
          >
            ×
          </button>
        </span>
      {/each}
      {#if addable.length > 0}
        <Select
          value=""
          size="sm"
          onchange={(e) => {
            const d = Number(e.currentTarget.value)
            if (Number.isInteger(d)) onPatch({ dependsOn: [...p.dependsOn, d] })
            // The React version was controlled back to ""; reset by hand here.
            e.currentTarget.value = ''
          }}
          class="w-40"
        >
          <option value="">+ add</option>
          {#each addable as { x, j } (j)}
            <option value={j}>
              #{j + 1} {short(x.title, 28)}
            </option>
          {/each}
        </Select>
      {/if}
      {#if p.dependsOn.length === 0 && addable.length === 0}<span class="text-xs text-muted">—</span>{/if}
    </div>
  {/if}
</div>
