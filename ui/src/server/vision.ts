// IMAGE UNDERSTANDING, SUPPLIED BY THE DEPLOYMENT.
//
// THE SHAPE OF THE PROBLEM. `vision` is a property of a MODEL, and most models
// do not have it — including several that are otherwise the right choice for an
// agent. Until now that was the end of the conversation: a blind model handed a
// screenshot could do nothing with it, and every surface that might have wanted
// one had to assume it was talking to a model that could see.
//
// It does not have to be the end. An org that has assigned ANY model to the
// `vision` role owns a way to read an image; a model that cannot see can still
// call a tool. So this is the second half of the pattern `capability-reach.ts`
// already established for search: the model does not do it, the DEPLOYMENT does,
// and the fitness matrix says `supplied` rather than `no`.
//
// WHY IT IS A HARNESS AND NOT A RAW CALL. The describing model is a model like
// any other: it needs routing, a capability floor, metering against the ledger,
// and a guard pass over what it returns — an image is untrusted input, and text
// extracted from one lands in a transcript. `runHarness` owns all of that, and
// a hand-written call would get some subset of it wrong in its own way, which is
// the whole argument `docs/HARNESSES.md` is built on.
//
// WHAT IT DELIBERATELY DOES NOT DO: pretend the calling model can see. The
// answer comes back as TEXT, attributed to the model that produced it, and the
// caller is told which one. A description is a second-hand account and every
// surface that shows it should be able to say so.
import { defineHarness } from './harness/define'
import { runHarness } from './harness/run'
import { gatewayImageTurn } from './harness/transport'

/** How long a description may take. Well under a harness turn budget: a caller
 *  is usually inside its own turn when it asks, and a describe that outlives the
 *  turn that wanted it is a timeout charged to the wrong model. */
const TIMEOUT_MS = 60_000

export interface DescribeInput {
  /** Data URI or https URL. The caller resolves an upload id to one of these —
   *  this module does not know what an upload is. */
  image: string
  /** What the asker actually needs to know. A description written blind is a
   *  paragraph about a screenshot; one written against a question is an answer. */
  question: string
}

/** THE DESCRIBING PROMPT, and the two rules that make its output usable.
 *
 *  DESCRIBE, DO NOT ADVISE. The caller is another model that will act on this;
 *  a describer that editorialises puts its own judgement into a chain where
 *  nobody can see it came from a second model.
 *
 *  SAY WHAT IS NOT THERE. "The error message is not visible in this crop" is the
 *  single most valuable sentence a describer produces, because the alternative
 *  is the calling model inventing one from a plausible-sounding description. */
const SYSTEM = [
  'You are reading an image on behalf of another agent that cannot see it. Answer its question from the image alone.',
  'Describe what is actually there — text verbatim where it is legible, layout and state where it matters, numbers exactly.',
  'If the image does not answer the question, say precisely what it does and does not show. Never guess at content that is cropped, blurred or absent.',
  'No advice, no next steps, no interpretation beyond what is visible. You are the eyes, not the decision.',
].join('\n')

const describeHarness = defineHarness<DescribeInput, string>({
  id: 'vision:describe',
  label: 'Describe an image',
  job: 'Reads an image on behalf of a model that cannot see, and answers one question about it.',
  // The one capability that is not optional here. `refuseBelow` is TRUE — unlike
  // most harnesses, degrading is not an option: a model that cannot see does not
  // produce a worse description, it produces a confident invention.
  requires: ['vision'],
  floor: {
    capabilities: ['vision'],
    refuseBelow: true,
    note: 'Describing an image needs a model that can see one. There is no degraded version of this: a blind model returns a plausible description of an image it never read.',
  },
  // THE ROLE, AND ONLY THE ROLE. An org assigns `vision` on the Models page and
  // that is the model that reads images. `chain: ['role']` rather than a
  // fallback: describing an image with whatever happened to route is how a
  // caller ends up trusting a description nobody chose the model for, and a
  // model that cannot see would be refused by the floor anyway — loudly, which
  // is the outcome an operator can act on.
  model: { role: 'vision', chain: ['role'] },
  render: (input) => [
    { role: 'system', content: SYSTEM },
    { role: 'user', content: input.question },
  ],
  output: { kind: 'text', clean: (raw) => raw.trim() || null },
  onFailure: 'null',
  // The description is untrusted text derived from an untrusted image, and it is
  // about to be handed to another model as fact. `ungrounded_ref` is the rule
  // that matters: a URL or an id "read off" an image nobody else can check is
  // exactly the shape of an injected instruction.
  guard: { rules: ['ungrounded_ref', 'secret_leak', 'pii_leak'], redact: true },
  holdMs: TIMEOUT_MS,
  temperature: 0.2,
})

export interface Description {
  text: string
  /** WHICH MODEL READ THE IMAGE. Reported so a caller can attribute it — a
   *  description is a second-hand account and a surface that presents it as the
   *  calling model's own observation is lying by omission. */
  model: string | null
  error: string | null
}

export interface VisionDeps {
  /** Swapped in tests. The real one runs the harness above through the runner. */
  describe: (input: DescribeInput) => Promise<Description>
}

const REAL: VisionDeps = {
  describe: async (input) => {
    // `gatewayImageTurn` is the seam that carries IMAGES, which `runHarness`'s
    // ordinary transport cannot: `Message.content` is a string by construction
    // (see the note on `Message` in harness/define.ts). So the runner resolves
    // the model, applies the floor and meters the call, and the image rides the
    // one transport built for it.
    const result = await runHarness(describeHarness, input, {
      caller: 'vision:describe',
      deps: {
        transport: async (req) => {
          const text = await gatewayImageTurn(req.model, req.messages, [input.image], 'vision:describe', { timeoutMs: TIMEOUT_MS })
          return { kind: 'gateway', text, toolNames: [], usage: null, contractDropped: false }
        },
      },
    })
    return { text: result.value ?? '', model: result.model, error: result.error ?? null }
  },
}

/** Read an image on behalf of a model that cannot. Never throws: a caller is a
 *  tool handler, and its error text goes into an agent's transcript — so a
 *  failure comes back as a sentence the calling model can act on. */
export async function describeImage(input: DescribeInput, deps?: Partial<VisionDeps>): Promise<Description> {
  const d = { ...REAL, ...deps }
  if (!input.image) return { text: '', model: null, error: 'no image was given' }
  if (!input.question.trim()) return { text: '', model: null, error: 'a description needs a question — say what you need to know from the image' }
  try {
    const out = await d.describe(input)
    if (!out.text) {
      return {
        text: '',
        model: out.model,
        error:
          out.error ??
          'no model is assigned to the vision role in this workspace, so images cannot be read here. Tell whoever asked that you cannot see the image rather than describing it.',
      }
    }
    return out
  } catch (err) {
    return { text: '', model: null, error: err instanceof Error ? err.message : String(err) }
  }
}
