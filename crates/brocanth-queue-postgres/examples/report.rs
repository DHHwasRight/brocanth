//! Print the conformance report for every backend, so the numbers in
//! `DESIGN.md` are copied from a run and not from memory.
use std::sync::Arc;

use brocanth_queue::DynBackend;

#[tokio::main]
async fn main() {
    let mem: Arc<dyn DynBackend> = Arc::new(brocanth_queue_memory::Memory::new());
    println!("{}", brocanth_queue_conformance::run(mem.clone()).await);
    println!(
        "{}",
        brocanth_queue_conformance::run(brocanth_queue_conformance::Adversary::new(mem)).await
    );
    let sqs: Arc<dyn DynBackend> = Arc::new(brocanth_queue_memory::Memory::sqs_shaped());
    println!("{}", brocanth_queue_conformance::run(sqs).await);

    if let Ok(url) = std::env::var("BROCANTH_QUEUE_TEST_DATABASE_URL") {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(8)
            .connect(&url)
            .await
            .unwrap();
        brocanth_queue_postgres::migrate(&pool).await.unwrap();
        let bulk: Arc<dyn DynBackend> =
            Arc::new(brocanth_queue_postgres::PgQueue::bulk(pool.clone()));
        println!("{}", brocanth_queue_conformance::run(bulk).await);
        let txq: Arc<dyn DynBackend> =
            Arc::new(brocanth_queue_postgres::PgQueue::transactional(pool));
        println!("{}", brocanth_queue_conformance::run(txq).await);
    }
}
