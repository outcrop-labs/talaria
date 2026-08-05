<script lang="ts">
  import { navigate } from '@/router'

  // The fleet overview folded into Agents (its health is on Home + the Agents
  // header). Keep the path working for old links.
  $effect(() => {
    navigate('/agents', { replace: true })
  })
</script>
