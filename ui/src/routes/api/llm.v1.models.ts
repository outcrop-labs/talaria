import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { authenticateKey } from '@/server/llm-keys'
import { gatewayModels } from '@/server/llm-gateway'

// OpenAI-compatible model list for the Talaria LLM gateway. External tools
// point at base_url http://<talaria>/api/llm/v1 with a minted tlk_ key.
export const Route = createFileRoute('/api/llm/v1/models')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const bearer = request.headers.get('authorization')?.replace(/^Bearer\s+/i, '') ?? null
        const id = await authenticateKey(bearer)
        if (!id) return json({ error: { message: 'invalid API key' } }, { status: 401 })
        const models = await gatewayModels()
        return json({
          object: 'list',
          data: models.map((m) => ({ id: m.id, object: 'model', owned_by: `talaria:${m.endpoints.join(',')}` })),
        })
      },
    },
  },
})
