// Agent memory lives INSIDE each agent's state volume (/opt/data/MEMORY.md in
// the hermes-<dept> volume) — the agent curates it itself at runtime. Talaria
// reads and writes it through the running managed container (docker exec), so
// there's no second copy to drift. Requires the container to be up.
import { execFile } from 'node:child_process'
import { db } from './db/pg'

const MEMORY_PATH = '/opt/data/memories/MEMORY.md'

function exec(args: string[], input?: string): Promise<{ stdout: string; stderr: string }> {
  return new Promise((res, rej) => {
    const child = execFile('docker', args, { timeout: 20_000, maxBuffer: 4 * 1024 * 1024 }, (err, stdout, stderr) =>
      err ? rej(new Error(stderr.trim() || err.message)) : res({ stdout, stderr }),
    )
    if (input !== undefined) {
      child.stdin?.write(input)
      child.stdin?.end()
    }
  })
}

async function departmentFor(defId: string): Promise<{ department: string; displayName: string }> {
  const sql = await db()
  const rows = (await sql`
    select department, display_name as "displayName" from agent_defs where id = ${defId} and managed
  `) as unknown as Array<{ department: string; displayName: string }>
  if (!rows[0]) throw new Error('not a managed agent')
  return rows[0]
}

const container = (department: string) => `talaria-fleet-agent-${department}-1`

export async function readMemory(defId: string): Promise<{ content: string; container: string }> {
  const { department } = await departmentFor(defId)
  const name = container(department)
  const { stdout } = await exec(['exec', name, 'cat', MEMORY_PATH]).catch((e: Error) => {
    if (/no such file/i.test(e.message)) return { stdout: '', stderr: '' }
    throw new Error(`cannot read memory from ${name}: ${e.message}`)
  })
  return { content: stdout, container: name }
}

/** Whole-file replace. The agent also writes this file (with a lockfile) — a
 *  concurrent agent write can race a human edit; last writer wins. */
export async function writeMemory(defId: string, content: string): Promise<void> {
  const { department } = await departmentFor(defId)
  const name = container(department)
  await exec(['exec', '-i', name, 'sh', '-c', `cat > ${MEMORY_PATH}`], content).catch((e: Error) => {
    throw new Error(`cannot write memory in ${name}: ${e.message}`)
  })
}
