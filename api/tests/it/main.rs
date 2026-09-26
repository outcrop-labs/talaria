// The api's integration tests, as ONE test binary.
//
// Each file used to be its own crate under api/tests/, and each of those
// binaries statically linked the whole api: 36 links of 560 to 720 MiB each,
// about 20 GiB written by every `cargo test -p talaria-api`, which is what
// pinned dev boxes and the workbench VMs. As modules of one binary the api
// links once. Run one file's tests with a module filter:
//
//     cargo test -p talaria-api --test it runs_run::
//     cargo test -p talaria-api --test it plan_row_live:: -- --ignored
//
// CI's live suite (api-integration.yml) still runs module by module, so tests
// from different files never share the scratch database concurrently, exactly
// as when they were separate binaries.

mod support;

mod agent_reply_notify;
mod artifacts_read;
mod attribution;
mod board_agent_access;
mod boards_store;
mod brief_item_live;
mod brief_verdict_carry_live;
mod conversation_reads;
mod developer_agent_live;
mod fitness_arming;
mod fitness_transcripts_live;
mod gateway_hot_caches;
mod github_jwt;
mod github_pem_repair;
mod llm_models;
mod notifications_reads;
mod pending_dedupe_live;
mod plan_row_live;
mod price_rig_live;
mod push_live;
mod realtime;
mod realtime_fan;
mod research_live;
mod runs_decide;
mod runs_reclaim;
mod runs_run;
mod runs_store;
mod secretbox;
mod sender_identity_live;
mod streamed_at_liveness;
mod teams_move_live;
mod ticket_threads_live;
mod typed_binds;
mod unreads;
mod update_live;
mod uploads_live;
mod users_link;
mod workchains_live;
mod yaml_stringify;
