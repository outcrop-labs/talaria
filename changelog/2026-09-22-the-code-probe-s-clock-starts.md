- **The code probe's clock starts at the candidate, not the engine.** The
  fitness code tasks ran inside a 250ms window that also covered building
  the boa context evaluating them — on a loaded host (CI's runners proved
  it) a cold context build ate the whole window and a correct solution
  failed with "the code did not run". The evaluator now signals when the
  engine is built and the window times only the candidate's script;
  an infinite loop still costs its 250ms exactly as before.

### Added
