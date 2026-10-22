<script lang="ts">
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatCard from '@/components/ui/StatCard.svelte'
  import { useSkills } from '@/lib/skills'
  import { useGaps } from '@/lib/workflows'
  import { p } from '@/router'

  // Agent Studio: the skills the fleet has, and the open suggestions — the
  // capability gaps agents have reported and nobody has acted on yet.
  const skillsQuery = useSkills()
  const gapsQuery = useGaps('open')
  const skills = $derived(skillsQuery.data?.reduce((n, o) => n + o.skills.length, 0))
  const gaps = $derived(gapsQuery.data?.length)
</script>

<StatCard
  label="Skills"
  href={p('/studio')}
  sub={gaps !== undefined ? (gaps > 0 ? `${gaps} open suggestion${gaps === 1 ? '' : 's'}` : 'no open suggestions') : gapsQuery.isError ? 'suggestions unavailable' : undefined}
>
  {#snippet value()}
    {#if skills !== undefined}{skills}
    {:else if skillsQuery.isError}—
    {:else}<Skeleton class="h-5 w-16 rounded-full" />{/if}
  {/snippet}
</StatCard>
