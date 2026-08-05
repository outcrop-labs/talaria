import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { getInstanceId } from '@/server/instance'

// Instance identity beacon — the target of hosting-domain verification's
// self-fetch. Public and harmless: a random UUID that proves which
// deployment answered, nothing more.
export const Route = defineApi('/api/well-known/talaria-instance', {
  GET: async () => json({ instance: await getInstanceId() }),
})
