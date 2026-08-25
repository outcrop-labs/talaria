<script lang="ts">
  import { navigate } from '@/router'

  // The fleet overview folded into Agents years before the Home Fleet tab came
  // and went; /agents owns it outright now. Keep the path working for old links.
  $effect(() => {
    navigate('/agents', { replace: true })
  })
</script>
