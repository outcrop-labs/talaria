<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import { TAG_TONE, rankTags, tagLabel, tagTitle, visibleTags, type ModelRow } from './fitness'

  // Capability tags for one model, wherever a model is PICKED — the roles
  // panel, the platform agents panel, member access.
  //
  // THREE STATES, NOT TWO. A tag says known-true, known-false, or never
  // measured, because those are three different facts and `missingCapabilities`
  // only ever treats the middle one as a lack. Rendering unknown as a "no"
  // would paint a fresh self-host — where nothing has been probed — as a
  // catalogue of broken models.
  //
  // A model with NOTHING measured collapses to one neutral "Untested" chip
  // rather than nine grey ones: nine chips that all say "we don't know" is
  // noise wearing the costume of information.
  let {
    row,
    /** Show measured-false tags only. For dense rows where the useful signal is
     *  "what will break", not the full record. */
    negativeOnly = false,
    /** Show this many tags, what the model lacks first, and fold the rest into
     *  one "+N" chip whose title names them. */
    max,
    class: className = '',
  }: { row: ModelRow | undefined; negativeOnly?: boolean; max?: number; class?: string } = $props()

  const tags = $derived(visibleTags(row))
  // `negativeOnly` is "what will break here", so a capability the DEPLOYMENT
  // supplies is not in it — that is the opposite of broken. It is exactly the
  // case a dense row would otherwise print as "No vision" on an install where
  // vision demonstrably works.
  const ranked = $derived(rankTags(negativeOnly ? tags.measured.filter((c) => c.state === 'no') : tags.measured))
  const shown = $derived(max === undefined ? ranked : ranked.slice(0, max))
  const folded = $derived(max === undefined ? [] : ranked.slice(max))
</script>

{#if row && (row.pooled || !tags.anyMeasured || shown.length > 0)}
  <span class="inline-flex flex-wrap items-center gap-1 {className}">
    {#if row.pooled}
      <!-- A bare id served by several endpoints. Capability is a property of
           the endpoint, so its facts can genuinely differ per member and a
           probe run refuses to write under a pooled key at all. -->
      <Chip title="This id is served by {row.endpoints.length} endpoints ({row.endpoints.join(', ')}), and capability is a property of the endpoint. Test the endpoint-qualified ids to record facts.">
        Pooled
      </Chip>
    {/if}
    {#if !tags.anyMeasured}
      <Chip title="Nothing has measured this model's capabilities. Unknown is not a no: Talaria will still run it, and the fitness probes are what turn this into facts.">
        Untested
      </Chip>
    {:else}
      {#each shown as c (c.cap)}
        <Chip tone={TAG_TONE[c.state]} title={tagTitle(c)}>{tagLabel(c)}</Chip>
      {/each}
      {#if folded.length > 0}
        <Chip title={folded.map(tagLabel).join(', ')}>+{folded.length}</Chip>
      {/if}
    {/if}
  </span>
{/if}
