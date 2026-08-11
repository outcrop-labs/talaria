import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { getSetting, logAudit, setSetting } from '@/server/audit'
import { searchReachable, searchUrl, SEARCH_URL_KEY } from '@/server/search'

const Body = z.object({
  /** Empty means "use the environment / the bundled instance" — an admin
   *  clearing the field is a real instruction, not a validation failure. */
  url: z.string().max(300),
})

// LIVE WEB SEARCH — where it points, and whether it is actually answering.
//
// WHY THE REACHABILITY CHECK IS AN ADMIN SURFACE AND NOT A LOG LINE. When search
// is down, the way an operator currently finds out is that an agent tells
// somebody it could not look something up — a model's excuse is a terrible
// monitoring channel, and it arrives one conversation at a time. This makes the
// answer a thing you can ask for, and `web_search`'s own error text is what it
// reports, so the sentence an admin reads is the sentence the agent got.
//
// THE ENV VAR WINS, deliberately: a self-host that already runs SearXNG sets
// SEARXNG_URL and should not have that silently overridden by a setting somebody
// typed once. The GET says which is in force so the field is never a lie.
export const Route = defineApi('/api/admin/search', {
  GET: async ({ request }) => {
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
    const stored = await getSetting<string>(SEARCH_URL_KEY, '')
    const health = await searchReachable()
    return json({
      url: await searchUrl(),
      stored,
      /** True when the environment is deciding, so the UI can say the field is
       *  being ignored rather than showing an editable value that does nothing. */
      fromEnv: Boolean(process.env.SEARXNG_URL),
      reachable: health.ok,
      error: health.error,
    })
  },

  PUT: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body

    const url = body.url.trim().replace(/\/$/, '')
    if (url && !/^https?:\/\//.test(url)) return json({ error: 'the search URL must start with http:// or https://' }, { status: 400 })
    await setSetting(SEARCH_URL_KEY, url)
    void logAudit({ actor: actorOf(user), action: 'search.configure', targetType: 'setting', targetId: SEARCH_URL_KEY, after: { url } })

    // TESTED AFTER SAVING, not before: the point of the button is to tell an
    // admin whether what they just saved works, and a check against the old
    // value would answer a question nobody asked.
    const health = await searchReachable()
    return json({ url: await searchUrl(), reachable: health.ok, error: health.error })
  },
})
