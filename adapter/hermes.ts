import { eventBus } from '@/lib/event-bus'
import { queryPendingAssignments } from './adapter'
import type { FrameworkAdapter, AgentRegistration, HeartbeatPayload, TaskReport, Assignment } from './adapter'

/**
 * Hermes framework adapter.
 *
 * Hermes agents (github.com/NousResearch/hermes-agent) connect to Mission Control
 * through Talaria (github.com/outcrop-labs/talaria): a Hermes plugin registers each
 * agent, heartbeat-polls `/api/agents/{id}/heartbeat` for assigned work, and reports
 * progress via `PUT /api/tasks/{id}`. A companion bridge lets the hermes-workspace UI
 * drive its Conductor missions here as tasks. This adapter mirrors the other framework
 * adapters: lifecycle events fan out on the eventBus, assignments come from the shared
 * task queue. (Note: Talaria never forces the Aegis-gated `done` transition — completion
 * flows through Mission Control's own approval.)
 */
export class HermesAdapter implements FrameworkAdapter {
  readonly framework = 'hermes'

  async register(agent: AgentRegistration): Promise<void> {
    eventBus.broadcast('agent.created', {
      id: agent.agentId,
      name: agent.name,
      framework: this.framework,
      status: 'online',
      ...(agent.metadata ?? {}),
    })
  }

  async heartbeat(payload: HeartbeatPayload): Promise<void> {
    eventBus.broadcast('agent.status_changed', {
      id: payload.agentId,
      status: payload.status,
      metrics: payload.metrics ?? {},
      framework: this.framework,
    })
  }

  async reportTask(report: TaskReport): Promise<void> {
    eventBus.broadcast('task.updated', {
      id: report.taskId,
      agentId: report.agentId,
      progress: report.progress,
      status: report.status,
      output: report.output,
      framework: this.framework,
    })
  }

  async getAssignments(agentId: string): Promise<Assignment[]> {
    return queryPendingAssignments(agentId)
  }

  async disconnect(agentId: string): Promise<void> {
    eventBus.broadcast('agent.status_changed', {
      id: agentId,
      status: 'offline',
      framework: this.framework,
    })
  }
}
