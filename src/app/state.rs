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
}
