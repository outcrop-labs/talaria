- **Three code-probe tests were failing on CI the moment CI started running
  them, because they inherited a production wall clock.** `CODE_TIMEOUT_MS` is a
  *product* value: it bounds how long a candidate's code may run while the
  fitness suite grades a model, and 250ms is five orders of headroom for the
  kind of function those tasks ask for — **on a machine that is not doing
  anything else.** A shared CI runner is not that machine, so
  `accepts_the_two_wrappers_a_model_habitually_adds` and two siblings failed
  with `Script execution timed out after 250ms` while passing on a dev box.

  A wall-clock budget is the one thing a unit test must never inherit from
  production: the test is about whether the **grading** is right, and the budget
  decides whether it got the chance to be. So grading is now callable under an
  explicit budget (`run_code_task_within`), the module's grading tests use a
  generous one, and **the timeout keeps its own test** — a spinning candidate
  reaped at 50ms, which also pins that the sentence names the window actually
  waited on rather than interpolating the production constant regardless.

  Production is untouched: `run_code_task` still exists with the same signature
  and still gets 250ms.

  **Found because `cargo test --workspace` started running these crates'
  tests.** Nothing here is new breakage — it is the second thing the flip has
  surfaced, after the nine cross-check failures, and the shape is different and
  worth noting: those nine were stale assertions that had been wrong for weeks,
  this one is a test that is *correct* and whose environment was never the one
  it assumed. It also showed up as a flake rather than a clean failure — #524
  went green on the same `rc` while #525 went red — which is exactly how a
  marginal timing budget presents, and why it would have gone on intermittently
  blocking unrelated pull requests.

  **And the production budget moves too: 250ms → 2s.** The two failures are not
  symmetric. Too loose costs a spinning candidate a little more wall clock
  before it is cut — and the clock is not even what bounds runaway work, since
  `LOOP_ITERATION_LIMIT` and `RECURSION_LIMIT` reap the thread regardless. Too
  tight reports a **correct** model as one whose code "did not run", on a
  fitness page an admin uses to decide which model to trust — a false negative
  that reads as a property of the model and is actually a property of how busy
  the box was. 250ms was ample on an idle machine and provably not on a loaded
  one; an instance grading a sweep while serving traffic is the same kind of
  machine CI is. Two seconds is three orders of magnitude more than a correct
  solution needs and still cuts a genuine infinite loop long before anyone
  notices.

  Raising it does not remove the need for the test budget: it moves the number
  at which a loaded host starts reporting correct models as broken, it does not
  abolish it.

  One thing the change itself caught: `survives_an_infinite_loop_…` had been
  swept onto the generous budget by a mechanical rename, which quietly turned a
  test *about the production window* into one about the test window — and its
  message still claimed "250ms". It is back on `run_code_task`, asserts against
  `CODE_TIMEOUT_MS` rather than a literal, and the module header no longer
  quotes a number that can drift from the constant.

  Verified: `cargo test -p talaria-fitness-code-runner` 11 passed (was 10 —
  the new timeout test is the extra); the three previously-failing cases now
  grade under a budget a shared runner can meet. clippy clean across
  `talaria-fitness-code-runner` and `talaria-fitness`; `bun run gate` green.
