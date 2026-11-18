<script lang="ts">
  import { useSkills } from '@/lib/skills'
  import { useGaps } from '@/lib/workflows'
  import { p } from '@/router'
  import InventoryItem from './InventoryItem.svelte'

  // Agent Studio: the skills the fleet has, and the open suggestions, which
  // are the capability gaps agents reported and nobody has acted on yet.
  const skillsQuery = useSkills()
  const gapsQuery = useGaps('open')
  const skills = $derived(skillsQuery.data?.reduce((n, o) => n + o.skills.length, 0))
  const gaps = $derived(gapsQuery.data?.length)
</script>

<InventoryItem
  href={p('/studio')}
  count={skills}
  noun="skill"
  note={gaps ? `${gaps} open suggestion${gaps === 1 ? '' : 's'}` : gapsQuery.isError ? 'suggestions unavailable' : undefined}
  failed={skillsQuery.isError}
/>
