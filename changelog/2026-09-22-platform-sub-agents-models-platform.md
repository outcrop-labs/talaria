- **Platform sub-agents (Models → Platform).** Talaria's own workers are
  now first-class, named agents — separate from the Hermes fleet, each a
  model-agnostic harness with its own skills for one internal job: Muse
  (prompt-editing everywhere), Distiller (chat → private-brain distills),
  Concluder (relay closing summaries), Catalog writer (model blurbs),
  Judge (ticket outcome review), and Briefer (view briefings; fixed to
  your personal assistant by design). Each agent's model is configured
  granularly on the new Models → Platform tab — an admin pick wins while
  it routes, otherwise the job's auto chain keeps working untouched (the
  Judge's pick shares judge_config with the Guard panel, one source of
  truth). All platform work is now metered under `platform:<agent>`
  callers, so spend per sub-agent is attributable.
