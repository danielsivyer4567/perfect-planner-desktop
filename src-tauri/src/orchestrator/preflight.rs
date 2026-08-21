use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessIdentity {
    pub pid: u32,
    pub executable_path: PathBuf,
    pub started_at_epoch_ms: u64,
    pub command_line: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortBinding {
    pub port: u16,
    pub address: String,
    pub process: ProcessIdentity,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSnapshot {
    pub logical_cpu_count: usize,
    pub cpu_usage_percent: f32,
    pub total_memory_bytes: u64,
    pub available_memory_bytes: u64,
    pub repository_disk_available_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemBaseline {
    pub repository_root: PathBuf,
    pub git_status_porcelain_v2: String,
    pub port_bindings: Vec<PortBinding>,
    pub resources: ResourceSnapshot,
}

pub trait SystemProbe {
    fn git_status_porcelain_v2(&self, repository_root: &Path) -> Result<String, String>;
    fn port_bindings(&self) -> Result<Vec<PortBinding>, String>;
    fn resources(&self, repository_root: &Path) -> Result<ResourceSnapshot, String>;
}

/// Process termination is deliberately separated from discovery so that callers can use a
/// read-only probe with the default deny adapter. Production code must inject a real adapter.
pub trait ProcessAdapter {
    fn stop(&self, process: &ProcessIdentity) -> Result<(), String>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DenyProcessAdapter;

impl ProcessAdapter for DenyProcessAdapter {
    fn stop(&self, _process: &ProcessIdentity) -> Result<(), String> {
        Err("process stopping is disabled; inject an explicit ProcessAdapter".to_string())
    }
}

#[derive(Clone, Debug)]
pub struct PreflightRequest {
    pub repository_root: PathBuf,
    pub required_ports: BTreeSet<u16>,
    /// Exact, immutable process identities. Matching only by PID or executable is forbidden.
    pub process_allowlist: BTreeSet<ProcessIdentity>,
    /// This is false by default. A UI decision must set it for the current assessment.
    pub stop_allowlisted_conflicts: bool,
}

impl PreflightRequest {
    pub fn inspect_only(repository_root: impl Into<PathBuf>) -> Self {
        Self {
            repository_root: repository_root.into(),
            required_ports: BTreeSet::new(),
            process_allowlist: BTreeSet::new(),
            stop_allowlisted_conflicts: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PreflightDisposition {
    Ready,
    DecisionRequired,
    StoppedAllowlistedConflicts,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightReport {
    pub disposition: PreflightDisposition,
    pub baseline: SystemBaseline,
    pub conflicts: Vec<PortBinding>,
    pub unknown_conflicts: Vec<PortBinding>,
    pub stopped_processes: Vec<ProcessIdentity>,
    pub reasons: Vec<String>,
}

pub struct PreflightEngine<P, A> {
    probe: P,
    process_adapter: A,
}

impl<P, A> PreflightEngine<P, A>
where
    P: SystemProbe,
    A: ProcessAdapter,
{
    pub fn new(probe: P, process_adapter: A) -> Self {
        Self {
            probe,
            process_adapter,
        }
    }

    pub fn run(&self, request: &PreflightRequest) -> Result<PreflightReport, String> {
        validate_repository(&request.repository_root)?;

        // Capture the entire baseline before considering any mutation.
        let baseline = SystemBaseline {
            repository_root: request.repository_root.clone(),
            git_status_porcelain_v2: self
                .probe
                .git_status_porcelain_v2(&request.repository_root)?,
            port_bindings: self.probe.port_bindings()?,
            resources: self.probe.resources(&request.repository_root)?,
        };

        validate_resources(&baseline.resources)?;

        let conflicts: Vec<_> = baseline
            .port_bindings
            .iter()
            .filter(|binding| request.required_ports.contains(&binding.port))
            .cloned()
            .collect();
        let unknown_conflicts: Vec<_> = conflicts
            .iter()
            .filter(|binding| !request.process_allowlist.contains(&binding.process))
            .cloned()
            .collect();

        // This check is intentionally before every call to stop(). One unknown identity makes the
        // whole operation non-mutating; known processes are not partially stopped.
        if !unknown_conflicts.is_empty() {
            return Ok(PreflightReport {
                disposition: PreflightDisposition::DecisionRequired,
                baseline,
                conflicts,
                unknown_conflicts,
                stopped_processes: Vec::new(),
                reasons: vec![
                    "one or more port conflicts do not exactly match the process allowlist"
                        .to_string(),
                ],
            });
        }

        if conflicts.is_empty() {
            return Ok(PreflightReport {
                disposition: PreflightDisposition::Ready,
                baseline,
                conflicts,
                unknown_conflicts,
                stopped_processes: Vec::new(),
                reasons: Vec::new(),
            });
        }

        if !request.stop_allowlisted_conflicts {
            return Ok(PreflightReport {
                disposition: PreflightDisposition::DecisionRequired,
                baseline,
                conflicts,
                unknown_conflicts,
                stopped_processes: Vec::new(),
                reasons: vec![
                    "explicit approval is required to stop allowlisted conflicts".to_string(),
                ],
            });
        }

        let processes: BTreeSet<_> = conflicts
            .iter()
            .map(|binding| binding.process.clone())
            .collect();
        let mut stopped_processes = Vec::with_capacity(processes.len());
        for process in processes {
            self.process_adapter.stop(&process).map_err(|error| {
                format!(
                    "failed to stop approved process {} (pid {}): {error}",
                    process.executable_path.display(),
                    process.pid
                )
            })?;
            stopped_processes.push(process);
        }

        Ok(PreflightReport {
            disposition: PreflightDisposition::StoppedAllowlistedConflicts,
            baseline,
            conflicts,
            unknown_conflicts,
            stopped_processes,
            reasons: Vec::new(),
        })
    }
}

fn validate_repository(repository_root: &Path) -> Result<(), String> {
    if !repository_root.is_absolute() {
        return Err("repository root must be absolute".to_string());
    }
    if !repository_root.is_dir() {
        return Err("repository root must be an existing directory".to_string());
    }
    if !repository_root.join(".git").exists() {
        return Err("repository root must contain a .git file or directory".to_string());
    }
    Ok(())
}

fn validate_resources(resources: &ResourceSnapshot) -> Result<(), String> {
    if resources.logical_cpu_count == 0 {
        return Err("system probe returned zero logical CPUs".to_string());
    }
    if !resources.cpu_usage_percent.is_finite()
        || !(0.0..=100.0).contains(&resources.cpu_usage_percent)
    {
        return Err("system probe returned invalid CPU usage".to_string());
    }
    if resources.available_memory_bytes > resources.total_memory_bytes {
        return Err("system probe returned available memory greater than total memory".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_ID: AtomicU64 = AtomicU64::new(1);

    struct TempRepo(PathBuf);

    impl TempRepo {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "perfect-planner-preflight-{}-{}",
                std::process::id(),
                TEMP_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(path.join(".git")).unwrap();
            Self(path)
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[derive(Clone)]
    struct MockProbe {
        bindings: Vec<PortBinding>,
    }

    impl SystemProbe for MockProbe {
        fn git_status_porcelain_v2(&self, _repository_root: &Path) -> Result<String, String> {
            Ok("1 .M N... 100644 100644 100644 abc abc src/lib.rs\n".to_string())
        }

        fn port_bindings(&self) -> Result<Vec<PortBinding>, String> {
            Ok(self.bindings.clone())
        }

        fn resources(&self, _repository_root: &Path) -> Result<ResourceSnapshot, String> {
            Ok(ResourceSnapshot {
                logical_cpu_count: 8,
                cpu_usage_percent: 12.5,
                total_memory_bytes: 16_000,
                available_memory_bytes: 8_000,
                repository_disk_available_bytes: 100_000,
            })
        }
    }

    #[derive(Default)]
    struct RecordingAdapter {
        stopped: RefCell<Vec<ProcessIdentity>>,
    }

    impl ProcessAdapter for RecordingAdapter {
        fn stop(&self, process: &ProcessIdentity) -> Result<(), String> {
            self.stopped.borrow_mut().push(process.clone());
            Ok(())
        }
    }

    fn process(pid: u32, name: &str) -> ProcessIdentity {
        ProcessIdentity {
            pid,
            executable_path: PathBuf::from(format!("C:\\tools\\{name}.exe")),
            started_at_epoch_ms: 1_700_000_000_000 + u64::from(pid),
            command_line: format!("{name}.exe --serve"),
        }
    }

    fn binding(port: u16, process: ProcessIdentity) -> PortBinding {
        PortBinding {
            port,
            address: "127.0.0.1".to_string(),
            process,
        }
    }

    #[test]
    fn unknown_conflict_requires_decision_and_stops_nothing() {
        let repository = TempRepo::new();
        let known = process(10, "known");
        let unknown = process(11, "unknown");
        let adapter = RecordingAdapter::default();
        let engine = PreflightEngine::new(
            MockProbe {
                bindings: vec![binding(5193, known.clone()), binding(5194, unknown)],
            },
            adapter,
        );
        let request = PreflightRequest {
            repository_root: repository.0.clone(),
            required_ports: [5193, 5194].into_iter().collect(),
            process_allowlist: [known].into_iter().collect(),
            stop_allowlisted_conflicts: true,
        };

        let report = engine.run(&request).unwrap();

        assert_eq!(report.disposition, PreflightDisposition::DecisionRequired);
        assert_eq!(report.unknown_conflicts.len(), 1);
        assert!(report.stopped_processes.is_empty());
        assert!(engine.process_adapter.stopped.borrow().is_empty());
    }

    #[test]
    fn allowlisted_conflict_still_needs_explicit_stop_decision() {
        let repository = TempRepo::new();
        let allowed = process(10, "vite");
        let engine = PreflightEngine::new(
            MockProbe {
                bindings: vec![binding(5193, allowed.clone())],
            },
            RecordingAdapter::default(),
        );
        let request = PreflightRequest {
            repository_root: repository.0.clone(),
            required_ports: [5193].into_iter().collect(),
            process_allowlist: [allowed].into_iter().collect(),
            stop_allowlisted_conflicts: false,
        };

        let report = engine.run(&request).unwrap();

        assert_eq!(report.disposition, PreflightDisposition::DecisionRequired);
        assert!(engine.process_adapter.stopped.borrow().is_empty());
    }

    #[test]
    fn approved_exact_identity_is_stopped_once_for_multiple_ports() {
        let repository = TempRepo::new();
        let allowed = process(10, "vite");
        let engine = PreflightEngine::new(
            MockProbe {
                bindings: vec![
                    binding(5193, allowed.clone()),
                    binding(5194, allowed.clone()),
                ],
            },
            RecordingAdapter::default(),
        );
        let request = PreflightRequest {
            repository_root: repository.0.clone(),
            required_ports: [5193, 5194].into_iter().collect(),
            process_allowlist: [allowed.clone()].into_iter().collect(),
            stop_allowlisted_conflicts: true,
        };

        let report = engine.run(&request).unwrap();

        assert_eq!(
            report.disposition,
            PreflightDisposition::StoppedAllowlistedConflicts
        );
        assert_eq!(report.stopped_processes, vec![allowed.clone()]);
        assert_eq!(
            engine.process_adapter.stopped.borrow().as_slice(),
            &[allowed]
        );
        assert!(report
            .baseline
            .git_status_porcelain_v2
            .contains("src/lib.rs"));
    }

    #[test]
    fn no_conflict_is_ready_and_preserves_full_baseline() {
        let repository = TempRepo::new();
        let engine = PreflightEngine::new(
            MockProbe {
                bindings: vec![binding(9000, process(12, "other"))],
            },
            DenyProcessAdapter,
        );
        let mut request = PreflightRequest::inspect_only(repository.0.clone());
        request.required_ports.insert(5193);

        let report = engine.run(&request).unwrap();

        assert_eq!(report.disposition, PreflightDisposition::Ready);
        assert_eq!(report.baseline.port_bindings.len(), 1);
        assert_eq!(report.baseline.resources.logical_cpu_count, 8);
    }
}
