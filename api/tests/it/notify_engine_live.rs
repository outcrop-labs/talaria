// Live-DB proof for the delivery engine (docs/notifications/delivery.md).
// A placeholder the contract PR registers, so W2-E fills this module
// without touching main.rs. House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test -p talaria-api --test it notify_engine_live:: -- --ignored

#[tokio::test]
#[ignore = "placeholder: W2-E fills this module"]
async fn placeholder() {}
