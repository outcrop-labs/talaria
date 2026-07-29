import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getInstanceId } from '@/server/instance'

// Instance identity beacon — the target of hosting-domain verification's
// self-fetch. Public and harmless: a random UUID that proves which
// deployment answered, nothing more.
export const Route = createFileRoute('/api/well-known/talaria-instance')({
  server: {
    handlers: {
      GET: async () => json({ instance: await getInstanceId() }),
    },
  },
})
