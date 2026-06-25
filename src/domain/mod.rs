//! Pure domain types. No I/O, no UI, no `odl` types leak through here.

pub mod advanced;
pub mod capture;
pub mod category;
pub mod checksum;
pub mod host_setting;
pub mod job;
pub mod queue;
pub mod settings;

pub use advanced::{Advanced, AuthAdv, AuthScheme, CustomHeader, ProxyAdv, ProxyMode};
pub use capture::CaptureRequest;
pub use category::{Category, classify};
pub use checksum::{Algo, Checksum, CsSource, CsStatus};
pub use host_setting::HostSetting;
pub use job::{
    Job, JobError, JobId, JobSnapshot, JobStatus, LiveCounters, OnCompletion, Phase,
    ShutdownAction, SpeedSample,
};
pub use queue::{Queue, QueueHook, QueueId, QueueSchedule, WeekDayMask, random_vivid_color};
pub use settings::{ConflictWhileHidden, Density, Settings, Theme};
