//! Pure domain types. No I/O, no UI, no `odl` types leak through here.

pub mod advanced;
pub mod capture;
pub mod category;
pub mod checksum;
pub mod filename;
pub mod headers;
pub mod job;
pub mod queue;
pub mod save_path;
pub mod settings;
pub mod user_agent;
pub mod watch_limit;

pub use advanced::{Advanced, AuthAdv, AuthScheme, CustomHeader, ProxyAdv, ProxyMode};
pub use capture::CaptureRequest;
pub use category::{Category, classify};
pub use checksum::{Algo, Checksum, CsSource, CsStatus};
pub use filename::{name_key, unique_name};
pub use headers::{has_header, header_name_eq, normalize_headers, upsert_header};
pub use job::{
    CapturedResponse, Job, JobError, JobId, JobSnapshot, JobStatus, LiveCounters, OnCompletion,
    Phase, PowerAction, ResponseHeader, SHUTDOWN_GRACE_SECS, ShutdownAction, SpeedSample,
    WillSendHeader, will_send_headers,
};
pub use queue::{
    CMD_INTERVAL_RANGE, CondCombine, CondCommand, CondKind, CondSet, IDLE_MINUTES_RANGE, Queue,
    QueueHook, QueueId, QueueSchedule, WeekDayMask, finish_summary, finish_title,
    random_vivid_color,
};
pub use save_path::{Destination, Resolver as SavePathResolver};
pub use settings::{
    Settings, Theme, default_category_folder, default_category_folders, detected_download_dir,
};
pub use user_agent::{default_user_agent, effective_user_agent};
pub use watch_limit::{WatchLimit, WatchLimitKind};
