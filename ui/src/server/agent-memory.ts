// Agent memory lives INSIDE each agent's state volume (/opt/data/MEMORY.md in
// the hermes-<dept> volume) — the agent curates it itself at runtime. Talaria
// reads and writes it through the running managed container (docker exec), so
// there's no second copy to drift. Requires the container to be up.
import { db } from './db/pg'
import { agentContainer, dockerExec } from './docker-exec'
import { snapshot } from './internal-history'

const MEMORY_PATH = '/opt/data/memories/MEMORY.md'

async function departmentFor(defId: string): Promise<{ department: string; displayName: string }> {
  const sql = await db()
  const rows = (await sql`
    select department, display_name as "displayName" from agent_defs where id = ${defId} and managed
  `) as unknown as Array<{ department: string; displayName: string }>
  if (!rows[0]) throw new Error('not a managed agent')
  return rows[0]
}

export async function readMemory(defId: string): Promise<{ content: string; container: string }> {
  const { department } = await departmentFor(defId)
  const name = await agentContainer(department)
  const { stdout } = await dockerExec(name, ['cat', MEMORY_PATH], { timeoutMs: 20_000 }).catch((e: Error) => {
    if (/no such file/i.test(e.message)) return { stdout: '', stderr: '' }
    throw new Error(`cannot read memory from ${name}: ${e.message}`)
  })
  return { content: stdout, container: name }
}

/** Whole-file replace. The agent also writes this file (with a lockfile) — a
 *  concurrent agent write can race a human edit; last writer wins. Every save
 *  is snapshotted so any prior memory is recoverable. */
export async function writeMemory(defId: string, content: string, author?: string | null): Promise<void> {
  const { department } = await departmentFor(defId)
  const name = await agentContainer(department)
  await dockerExec(name, ['sh', '-c', `cat > ${MEMORY_PATH}`], { timeoutMs: 20_000, input: content }).catch((e: Error) => {
    throw new Error(`cannot write memory in ${name}: ${e.message}`)
  })
  await snapshot('memory', defId, content, author ?? null).catch(() => {})
}
