<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import { CAPABILITY_WORDS, TAG_TONE, tagTitle, visibleTags, type ModelRow } from './fitness'

  // Capability tags for one model, wherever a model is PICKED — the roles
  // panel, the platform agents panel, member access.
  //
  // THREE STATES, NOT TWO. A tag says known-true, known-false, or never
  // measured, because those are three different facts and `missingCapabilities`
  // only ever treats the middle one as a lack. Rendering unknown as a red "no"
  // would paint a fresh self-host — where nothing has been probed — as a
  // catalogue of broken models.
  //
  // A model with NOTHING measured collapses to one neutral "untested" chip
  // rather than nine grey ones: nine chips that all say "we don't know" is
  // noise wearing the costume of information.
  let {
    row,
    /** Show measured-false tags only. For dense rows where the useful signal is
     *  "what will break", not the full record. */
    negativeOnly = false,
    class: className = '',
  }: { row: ModelRow | undefined; negativeOnly?: boolean; class?: string } = $props()

  const tags = $derived(visibleTags(row))
  const shown = $derived(negativeOnly ? tags.measured.filter((c) => c.state === 'no') : tags.measured)
</script>

{#if row}
  <span class="inline-flex flex-wrap items-center gap-1 {className}">
    {#if row.pooled}
      <!-- A bare id served by several endpoints. Capability is a property of
           the endpoint, so its facts can genuinely differ per member and a
           probe run refuses to write under a pooled key at all. -->
      <Chip title="This id is served by {row.endpoints.length} endpoints ({row.endpoints.join(', ')}), and capability is a property of the endpoint. Test the endpoint-qualified ids to record facts.">
        pooled
      </Chip>
    {/if}
    {#if !tags.anyMeasured}
      <Chip title="Nothing has measured this model's capabilities. Unknown is not a no — Talaria will still run it, and the fitness probes are what turn this into facts.">
        untested
      </Chip>
    {:else}
      {#each shown as c (c.cap)}
        <Chip tone={TAG_TONE[c.state]} title={tagTitle(c)}>
          {c.state === 'no' ? 'no ' : ''}{CAPABILITY_WORDS[c.cap].short}
        </Chip>
      {/each}
    {/if}
  </span>
{/if}
