//! Phase 1 — concurrency + graceful-shutdown tests for `run_dispatcher`.
//!
//! These tests prove that `GMRAG_WORKER_CONCURRENCY` is honoured at runtime:
//! more than one popped job runs concurrently up to the configured bound,
//! and shutdown does not drop a job that is already in-flight.

use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gmrag_worker::{run_dispatcher, IngestJob, JobHandler, MockQueue};

fn job_payload(id: u8) -> Vec<u8> {
    let id = format!("{id:08x}-0000-0000-0000-000000000000");
    format!(
        r#"{{
            "id": "{id}",
            "tenant_id": "11111111-1111-1111-1111-111111111111",
            "workspace_id": "22222222-2222-2222-2222-222222222222",
            "document_id": "33333333-3333-3333-3333-333333333333",
            "s3_key": "k",
            "filename": "f.pdf",
            "owner_id": "44444444-4444-4444-4444-444444444444",
            "visibility": "private",
            "attempts": 0
        }}"#
    )
    .into_bytes()
}

/// Build a closure handler that records the max number of concurrently running
/// invocations and sleeps `hold` so we can observe overlap.
fn overlap_handler(hold: Duration) -> (JobHandler, Arc<AtomicI32>, Arc<AtomicI32>) {
    let cur = Arc::new(AtomicI32::new(0));
    let max = Arc::new(AtomicI32::new(0));
    let cur2 = cur.clone();
    let max2 = max.clone();
    let handler: JobHandler = Arc::new(move |_job: IngestJob| {
        let cur = cur2.clone();
        let max = max2.clone();
        Box::pin(async move {
            let now = cur.fetch_add(1, Ordering::SeqCst) + 1;
            let mut m = max.load(Ordering::SeqCst);
            while now > m {
                match max.compare_exchange(m, now, Ordering::SeqCst, Ordering::SeqCst) {
                    Ok(_) => break,
                    Err(v) => m = v,
                }
            }
            tokio::time::sleep(hold).await;
            cur.fetch_sub(1, Ordering::SeqCst);
        })
    });
    (handler, cur, max)
}

#[tokio::test]
async fn dispatcher_runs_jobs_concurrently_up_to_bound() {
    // 3 jobs, concurrency = 3 → all overlap (max concurrency hits 3).
    let payloads = vec![job_payload(1), job_payload(2), job_payload(3)];
    let queue = MockQueue::new(payloads);
    let (handler, _cur, max) = overlap_handler(Duration::from_millis(80));
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown = async move {
        let _ = rx.await;
    };
    let handle = tokio::spawn(run_dispatcher(queue, 3, shutdown, handler));

    // Let all 3 jobs run and overlap.
    tokio::time::sleep(Duration::from_millis(150)).await;
    let _ = tx.send(());
    let outcome = handle.await.unwrap().expect("dispatcher ok");

    assert_eq!(
        max.load(Ordering::SeqCst),
        3,
        "all 3 jobs must run concurrently when concurrency=3; got max={}",
        max.load(Ordering::SeqCst)
    );
    assert_eq!(outcome.jobs_popped, 3);
    assert_eq!(
        outcome.jobs_finished, 3,
        "all jobs must finish before shutdown drain completes"
    );
}

#[tokio::test]
async fn dispatcher_serializes_when_concurrency_is_one() {
    // 3 jobs, concurrency = 1 → no overlap (max concurrency stays 1).
    let payloads = vec![job_payload(1), job_payload(2), job_payload(3)];
    let queue = MockQueue::new(payloads);
    let (handler, _cur, max) = overlap_handler(Duration::from_millis(60));
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown = async move {
        let _ = rx.await;
    };
    let handle = tokio::spawn(run_dispatcher(queue, 1, shutdown, handler));

    tokio::time::sleep(Duration::from_millis(250)).await;
    let _ = tx.send(());
    let outcome = handle.await.unwrap().expect("dispatcher ok");

    assert_eq!(
        max.load(Ordering::SeqCst),
        1,
        "concurrency=1 must serialize jobs (max concurrent == 1), got {}",
        max.load(Ordering::SeqCst)
    );
    assert_eq!(outcome.jobs_popped, 3);
    assert_eq!(outcome.jobs_finished, 3);
}

#[tokio::test]
async fn dispatcher_does_not_drop_inflight_job_on_shutdown() {
    // One slow job: shutdown fires WHILE the job is running. The in-flight
    // job must run to completion; the dispatcher must not abort it.
    let queue = MockQueue::new(vec![job_payload(1)]);
    let started = Arc::new(AtomicU32::new(0));
    let completed = Arc::new(AtomicU32::new(0));
    let started_c = started.clone();
    let completed_c = completed.clone();
    let handler: JobHandler = Arc::new(move |_job: IngestJob| {
        let started = started_c.clone();
        let completed = completed_c.clone();
        Box::pin(async move {
            started.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(150)).await;
            completed.fetch_add(1, Ordering::SeqCst);
        })
    });
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown = async move {
        let _ = rx.await;
    };
    let handle = tokio::spawn(run_dispatcher(queue, 1, shutdown, handler));

    // Wait until the dispatcher has actually started the in-flight job.
    let mut waited = 0;
    while started.load(Ordering::SeqCst) == 0 && waited < 2000 {
        tokio::time::sleep(Duration::from_millis(5)).await;
        waited += 5;
    }
    assert!(
        started.load(Ordering::SeqCst) >= 1,
        "job must start before shutdown"
    );

    // Fire shutdown while the job is mid-sleep.
    let _ = tx.send(());
    let outcome = handle.await.unwrap().expect("dispatcher ok");

    assert_eq!(
        completed.load(Ordering::SeqCst),
        1,
        "in-flight job must complete despite shutdown (no silent drop)"
    );
    assert_eq!(outcome.jobs_finished, 1);
}
