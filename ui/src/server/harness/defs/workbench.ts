// THE CODING HARNESSES — three of them, one per Workbench effort slot.
//
// WHY THEY EXIST. `MODEL_ROLES` has carried `code-light`, `code-standard` and
// `code-heavy` since the Workbench shipped, and the fitness matrix printed all
// three as "No harness in this install is bound to Workbench · X effort, so a
// sweep can say nothing about a model for it." That sentence was accurate and
// useless: an admin assigning a coder had no evidence to assign on, from a page
// whose entire job is to give them some.
//
// WHAT THEY MEASURE, and it is narrower than "is this a good coding model".
// The role's own declaration says what matters — `requires: ['code', 'tools']`,
// with the hint spelling out why: "without tool calling the run does not
// degrade, it does nothing while reporting that it worked". So the question is:
//
//     given a repository, a failing test and file tools, does this model
//     LOCATE the defect, EDIT the right file, and CHECK its own work?
//
// A model that answers the bug in prose has failed. A model that writes a
// plausible patch to the wrong file has failed. A model that fixes it and never
// runs the tests has done the job and cannot know it. Those three are the
// failures a Workbench run actually hits, and none of them is visible from a
// reply's text.
//
// WHY THREE HARNESSES AND NOT ONE WITH A PARAMETER. A harness has ONE
// `ModelSpec`, and `score.ts` binds a harness to a slot by running the REAL
// resolver over it (`rolesReaching`). One harness declaring `role:
// 'code-standard'` binds one column and leaves the other two exactly as empty
// as they were. Three definitions over one shared builder is what actually
// fills the row — and it is honest besides: the three slots are three different
// purchases, and an admin comparing them wants the same task run against each.
//
// WHAT THIS IS A PROXY FOR, AND WHAT IT IS NOT A REPLAY OF. Read this before
// trusting a `Workbench · X effort` cell to predict a production run.
//
// Production does NOT run this loop. A Workbench run hands the model to an
// EXTERNAL CODING AGENT — claude-code, opencode, whatever the agent's harness
// slug resolves to — running in a container with ITS OWN prompt, its own tool
// surface and its own turn budget (see `workbench-harnesses.ts`, a completely
// different contract that until a rename shared the spelling `defineHarness`).
// Talaria chooses the MODEL through the `code-*` roles and controls nothing
// else about that loop.
//
// So what these three harnesses measure is the model's own coding behaviour on a
// surface Talaria drives: locate the defect, edit the right file, check the
// work. That is genuinely predictive — a model that patches the wrong file here
// will patch the wrong file there — and it is NOT the same claim as "this model
// works under claude-code". A green cell is evidence about the model, not a
// rehearsal of the production path, and no fixture below should ever be written
// as though it were.
//
// THE TASKS ARE THE SAME ACROSS ALL THREE, deliberately. The efforts differ in
// what an admin SPENDS, not in what the job is; running a harder suite against
// the heavy slot would make the three columns incomparable, which is the one
// thing a matrix must not be.
import { defineHarness, type EvalCase, type EvalContext, type CheckResult } from '../define'
import type { ModelRole } from '../../model-roles'

export interface WorkbenchTaskInput {
  /** What the agent has been asked to do, as a work session would phrase it. */
  task: string
  /** The repository it starts from. */
  files: Array<{ path: string; content: string }>
  /** The failing test, as the runner first reported it. */
  failure: string
}

/** THE ORACLE, per task. Applied to the files as they stand — see
 *  `hermes-tools.ts` on why `run_tests` does not execute anything. */
type Oracle = (files: ReadonlyArray<{ path: string; content: string }>) => string | null

const fileAt = (files: ReadonlyArray<{ path: string; content: string }>, path: string): string => files.find((f) => f.path === path)?.content ?? ''

// ── Task 1: an off-by-one in a pagination helper ─────────────────────────────
//
// The classic. Small, unambiguous, one line to change, and the test names the
// expected value — so a model that reads the test knows the answer without
// having to reason about pagination at all. This is the EASY band's floor.

const PAGINATE = `export function pageRange(page, perPage, total) {
  const start = page * perPage
  const end = Math.min(start + perPage, total)
  return { start, end }
}
`

const PAGINATE_TEST = `import { pageRange } from './paginate.js'
import { test, assert } from './harness.js'

// Pages are ONE-BASED everywhere in this codebase.
test('page 1 starts at the beginning', () => {
  assert.deepEqual(pageRange(1, 20, 100), { start: 0, end: 20 })
})
test('page 3 of 20-per-page starts at 40', () => {
  assert.deepEqual(pageRange(3, 20, 100), { start: 40, end: 60 })
})
`

const paginateOracle: Oracle = (files) => {
  const src = fileAt(files, 'src/paginate.js')
  if (!src.trim()) return 'src/paginate.js is empty or missing'
  // The fix is one-based paging: `(page - 1) * perPage`. Accept any spelling
  // that computes it — a benchmark that demanded one exact diff would measure
  // obedience rather than capability.
  const oneBased = /\(\s*page\s*-\s*1\s*\)\s*\*\s*perPage|perPage\s*\*\s*\(\s*page\s*-\s*1\s*\)|start\s*=\s*page\s*\*\s*perPage\s*-\s*perPage/.test(src)
  if (!oneBased) return "pageRange(1, 20, 100) returned { start: 20, end: 40 }, expected { start: 0, end: 20 } — pages are one-based"
  if (!/Math\.min/.test(src)) return 'pageRange(5, 20, 90) returned { end: 100 }, expected 90 — end must be clamped to total'
  return null
}

// ── Task 2: the bug is NOT in the file the task names ────────────────────────
//
// The task says the total is wrong on the invoice. It is — because the rounding
// helper in a DIFFERENT file truncates instead of rounding. A model that edits
// the file it was pointed at makes the symptom worse and leaves the defect. This
// is what `search` and `list_files` are for, and it is the standard band's
// discriminator.

const INVOICE = `import { money } from './money.js'

export function invoiceTotal(lines) {
  let sum = 0
  for (const line of lines) sum += money(line.unitPrice * line.quantity)
  return money(sum)
}
`

const MONEY = `// Every monetary value in the system passes through here.
export function money(n) {
  return Math.trunc(n * 100) / 100
}
`

const INVOICE_TEST = `import { invoiceTotal } from './invoice.js'
import { test, assert } from './harness.js'

test('rounds each line to the nearest cent, not down', () => {
  assert.equal(invoiceTotal([{ unitPrice: 0.335, quantity: 1 }]), 0.34)
})
test('a three-line invoice totals correctly', () => {
  assert.equal(invoiceTotal([{ unitPrice: 1.005, quantity: 1 }, { unitPrice: 2.005, quantity: 1 }, { unitPrice: 0.005, quantity: 1 }]), 3.02)
})
`

const invoiceOracle: Oracle = (files) => {
  const money = fileAt(files, 'src/money.js')
  const invoice = fileAt(files, 'src/invoice.js')
  if (!money.trim()) return 'src/money.js is empty or missing'
  if (/Math\.trunc/.test(money)) {
    return "invoiceTotal([{ unitPrice: 0.335, quantity: 1 }]) returned 0.33, expected 0.34 — money() truncates where it should round"
  }
  if (!/Math\.round/.test(money)) return "invoiceTotal([{ unitPrice: 0.335, quantity: 1 }]) returned 0.33, expected 0.34"
  // Patching the symptom in the caller leaves every other consumer of money()
  // still truncating, which is the failure this task is built around.
  if (/Math\.round|toFixed/.test(invoice)) {
    return 'src/invoice.js now rounds for itself — every other caller of money() still truncates, so the defect is still there'
  }
  return null
}

// ── Task 3: a fix that must not break the passing tests ──────────────────────
//
// The reported bug is real and the obvious fix breaks a documented behaviour
// two tests already cover. The hard band's question: does this model read the
// rest of the suite before it changes shared code?

const SLUG = `export function slug(title) {
  return title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-|-$/g, '')
}
`

const SLUG_TEST = `import { slug } from './slug.js'
import { test, assert } from './harness.js'

test('collapses punctuation into single hyphens', () => {
  assert.equal(slug('Hello, World!'), 'hello-world')
})
test('trims leading and trailing hyphens', () => {
  assert.equal(slug('  spaced  '), 'spaced')
})
// Accented characters must survive as their base letter, not be eaten.
test('folds accents rather than dropping them', () => {
  assert.equal(slug('Café Münster'), 'cafe-munster')
})
`

const slugOracle: Oracle = (files) => {
  const src = fileAt(files, 'src/slug.js')
  if (!src.trim()) return 'src/slug.js is empty or missing'
  const folds = /normalize\(\s*['"]NF[KD]D['"]\s*\)/.test(src) || /\\u0300-\\u036f/.test(src) || /u0300/.test(src)
  if (!folds) return "slug('Café Münster') returned 'caf-m-nster', expected 'cafe-munster' — accents are being dropped rather than folded"
  // The two behaviours that already worked. A model that widens the character
  // class without re-reading the suite loses one of them.
  if (!/\^-\|-\$|replace\(\/\^-/.test(src)) return "slug('  spaced  ') returned '-spaced-', expected 'spaced' — the leading/trailing hyphen trim was lost"
  if (!/\+/.test(src)) return "slug('Hello, World!') returned 'hello--world', expected 'hello-world' — punctuation runs are no longer collapsed"
  return null
}

// ── The shared harness ───────────────────────────────────────────────────────

/** A test harness stub, so the repo reads like a real one and `list_files`
 *  shows something other than the two files under test. */
const HARNESS_JS = `// Minimal test harness: test(name, fn) and assert.equal / assert.deepEqual.
export const test = (name, fn) => fn()
export const assert = {
  equal: (a, b) => { if (a !== b) throw new Error(\`expected \${b}, got \${a}\`) },
  deepEqual: (a, b) => { if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(\`expected \${JSON.stringify(b)}, got \${JSON.stringify(a)}\`) },
}
`

const README = `# widgets

A small utility package. Run the suite with \`npm test\`.
Every monetary value passes through \`money()\` in src/money.js.
`

interface Task {
  name: string
  band: EvalCase<WorkbenchTaskInput, string>['band']
  input: WorkbenchTaskInput
  oracle: Oracle
  /** The file a correct fix has to change, for the "edited the right thing"
   *  assertion. */
  fixIn: string
}

const TASKS: Task[] = [
  {
    name: 'fixes an off-by-one the test spells out',
    band: 'easy',
    fixIn: 'src/paginate.js',
    oracle: paginateOracle,
    input: {
      task: 'The pagination helper returns the wrong range for page 1 — the first page skips the first 20 rows. Fix it and make the suite green.',
      failure: "src/paginate.test.js › page 1 starts at the beginning\n  expected {\"start\":0,\"end\":20}, got {\"start\":20,\"end\":40}",
      files: [
        { path: 'README.md', content: README },
        { path: 'src/harness.js', content: HARNESS_JS },
        { path: 'src/paginate.js', content: PAGINATE },
        { path: 'src/paginate.test.js', content: PAGINATE_TEST },
      ],
    },
  },
  {
    name: 'fixes the defect, not the file it was pointed at',
    band: 'standard',
    fixIn: 'src/money.js',
    oracle: invoiceOracle,
    input: {
      task: 'Invoice totals come out a cent low on some lines. The failing test is in src/invoice.test.js. Fix it and make the suite green.',
      failure: 'src/invoice.test.js › rounds each line to the nearest cent, not down\n  expected 0.34, got 0.33',
      files: [
        { path: 'README.md', content: README },
        { path: 'src/harness.js', content: HARNESS_JS },
        { path: 'src/invoice.js', content: INVOICE },
        { path: 'src/money.js', content: MONEY },
        { path: 'src/invoice.test.js', content: INVOICE_TEST },
      ],
    },
  },
  {
    name: 'fixes the reported bug without breaking what already worked',
    band: 'hard',
    fixIn: 'src/slug.js',
    oracle: slugOracle,
    input: {
      task: 'Slugs drop accented characters instead of folding them — "Café Münster" becomes "caf-m-nster". Fix it and make the suite green.',
      failure: "src/slug.test.js › folds accents rather than dropping them\n  expected cafe-munster, got caf-m-nster",
      files: [
        { path: 'README.md', content: README },
        { path: 'src/harness.js', content: HARNESS_JS },
        { path: 'src/slug.js', content: SLUG },
        { path: 'src/slug.test.js', content: SLUG_TEST },
      ],
    },
  },
]

/** The prompt a coding harness gets: the task, the failure, and the standing
 *  instruction to work through the tools rather than to answer in prose. */
const render = (input: WorkbenchTaskInput): Array<{ role: 'system' | 'user'; content: string }> => [
  {
    role: 'system',
    content: [
      'You are working in a checked-out repository through file tools. This is real work, not a question: nothing you say changes the code, only what you write with `write_file` does.',
      'Work like a developer at a desk: look at the tree, read the code AND the test that is failing, make the change, run the tests, read the result, keep going until they pass.',
      'Read a file before you replace it — `write_file` overwrites the whole file, so an edit written from memory silently deletes whatever you did not remember.',
      'Fix the DEFECT, not the symptom: if the failing test points at one file and the cause is in another, change the cause.',
      'When the suite is green, reply with one short line saying what was wrong and what you changed.',
    ].join('\n'),
  },
  { role: 'user', content: `Task: ${input.task}\n\nThe test runner reports:\n${input.failure}` },
]

/** THE THREE THINGS A CODING RUN CAN GET WRONG, as three fixtures per task.
 *
 *  They are not one assertion split for the sake of a count — they fail
 *  independently and mean different things, and a model can do any two without
 *  the third:
 *
 *    FIXED      the bug is gone, judged by the task's own oracle against the
 *               files as the model left them. Not "the diff matched": a real
 *               fix can be made in more than one place.
 *    CAREFULLY  it read a file before replacing it. `write_file` overwrites the
 *               whole file, so an edit written from memory silently deletes
 *               whatever the model did not remember — on a real repository that
 *               is the expensive mistake, and it is invisible in a green suite.
 *    VERIFIED   it ran the tests. A model that fixes the bug and never checks
 *               has done the work and cannot know it, which on a real ticket
 *               reads as success it never confirmed.
 *
 *  All three fail when nothing was called at all, which is what keeps them out
 *  of the sweep's garbage census. */
/** Every tool this sandbox offers, so the gap check below can tell a model that
 *  NARRATED a call from one that never made one. */
const WORKBENCH_TOOLS = ['list_files', 'read_file', 'search', 'write_file', 'run_tests']

/** DID THE RUN ACTUALLY GET A TOOL LOOP, before any assertion about what the
 *  model did with one.
 *
 *  THREE OUTCOMES, and the middle one is the whole point:
 *
 *    null    calls reached the sandbox; ask the real question.
 *    gap     no call reached us, but the reply NAMES a tool we offered — so the
 *            model tried and the loop did not receive it. That is our defect,
 *            and it has happened twice: `[tool] write_file({...})` and then
 *            `(called write_file)` were both written into the assistant's own
 *            prose, and models reproduced whichever they were shown instead of
 *            emitting a structured call. 34 replies in one sweep came back
 *            containing our narration verbatim, and every one was scored as a
 *            model that "read the repository and never wrote a file".
 *    string  no calls and no mention of one: it answered a coding task in prose,
 *            which is a real failure and changes nothing in the repository.
 *
 *  The gap branch cannot mask a genuine failure — a model that never intended to
 *  call a tool does not name one — and it stops us billing a model for a channel
 *  we did not give it. */
/** A FOREIGN CALL SYNTAX, which is the same failure as our own narration wearing
 *  a different coat. gemma emitted `call:file_control:list_files{path: "."}` — its
 *  own invented format, on turn one, imitating nothing of ours. The loop cannot
 *  parse it, so the call never happened as far as the sandbox is concerned.
 *
 *  That is still OUR gap and not the model's failure: the model tried to use the
 *  tool channel and this build could not receive it. Detecting it needs only the
 *  tool NAME plus a bracket nearby — a model discussing `run_tests` in prose does
 *  not put a brace after it. */
const NAMED_CALL = (value: string): string[] =>
  // The tool name, then at most a few closing/whitespace characters, then an
  // OPENING bracket — which is what every call syntax has and what prose about a
  // tool does not. It catches gemma's `list_files{path: "."}` and the older
  // `(called write_file)\n{"path": …}` alike, and leaves "you should run_tests
  // after fixing this" as the plain failure it is.
  WORKBENCH_TOOLS.filter((t) => new RegExp(`${t}[\\s)\\]"']{0,4}[({\\[]`).test(value))

const usedTools = (value: string, ctx: EvalContext): CheckResult => {
  if (ctx.calls.length > 0) return null
  const foreign = NAMED_CALL(value)
  if (foreign.length > 0) {
    return { gap: `the reply calls ${foreign.join(', ')} in a syntax this build does not parse — the call never reached the sandbox, so this run cannot be scored` }
  }
  // A BARE MENTION IS NOT AN ATTEMPTED CALL. The first version matched
  // `value.includes(toolName)`, which turned "you should run_tests after fixing
  // this" — a model explaining rather than acting, a real failure — into a gap.
  // A gap branch that launders real failures is worse than no gap branch, so the
  // syntax check above is the only one: a call has a bracket after it.
  return 'called no tool at all — it answered a coding task in prose, which changes nothing'
}

const workspaceOf = (ctx: EvalContext): { failure: string | null } | null => (ctx.world as { failure: string | null } | null)

const fixturesFor = (task: Task): Array<EvalCase<WorkbenchTaskInput, string>> => [
  {
    name: `${task.name} — the suite goes green`,
    band: task.band,
    input: task.input,
    check: (value, ctx) => {
      const noTools = usedTools(value, ctx)
      if (noTools) return noTools
      if (!ctx.calls.some((c) => c.tool === 'write_file' && c.error === null)) return 'read the repository and never wrote a file, so nothing was fixed'
      const failure = workspaceOf(ctx)?.failure
      return failure ? `the suite is still red: ${failure}` : null
    },
  },
  {
    name: `${task.name} — reads a file before replacing it`,
    band: task.band,
    input: task.input,
    check: (value, ctx) => {
      const noTools = usedTools(value, ctx)
      if (noTools) return noTools
      const wrote = ctx.calls.filter((c) => c.tool === 'write_file' && c.error === null)
      if (wrote.length === 0) return 'read the repository and never wrote a file, so nothing was fixed'
      const blind = wrote.filter((w) => {
        const path = String(w.args.path ?? '')
        // A `search` hit counts as having seen the file: it returns the matching
        // lines, which is how a developer actually locates a one-line fix.
        const seen = ctx.calls.findIndex((c) => (c.tool === 'read_file' && c.args.path === path) || c.tool === 'search')
        const at = ctx.calls.indexOf(w)
        return seen === -1 || seen > at
      })
      return blind.length === 0 ? null : `replaced ${blind.map((b) => `"${String(b.args.path)}"`).join(', ')} without reading it first — write_file overwrites the whole file`
    },
  },
  {
    name: `${task.name} — runs the tests before it calls it done`,
    band: task.band,
    input: task.input,
    check: (value, ctx) => {
      const noTools = usedTools(value, ctx)
      if (noTools) return noTools
      if (!ctx.calls.some((c) => c.tool === 'write_file' && c.error === null)) return 'read the repository and never wrote a file, so nothing was fixed'
      // A RUN WE CUT SHORT CANNOT BE ASKED ABOUT ITS ORDER. `exhausted` means the
      // model was still working when the turn budget ran out, so "did it verify
      // AFTER its last edit" has no answer — the sequence it would have finished
      // with never happened. Scoring it said the model verified a state it then
      // changed, which is a description of something that did not occur.
      //
      // This is our budget, not its mistake, and it goes in the gap list where
      // someone can decide whether six turns is enough.
      if (ctx.exhausted) {
        return { gap: `the turn budget ran out while the model was still working, so "did it re-run the tests after its last edit" cannot be asked of this run` }
      }
      if (!ctx.calls.some((c) => c.tool === 'run_tests')) return 'never ran the tests, so it reported work it had no way to verify'
      // Running them BEFORE the last edit and not after is the same failure
      // wearing a green tick: the model verified a state it then changed.
      const lastWrite = ctx.calls.map((c) => c.tool).lastIndexOf('write_file')
      const lastRun = ctx.calls.map((c) => c.tool).lastIndexOf('run_tests')
      return lastRun > lastWrite ? null : 'ran the tests and then edited again without re-running them, so the last change was never verified'
    },
  },
]

/** The three definitions, over one builder. Same tasks, same assertions, three
 *  `ModelSpec`s — which is what binds the three Workbench columns. */
const workbenchHarness = (role: ModelRole, id: string, label: string, effort: string) =>
  defineHarness<WorkbenchTaskInput, string>({
    id,
    label,
    job: `Drives a coding harness at ${effort} effort: reads the repository, edits files, and runs the tests until they pass.`,

    // The role's own declaration, restated where the fitness matrix reads it.
    // `tools` is not a quality bar here — without it the run does nothing while
    // reporting that it worked.
    requires: ['code', 'tools'],

    floor: {
      // NOTHING REFUSES. A weaker coder makes a worse change and a human reviews
      // every one of them before it merges — the Workbench's whole lifecycle is
      // platform-owned for exactly that reason. Refusing would take the
      // Workbench away from every self-host whose model nobody has probed.
      capabilities: [],
      refuseBelow: false,
      note: 'A weaker model makes smaller, clumsier changes and leans harder on review; it never merges anything by itself, because branches and PRs are the platform’s to drive.',
    },

    // THE BINDING. `score.ts` runs the real resolver over this spec, so naming
    // the role here is what puts a verdict in the Workbench column an admin is
    // actually assigning.
    model: { role },

    render: (input) => render(input),
    output: { kind: 'text', clean: (raw) => raw.trim() || null },

    // The caller keeps what it had: a workbench run that produced nothing leaves
    // the branch untouched, which is the safe end state and the one the session
    // loop already handles.
    onFailure: 'null',

    // THE TOOL LOOP IS THE JOB. Declared for the same reason `work-session`
    // declares it — a coding harness that cannot call tools cannot read a file.
    tools: 'own',

    // The file workspace comes from the FIXTURE, because the repository and the
    // oracle that decides whether its tests pass are properties of the task.
    dryRun: {
      // TEN TURNS, AND THE EVIDENCE IS ON THE RECORD. At six, nine of gemma's
      // workbench cases came back as OUR gap — "the turn budget ran out while
      // the model was still working, so 'did it re-run the tests after its last
      // edit' cannot be asked of this run" — and one of DeepSeek's did. The job
      // this harness poses is genuinely a lot of turns: list the tree, read the
      // file, find the defect, edit, run the tests, read the failure, edit
      // again, run them again. Six was a budget for a shorter job than the
      // fixtures describe, and the fixtures then asked whether the job was done.
      //
      // This is the raise that was reverted once, correctly, for having no
      // measurement behind it. It has one now.
      //
      // AND TEN WAS STILL SHORT — same argument, second measurement. Across a
      // twelve-model archive the workbench harnesses filed THREE more turn-budget
      // gaps at ten (`workbench:standard` twice, `workbench:light` once), and the
      // tool-call distribution says why: p50 is 6 calls, p90 is 8, and the tail
      // reaches 13. A model at the top of that distribution is still editing when
      // the loop stops, and the fixture then asks whether it re-ran the tests.
      //
      // Twelve is `MAX_TURN_CEILING`, so this is the last raise available without
      // moving that — and moving it is a cost decision (an unbounded loop on a
      // chatty model outspends the rest of the suite), not a fixture decision.
      // If twelve still gaps, the honest next move is a smaller task, not a
      // bigger budget.
      maxTurns: 12,
      workspace: (input) => ({
        files: input.files,
        passes: (files) => TASKS.find((t) => t.input.task === input.task)?.oracle(files) ?? null,
      }),
    },

    guard: {
      // Credentials only, and it matters here: a repository is one of the
      // likelier places for a key to be sitting, and a model that quotes one
      // back into its summary would put it in the run's own record.
      rules: ['secret_leak', 'pii_leak'],
      redact: true,
    },

    evals: TASKS.flatMap(fixturesFor),
  })

export const workbenchLightHarness = workbenchHarness('code-light', 'workbench:light', 'Workbench — light effort', 'light')
export const workbenchStandardHarness = workbenchHarness('code-standard', 'workbench:standard', 'Workbench — standard effort', 'standard')
export const workbenchHeavyHarness = workbenchHarness('code-heavy', 'workbench:heavy', 'Workbench — heavy effort', 'heavy')
