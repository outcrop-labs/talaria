<script lang="ts">
  import { navigate } from '@/router'

  // The inbox IS the home surface now (notifications + the day's dashboard).
  $effect(() => {
    void navigate('/', { replace: true })
  })
</script>
