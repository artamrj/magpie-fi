use crate::{config::Config, types::WorkflowPhase};
use serde::Serialize;
use sqlx::SqlitePool;
use std::{sync::Arc, time::Duration};
use tokio::sync::{Notify, RwLock, Semaphore};

#[derive(Clone, Debug, Default, Serialize)]
pub struct Workflow {
    pub phase: WorkflowPhase,
    pub message: String,
    pub current_file: Option<String>,
    pub current: usize,
    pub total: usize,
    pub processed: usize,
    #[serde(skip)]
    pub cancelled: bool,
}

pub struct AppState {
    pub config: Config,
    pub pool: SqlitePool,
    pub client: reqwest::Client,
    pub artwork_downloads: Arc<Semaphore>,
    pub tag_writes: Arc<Semaphore>,
    pub workflow: RwLock<Workflow>,
    automation_notify: Notify,
    acquire_notify: Notify,
}

impl AppState {
    pub fn new(config: Config, pool: SqlitePool) -> Self {
        Self {
            config,
            pool,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(12))
                .user_agent(crate::config::USER_AGENT)
                .build()
                .expect("HTTP client should build"),
            artwork_downloads: Arc::new(Semaphore::new(crate::config::LOOKUP_WORKERS)),
            tag_writes: Arc::new(Semaphore::new(crate::config::WRITE_WORKERS)),
            workflow: RwLock::new(Workflow {
                message: "Ready".into(),
                ..Default::default()
            }),
            automation_notify: Notify::new(),
            acquire_notify: Notify::new(),
        }
    }

    pub async fn workflow_running(&self) -> bool {
        matches!(
            self.workflow.read().await.phase,
            WorkflowPhase::Scan | WorkflowPhase::Fetch | WorkflowPhase::Apply
        )
    }

    /// Atomically claims the workflow slot unless one is already running.
    /// The check and the phase transition share a single write lock, so two
    /// racing requests can never both start a workflow.
    pub async fn try_claim_workflow(
        &self,
        phase: WorkflowPhase,
        message: impl Into<String>,
    ) -> bool {
        let mut workflow = self.workflow.write().await;
        if matches!(
            workflow.phase,
            WorkflowPhase::Scan | WorkflowPhase::Fetch | WorkflowPhase::Apply
        ) {
            return false;
        }
        *workflow = Workflow {
            phase,
            message: message.into(),
            ..Default::default()
        };
        true
    }

    pub async fn reset_workflow(&self, phase: WorkflowPhase, message: impl Into<String>) {
        *self.workflow.write().await = Workflow {
            phase,
            message: message.into(),
            ..Default::default()
        };
    }

    /// Runs `work` in the workflow slot and frees the slot when it ends.
    /// `None` when another workflow holds the slot.
    ///
    /// The work runs on its own task. The page gives up on a request after
    /// 30 s and a browser tab can close at any time; the request is then
    /// dropped, but the work still finishes (an artist is never left half
    /// renamed) and the slot is freed. It is freed after a panic as well, so
    /// one bad song cannot block every later scan and filing until a restart.
    pub async fn run_in_workflow<T, F>(
        self: &Arc<Self>,
        phase: WorkflowPhase,
        message: impl Into<String>,
        work: F,
    ) -> Option<Result<T, tokio::task::JoinError>>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        if !self.try_claim_workflow(phase, message).await {
            return None;
        }
        let state = self.clone();
        let finished = tokio::spawn(async move {
            let outcome = tokio::spawn(work).await;
            state.reset_workflow(WorkflowPhase::Idle, "Ready").await;
            outcome
        });
        Some(finished.await.and_then(|outcome| outcome))
    }

    /// Runs work that already holds the workflow slot on its own task. When
    /// the work panics, the panic stays inside that task: the slot is given
    /// back with a "failed" status the page shows, and `None` is returned.
    /// Without this, one song that trips a bug would block every later scan
    /// and filing until a restart.
    pub async fn guard_workflow<T, F>(self: &Arc<Self>, what: &'static str, work: F) -> Option<T>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        match tokio::spawn(work).await {
            Ok(done) => Some(done),
            Err(error) => {
                tracing::error!("{what} stopped unexpectedly: {error}");
                self.reset_workflow(
                    WorkflowPhase::Failed,
                    format!("{what} stopped unexpectedly. It will be tried again."),
                )
                .await;
                None
            }
        }
    }

    pub async fn cancel_workflow(&self) {
        self.workflow.write().await.cancelled = true;
    }

    pub async fn workflow_cancelled(&self) -> bool {
        self.workflow.read().await.cancelled
    }

    pub async fn finish_workflow(&self, phase: WorkflowPhase, message: impl Into<String>) {
        let mut workflow = self.workflow.write().await;
        workflow.phase = phase;
        workflow.message = message.into();
        workflow.cancelled = false;
        workflow.current_file = None;
    }

    pub fn notify_automation_scheduler(&self) {
        self.automation_notify.notify_one();
    }

    pub async fn wait_for_automation_change(&self) {
        self.automation_notify.notified().await;
    }

    /// Wakes the download worker (new songs queued, or a slot became free).
    pub fn notify_acquire(&self) {
        self.acquire_notify.notify_one();
    }

    pub async fn wait_for_acquire(&self) {
        self.acquire_notify.notified().await;
    }

    pub async fn set_workflow(
        &self,
        phase: WorkflowPhase,
        message: impl Into<String>,
        current: usize,
        total: usize,
        file: Option<String>,
    ) {
        let mut workflow = self.workflow.write().await;
        workflow.phase = phase;
        workflow.message = message.into();
        workflow.current = current;
        workflow.total = total;
        workflow.current_file = file;
    }

    pub async fn start_track(
        &self,
        total: usize,
        file: impl Into<String>,
        message: impl Into<String>,
    ) {
        let mut workflow = self.workflow.write().await;
        workflow.phase = WorkflowPhase::Fetch;
        workflow.message = message.into();
        workflow.total = total;
        workflow.current_file = Some(file.into());
    }

    pub async fn finish_track(&self, total: usize) -> usize {
        let mut workflow = self.workflow.write().await;
        workflow.processed += 1;
        workflow.current = workflow.processed;
        workflow.total = total;
        workflow.processed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn claim_is_atomic_and_refuses_while_running() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("claim-atomic.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let state = AppState::new(Config::default(), pool);

        assert!(
            state
                .try_claim_workflow(WorkflowPhase::Scan, "First scan")
                .await
        );
        assert!(
            !state
                .try_claim_workflow(WorkflowPhase::Apply, "Second workflow")
                .await
        );
        assert!(!state.try_claim_workflow(WorkflowPhase::Scan, "Third").await);
        assert_eq!(state.workflow.read().await.phase, WorkflowPhase::Scan);
    }

    async fn test_state(name: &str) -> (tempfile::TempDir, Arc<AppState>) {
        let directory = tempfile::tempdir().unwrap();
        let pool =
            crate::infrastructure::db::connect(directory.path().join(name).to_str().unwrap())
                .await
                .unwrap();
        (directory, Arc::new(AppState::new(Config::default(), pool)))
    }

    #[tokio::test]
    async fn work_finishes_and_frees_the_slot_when_the_request_is_dropped() {
        let (_directory, state) = test_state("dropped-request.sqlite").await;
        let (started, wait_for_start) = tokio::sync::oneshot::channel::<()>();
        let (let_finish, finish) = tokio::sync::oneshot::channel::<()>();
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let request = tokio::spawn({
            let (state, done) = (state.clone(), done.clone());
            async move {
                state
                    .run_in_workflow(WorkflowPhase::Apply, "Renaming", async move {
                        started.send(()).unwrap();
                        finish.await.unwrap();
                        done.store(true, std::sync::atomic::Ordering::SeqCst);
                    })
                    .await
            }
        });
        wait_for_start.await.unwrap();
        // The browser gave up: the request is dropped in the middle of the work.
        request.abort();
        let _ = request.await;
        assert!(state.workflow_running().await);

        let_finish.send(()).unwrap();
        for _ in 0..200 {
            if !state.workflow_running().await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(done.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!state.workflow_running().await);
    }

    #[tokio::test]
    async fn a_panic_in_the_work_frees_the_slot() {
        let (_directory, state) = test_state("panic.sqlite").await;
        let outcome = state
            .run_in_workflow(WorkflowPhase::Apply, "Saving", async {
                panic!("a song nobody expected");
            })
            .await;
        assert!(matches!(outcome, Some(Err(_))));
        assert!(!state.workflow_running().await);

        assert!(state.try_claim_workflow(WorkflowPhase::Scan, "Scan").await);
        let guarded: Option<()> = state
            .guard_workflow("The inbox check", async {
                panic!("a song nobody expected");
            })
            .await;
        assert!(guarded.is_none());
        assert!(!state.workflow_running().await);
        assert_eq!(state.workflow.read().await.phase, WorkflowPhase::Failed);
        assert_eq!(
            state.guard_workflow("The inbox check", async { 7 }).await,
            Some(7)
        );

        // And the slot is refused, not queued, while something else runs.
        assert!(state.try_claim_workflow(WorkflowPhase::Scan, "Scan").await);
        let refused = state
            .run_in_workflow(WorkflowPhase::Apply, "Saving", async {})
            .await;
        assert!(refused.is_none());
    }
}
