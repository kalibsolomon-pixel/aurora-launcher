//! Non-secret launch readiness and process-state vocabulary.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchInstanceStatus {
    Ready,
    Missing,
    Installing,
    Damaged,
}

impl LaunchInstanceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Missing => "missing",
            Self::Installing => "installing",
            Self::Damaged => "damaged",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchRuntimeStatus {
    Ready,
    Missing,
    Damaged,
    Unresolved,
}

impl LaunchRuntimeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Missing => "missing",
            Self::Damaged => "damaged",
            Self::Unresolved => "unresolved",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchAccountStatus {
    Ready,
    Missing,
    ReauthenticationRequired,
    ConfigurationMissing,
}

impl LaunchAccountStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Missing => "missing",
            Self::ReauthenticationRequired => "reauthenticationRequired",
            Self::ConfigurationMissing => "configurationMissing",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchProcessStatus {
    Stopped,
    Starting,
    Running,
    Exited,
    Failed,
}

impl LaunchProcessStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Failed => "failed",
        }
    }

    pub fn blocks_launch(self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchBlocker {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayReadiness {
    pub instance: LaunchInstanceStatus,
    pub runtime: LaunchRuntimeStatus,
    pub account: LaunchAccountStatus,
    pub process: LaunchProcessStatus,
    pub blockers: Vec<LaunchBlocker>,
}

impl PlayReadiness {
    pub fn evaluate(
        instance: LaunchInstanceStatus,
        runtime: LaunchRuntimeStatus,
        account: LaunchAccountStatus,
        process: LaunchProcessStatus,
    ) -> Self {
        let mut blockers = Vec::new();
        match instance {
            LaunchInstanceStatus::Ready => {}
            LaunchInstanceStatus::Missing => blockers.push(LaunchBlocker {
                code: "launch_instance_not_ready",
                message: "The selected instance does not exist.".to_owned(),
            }),
            LaunchInstanceStatus::Installing => blockers.push(LaunchBlocker {
                code: "launch_instance_not_ready",
                message: "The selected instance has not finished installing.".to_owned(),
            }),
            LaunchInstanceStatus::Damaged => blockers.push(LaunchBlocker {
                code: "launch_instance_damaged",
                message: "The selected instance failed complete content validation.".to_owned(),
            }),
        }
        match runtime {
            LaunchRuntimeStatus::Ready => {}
            LaunchRuntimeStatus::Missing => blockers.push(LaunchBlocker {
                code: "launch_runtime_not_ready",
                message: "The required managed Java runtime is not installed.".to_owned(),
            }),
            LaunchRuntimeStatus::Damaged => blockers.push(LaunchBlocker {
                code: "launch_runtime_not_ready",
                message: "The required managed Java runtime is damaged.".to_owned(),
            }),
            LaunchRuntimeStatus::Unresolved => blockers.push(LaunchBlocker {
                code: "launch_runtime_not_ready",
                message: "The required managed Java runtime could not be resolved.".to_owned(),
            }),
        }
        match account {
            LaunchAccountStatus::Ready => {}
            LaunchAccountStatus::Missing => blockers.push(LaunchBlocker {
                code: "launch_authentication_required",
                message: "Select a Minecraft account before playing.".to_owned(),
            }),
            LaunchAccountStatus::ReauthenticationRequired => blockers.push(LaunchBlocker {
                code: "launch_authentication_required",
                message: "The selected Minecraft account must sign in again.".to_owned(),
            }),
            LaunchAccountStatus::ConfigurationMissing => blockers.push(LaunchBlocker {
                code: "launch_authentication_required",
                message: "Aurora's Microsoft application registration is not configured."
                    .to_owned(),
            }),
        }
        if process.blocks_launch() {
            blockers.push(LaunchBlocker {
                code: "launch_already_running",
                message: "This instance is already starting or running.".to_owned(),
            });
        }
        Self {
            instance,
            runtime,
            account,
            process,
            blockers,
        }
    }

    pub fn ready(&self) -> bool {
        self.blockers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_launch_precondition_is_rust_owned_and_explicit() {
        let ready = PlayReadiness::evaluate(
            LaunchInstanceStatus::Ready,
            LaunchRuntimeStatus::Ready,
            LaunchAccountStatus::Ready,
            LaunchProcessStatus::Stopped,
        );
        assert!(ready.ready());

        for (instance, runtime, account, process, code) in [
            (
                LaunchInstanceStatus::Missing,
                LaunchRuntimeStatus::Ready,
                LaunchAccountStatus::Ready,
                LaunchProcessStatus::Stopped,
                "launch_instance_not_ready",
            ),
            (
                LaunchInstanceStatus::Installing,
                LaunchRuntimeStatus::Ready,
                LaunchAccountStatus::Ready,
                LaunchProcessStatus::Stopped,
                "launch_instance_not_ready",
            ),
            (
                LaunchInstanceStatus::Damaged,
                LaunchRuntimeStatus::Ready,
                LaunchAccountStatus::Ready,
                LaunchProcessStatus::Stopped,
                "launch_instance_damaged",
            ),
            (
                LaunchInstanceStatus::Ready,
                LaunchRuntimeStatus::Missing,
                LaunchAccountStatus::Ready,
                LaunchProcessStatus::Stopped,
                "launch_runtime_not_ready",
            ),
            (
                LaunchInstanceStatus::Ready,
                LaunchRuntimeStatus::Damaged,
                LaunchAccountStatus::Ready,
                LaunchProcessStatus::Stopped,
                "launch_runtime_not_ready",
            ),
            (
                LaunchInstanceStatus::Ready,
                LaunchRuntimeStatus::Ready,
                LaunchAccountStatus::Missing,
                LaunchProcessStatus::Stopped,
                "launch_authentication_required",
            ),
            (
                LaunchInstanceStatus::Ready,
                LaunchRuntimeStatus::Ready,
                LaunchAccountStatus::ReauthenticationRequired,
                LaunchProcessStatus::Stopped,
                "launch_authentication_required",
            ),
            (
                LaunchInstanceStatus::Ready,
                LaunchRuntimeStatus::Ready,
                LaunchAccountStatus::ConfigurationMissing,
                LaunchProcessStatus::Stopped,
                "launch_authentication_required",
            ),
            (
                LaunchInstanceStatus::Ready,
                LaunchRuntimeStatus::Ready,
                LaunchAccountStatus::Ready,
                LaunchProcessStatus::Running,
                "launch_already_running",
            ),
        ] {
            let result = PlayReadiness::evaluate(instance, runtime, account, process);
            assert!(!result.ready());
            assert!(result.blockers.iter().any(|blocker| blocker.code == code));
        }
    }
}
