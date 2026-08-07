// THE BASE AGENT SURFACE — files and a test runner — for the fitness suite.
//
// WHY IT IS SEPARATE FROM `talaria-tools.ts`. That file is a copy of Talaria's
// own MCP toolkit, locked to `mcp/src/index.ts` by a sync test, and every tool
// on it is a workspace verb: tickets, channels, documents. This is the other
// half of what a coding agent holds — the CLI harness's own file tools, which
// belong to the harness (Claude Code, Codex, Aider …) rather than to Talaria,
// and which no file in this repository defines. There is nothing to lock a copy
// against, so this is a MODEL of that surface rather than a copy of one, and the
// distinction is stated here rather than left for a reader to infer.
//
// WHAT THAT MEANS FOR THE VERDICT IT PRODUCES. The three Workbench slots
// (`code-light`, `code-standard`, `code-heavy`) declare `requires: ['code',
// 'tools']`, and the role hint says why: "without tool calling the run does not
// degrade, it does nothing while reporting that it worked". That is the exact
// failure a prose eval cannot see and this can — so the question these tools
// exist to answer is narrow and worth answering:
//
//     given a repository, a failing test and file tools, does this model
//     LOCATE the defect, EDIT the right file, and CHECK its own work?
//
// It is NOT a claim that the model would drive Claude Code well end to end.
// Nobody can measure that from here, and the fixtures say what they measure.
//
// THE TEST RUNNER DOES NOT RUN CODE. Executing model-written code inside a
// benchmark is a sandbox-escape surface and a flake source, and Talaria has no
// business doing it in-process. `run_tests` instead applies each fixture's own
// `passes` predicate to the CURRENT file contents — a deterministic assertion
// the fixture author wrote, exactly like every other `EvalCase.check` in the
// tree. The model cannot tell the difference: it edits a file, runs the tests,
// and gets a pass or a named failure back.
import type { ToolDefinition } from '../../harness/transport'

/** A file in the sandbox workspace. */
export interface WorkspaceFile {
  path: string
  content: string
}

/** THE WORKSPACE AND ITS ORACLE.
 *
 *  `passes` is the fixture's own definition of "the tests are green", applied
 *  to the files as they stand. It is a predicate over the workspace rather than
 *  a string match on one file, because a real fix can be made in more than one
 *  place and a benchmark that demands one exact diff measures obedience rather
 *  than capability. */
export interface Workspace {
  files: WorkspaceFile[]
  /** Null when the suite passes; otherwise the failure the runner reports. */
  passes: (files: readonly WorkspaceFile[]) => string | null
}

const str = (description: string) => ({ type: 'string', description })

/** The base tools, as a coding harness offers them. Deliberately five: the
 *  fewest that make "find the defect, fix it, verify" expressible. A wider
 *  surface would measure tolerance for options rather than the job. */
export const HERMES_TOOLS: readonly ToolDefinition[] = [
  {
    name: 'list_files',
    description: 'List every file in the working tree, with its size in bytes. Start here — the tree is small and this is cheaper than guessing at paths.',
    parameters: { type: 'object', properties: {}, required: [] },
  },
  {
    name: 'read_file',
    description: 'Read a file in full. Read before you edit: write_file replaces the whole file, so an edit written from memory loses whatever you did not remember.',
    parameters: { type: 'object', properties: { path: str('Path from the working tree root') }, required: ['path'] },
  },
  {
    name: 'search',
    description: 'Search the working tree for a literal string and return every matching line with its path and line number. Use it to find where a symbol is defined or used.',
    parameters: { type: 'object', properties: { query: str('Literal text to look for') }, required: ['query'] },
  },
  {
    name: 'write_file',
    description: 'Replace a file’s ENTIRE contents. There is no patch mode: send the whole file as it should end up. Creating a new file is a write to a path that does not exist yet.',
    parameters: { type: 'object', properties: { path: str('Path from the working tree root'), content: str('The complete new contents of the file') }, required: ['path', 'content'] },
  },
  {
    name: 'run_tests',
    description: 'Run the project’s test suite against the files as they stand right now. Returns either that everything passed, or the first failure with the assertion that produced it.',
    parameters: { type: 'object', properties: {}, required: [] },
  },
] as const

/** One tool call against the workspace, in order. Same shape as the Talaria
 *  sandbox's log, so a fixture reads both the same way. */
export interface WorkbenchCall {
  tool: string
  args: Record<string, unknown>
  result: unknown
  error: string | null
}

export interface WorkbenchSandbox {
  files: WorkspaceFile[]
  calls: WorkbenchCall[]
  tools: ToolDefinition[]
  dispatch: (call: { name: string; args: string }) => Promise<{ text: string; isError: boolean }>
  callsTo: (tool: string) => WorkbenchCall[]
  calledBefore: (a: string, b: string) => boolean
  /** Are the tests green as things stand? The fixture's own oracle, so an
   *  assertion can ask "did it actually fix it" rather than "did it claim to". */
  green: () => boolean
  /** THE WORKSPACE AS THE MODEL LEFT IT, in the slot `EvalContext.world` reads.
   *  A file surface has no Talaria world, so this is what stands in its place:
   *  the files, and the oracle's verdict on them. */
  world: { files: readonly WorkspaceFile[]; failure: string | null }
}

class ToolRefusal extends Error {}

const clone = (files: readonly WorkspaceFile[]): WorkspaceFile[] => files.map((f) => ({ ...f }))

/** A fresh workspace. Nothing it touches outlives the returned object. */
export function makeWorkbench(workspace: Workspace): WorkbenchSandbox {
  const files = clone(workspace.files)
  const calls: WorkbenchCall[] = []
  const byPath = (path: string): WorkspaceFile | undefined => files.find((f) => f.path === path)

  const handlers: Record<string, (args: Record<string, unknown>) => unknown> = {
    list_files: () => ({ files: files.map((f) => ({ path: f.path, bytes: f.content.length })) }),

    read_file: (a) => {
      const path = String(a.path ?? '')
      const file = byPath(path)
      if (!file) {
        // The tree is small, so naming what IS there turns a wrong guess into
        // one recoverable turn instead of a dead end.
        throw new ToolRefusal(`no file at "${path}". The tree contains: ${files.map((f) => f.path).join(', ')}`)
      }
      return { path, content: file.content }
    },

    search: (a) => {
      const query = String(a.query ?? '')
      if (!query) throw new ToolRefusal('"query" is required')
      const hits: Array<{ path: string; line: number; text: string }> = []
      for (const f of files) {
        f.content.split('\n').forEach((text, i) => {
          if (text.includes(query)) hits.push({ path: f.path, line: i + 1, text: text.trim() })
        })
      }
      return { hits }
    },

    write_file: (a) => {
      const path = String(a.path ?? '')
      const content = a.content
      if (!path) throw new ToolRefusal('"path" is required')
      // A write with no content is the commonest malformed call here, and
      // silently creating an empty file would destroy the thing being fixed.
      if (typeof content !== 'string') throw new ToolRefusal('"content" must be the complete new contents of the file, as a string')
      const file = byPath(path)
      if (file) file.content = content
      else files.push({ path, content })
      return { ok: true, path, bytes: content.length }
    },

    run_tests: () => {
      const failure = workspace.passes(files)
      return failure === null ? { passed: true } : { passed: false, failure }
    },
  }

  const dispatch = async (call: { name: string; args: string }): Promise<{ text: string; isError: boolean }> => {
    let args: Record<string, unknown> = {}
    try {
      const parsed: unknown = JSON.parse(call.args)
      args = parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? (parsed as Record<string, unknown>) : {}
    } catch {
      const error = 'the arguments were not valid JSON'
      calls.push({ tool: call.name, args: {}, result: null, error })
      return { text: `Error: ${error}`, isError: true }
    }

    const handler = handlers[call.name]
    if (!handler) {
      const error = `there is no tool called "${call.name}"`
      calls.push({ tool: call.name, args, result: null, error })
      return { text: `Error: ${error}`, isError: true }
    }

    try {
      const result = handler(args)
      calls.push({ tool: call.name, args, result, error: null })
      return { text: JSON.stringify(result), isError: false }
    } catch (err) {
      // A refusal is the sandbox behaving like a real tool. Anything else is a
      // bug in this file and must not be dressed up as the model's fault.
      if (!(err instanceof ToolRefusal)) throw err
      calls.push({ tool: call.name, args, result: null, error: err.message })
      return { text: `Error: ${err.message}`, isError: true }
    }
  }

  return {
    files,
    calls,
    tools: HERMES_TOOLS.slice(),
    dispatch,
    callsTo: (tool) => calls.filter((c) => c.tool === tool),
    calledBefore: (a, b) => {
      const first = calls.findIndex((c) => c.tool === a)
      const second = calls.findIndex((c) => c.tool === b)
      return first !== -1 && second !== -1 && first < second
    },
    green: () => workspace.passes(files) === null,
    // A GETTER, not a snapshot: the sweep reads this after the loop has
    // finished, and a value captured at construction would report the state the
    // model started from.
    get world() {
      return { files, failure: workspace.passes(files) }
    },
  }
}
