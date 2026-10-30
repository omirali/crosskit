// The background service's own files and the one system program it talks to.
//
// Everything here is about the service as a program on this machine — where its copy of the
// binary lives, what launchd is told, where it writes what it did — and nothing is about
// what it watches: a context is watched by the same composition `index watch` runs.
mod file_stamp;
mod instance_lock;
mod journal;
mod launch_agent;
mod launchctl;
mod layout;
mod program_copy;
mod protocol;
mod socket_client;
mod socket_place;
mod socket_server;
mod status_file;
mod stop_broadcast;

pub use file_stamp::{FileStamp, content_digest, file_stamp, now_ms};
pub use instance_lock::{InstanceLock, InstanceLockError, instance_holder};
pub use journal::{Journal, trim_launchd_output};
pub use launch_agent::{LaunchAgent, SERVICE_LABEL};
pub use launchctl::Launchctl;
pub use layout::ServiceLayout;
pub use program_copy::copy_program;
pub use protocol::{PROTOCOL, Refused, ServiceRequest, refusal_line};
pub use socket_client::{Served, ask_service};
pub use socket_place::{PlacedSocket, SOCKET_NAME, place_socket};
pub use socket_server::{ServedReads, ServerLimits, serve};
pub use status_file::{
    ContextPhase, ContextStatus, ServiceStatus, StatusBoard, TimedLine, WatchedBy, read_status,
};
pub use stop_broadcast::StopBroadcast;
