- **A run that needs a person asks on its own page**: a parked run's question renders in
  place on the research surface — with the options the step itself offered — and answering
  there resumes the run (`POST /api/research/:id/decide`, authority-checked by the run's
  declared audience; a member reads the question, the owner answers it). Status stays
  four-value on the wire; the question rides beside it as `awaiting`, null the moment it
  is answered.
