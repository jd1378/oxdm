use async_trait::async_trait;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::domain::{JobError, JobId};

/// Handle the pause strategy operates on. Carries the bits a future
/// HTTP-level pause would need (the live `odl` cancel token plus the
/// owning state).
#[derive(Clone)]
pub struct JobHandle {
    pub id: JobId,
    pub cancel: CancellationToken,
}

/// Pause / resume policy.
///
/// v0 ships [`CancelResumeStrategy`]: pause = trip the cancel token,
/// resume = restart `evaluate → download` (ODL recovers from
/// `metadata.pb`). A future `HttpPauseStrategy` can drop in once `odl`
/// grows a real pause API; the UI and runner do not change.
#[async_trait]
pub trait PauseStrategy: Send + Sync + 'static {
    async fn pause(&self, handle: &JobHandle) -> Result<(), JobError>;
    async fn resume(&self, handle: &JobHandle, ctx: &dyn ResumeContext) -> Result<(), JobError>;
    /// Whether bytes already in flight are kept across a pause. Drives
    /// the UI's "Paused — will resume from last byte" copy.
    fn preserves_in_flight_bytes(&self) -> bool;
}

/// Decoupled callback the strategy uses to ask the host to spawn a
/// fresh runner for a paused job. Avoids a circular `AppState` dep here.
#[async_trait]
pub trait ResumeContext: Send + Sync {
    async fn relaunch(&self, id: JobId) -> Result<(), JobError>;
}

/// Default v0 implementation. Pause = cancel; Resume = relaunch a runner.
pub struct CancelResumeStrategy;

#[async_trait]
impl PauseStrategy for CancelResumeStrategy {
    async fn pause(&self, handle: &JobHandle) -> Result<(), JobError> {
        handle.cancel.cancel();
        Ok(())
    }

    async fn resume(&self, handle: &JobHandle, ctx: &dyn ResumeContext) -> Result<(), JobError> {
        ctx.relaunch(handle.id).await
    }

    fn preserves_in_flight_bytes(&self) -> bool {
        false
    }
}

/// Box helper used by `AppState` to hold whichever strategy is active.
pub type DynPauseStrategy = Arc<dyn PauseStrategy>;
