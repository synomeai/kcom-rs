#[cfg(not(feature = "driver"))]
mod executor_edge_spec {
    use core::future::Future;
    use core::pin::Pin;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use core::task::{Context, Poll};
    use std::sync::Arc;

    use kcom::{spawn_dpc_task_cancellable, spawn_task, NTSTATUS, STATUS_SUCCESS};

    struct CountFuture {
        polls: Arc<AtomicUsize>,
    }

    impl Future for CountFuture {
        type Output = NTSTATUS;

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            self.polls.fetch_add(1, Ordering::Relaxed);
            Poll::Pending
        }
    }

    #[test]
    fn spawn_task_polls_once_on_host() {
        let polls = Arc::new(AtomicUsize::new(0));
        let fut = CountFuture { polls: polls.clone() };

        let status = spawn_task(fut);
        assert_eq!(status, STATUS_SUCCESS);
        assert_eq!(polls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn spawn_dpc_task_cancellable_polls_once_on_host() {
        let polls = Arc::new(AtomicUsize::new(0));
        let fut = CountFuture { polls: polls.clone() };

        let handle = unsafe { spawn_dpc_task_cancellable(fut) }.expect("spawn dpc task");
        assert_eq!(polls.load(Ordering::Relaxed), 1);
        assert!(!handle.is_cancelled());
        handle.cancel();
        assert!(handle.is_cancelled());
    }
}
