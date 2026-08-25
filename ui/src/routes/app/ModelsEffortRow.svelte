<script lang="ts">
  import Input from '@/components/ui/Input.svelte'
  import { submitOnEnter } from '@/components/ui/control'

  // One model's effort-ladder row in the endpoint modal.
  //
  // TWO VOICES, both shown: what the PROVIDER's catalog publishes (the live
  // answer the modal already fetched for the model adder), and what the ADMIN
  // declares. This row exists for the gap between them — an endpoint that
  // answers /models with `{id}` and nothing else publishes no ladder at all,
  // no matter what the weights behind it accept, which is how a self-host its
  // own operator knows takes effort levels showed no picker anywhere. The
  // declaration replaces the catalog's ladder for this endpoint's build (it
  // never merges — a union would offer a level neither voice vouched for).
  //
  // The input is FREE TEXT on purpose: levels are the provider's own
  // spellings, sent verbatim, and a fixed list would rename a level into one
  // the model rejects. Comma or space separated; cleared = undeclare.
  let {
    model,
    catalogEfforts,
    declared,
    onDeclare,
  }: {
    /** Upstream model id on this endpoint. */
    model: string
    /** What the provider's catalog publishes right now (null = silent). */
    catalogEfforts: string[] | null
    /** The admin's stored declaration for this model, if any. */
    declared: string[] | undefined
    /** levels = declare/replace; null = undeclare (back to the catalog). */
    onDeclare: (levels: string[] | null) => void
  } = $props()

  // Uncontrolled, like the pricing inputs beside this section: seeded from
  // the stored declaration, committed on blur/Enter.
  let value = $state(declared?.join(', ') ?? '')

  const parse = (raw: string): string[] | null => {
    const levels = [...new Set(raw.split(/[\s,]+/).map((l) => l.trim().toLowerCase()).filter(Boolean))]
    return levels.length > 0 ? levels : null
  }
  const commit = () => {
    const levels = parse(value)
    const same =
      (levels === null && declared === undefined) ||
      (levels !== null && declared !== undefined && levels.join() === declared.join())
    if (same) return
    onDeclare(levels)
  }
</script>

<div class="flex items-center gap-2 py-1.5 font-mono text-xs">
  <span class="min-w-0 flex-1 truncate text-fg">{model}</span>
  <!-- The provider's word, or its silence — the reason this row exists. -->
  {#if declared}
    <span class="shrink-0 tracking-[0.05em] text-accent" title="Your declaration replaces the catalog's ladder for this endpoint's build of the model">declared</span>
  {:else if catalogEfforts}
    <span class="shrink-0 text-muted" title="What the provider's catalog publishes right now">catalog: {catalogEfforts.join(' · ')}</span>
  {:else}
    <span class="shrink-0 text-muted" title="This provider publishes no ladder for the model. Declare one if the model takes one">catalog silent</span>
  {/if}
  <Input
    size="sm"
    class="w-56 shrink-0"
    bind:value
    onkeydown={submitOnEnter(commit)}
    onblur={commit}
    placeholder={declared ? 'clear to undeclare' : 'e.g. low, medium, high'}
  />
</div>
