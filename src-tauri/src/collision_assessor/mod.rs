// These retained formal-model implementations are exercised by the adversarial test target.
// Production admission uses the native registry plus orchestrator authority runtime, so compiling
// the alternative models into the desktop binary would increase its surface without adding a
// callable path.
#[cfg(test)]
pub(crate) mod admission;
#[cfg(test)]
pub(crate) mod analyzer;
pub mod api;
pub(crate) mod authority;
pub mod capability;
#[cfg(test)]
pub(crate) mod clearance;
pub mod collector_process;
pub mod discovery;
pub mod identity;
#[cfg(test)]
pub(crate) mod journal;
pub mod model;
pub mod registry;
#[cfg(test)]
pub(crate) mod snapshot;
#[cfg(test)]
pub(crate) mod tickets;
