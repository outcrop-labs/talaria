- **W12a — one home for the cli's env, compose, port and container spellings.**
  `readEnvFile(ctx, rel, { quotes?, envWins? })` replaces ten hand-assembled
  `.env` readers; `composeFileArgs(composeFile, base)` (+`COMPOSE_BASE`,
  `composeFileEnv`) replaces seven copies of the COMPOSE_FILE law — the printed
  argv the two tests pin is byte-identical; `cli/src/ports.ts` owns the five dev
  host ports and `cli/src/containers.ts` the seven container names plus the
  `containerState`/`containerRunning`/`containerExists`/`containerNames` probes
  (each keeping its original docker argv); `cmd/box/seed.ts`'s ~18 string
  concatenations read `boxSvc`/`boxHost`/`boxProject`/`boxFleetNetwork`; and the
  duplicate `COMPOSE_FILE` export in `cmd/service/shared.ts` is gone.
  `composeFileArgs` takes the already-resolved value rather than `ctx` because
  `service/unit.ts` is a pure renderer with no `Ctx` — the ctx side is
  `composeFileEnv(ctx)`.
