pub mod ssh;
pub mod telnet;

/// Every generic terminal transport in this module represents a single
/// logical session, so it always uses connection id 0.
pub(crate) const CONN_ID: crate::ConnectionId = 0;
