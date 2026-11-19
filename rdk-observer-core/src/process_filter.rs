use crate::process_worker::{ProcessId, ProcessInfo};
use regex::{RegexSet, RegexSetBuilder};
use std::collections::HashSet;
use std::sync::LazyLock;

////////////////////////////////////////////////////////////////////////////////
static RDK_PROCESS_PATTERNS: LazyLock<RegexSet> = LazyLock::new(|| {
    RegexSetBuilder::new([r"^ccsp", r"^rdk"])
        .case_insensitive(true)
        .build()
        .expect("invalid RDK process pattern")
});

////////////////////////////////////////////////////////////////////////////////
/// Filter that dictates which processes are tracked
#[derive(Debug, Default, Clone)]
pub struct ProcessFilter {
    /// Include the processes with this PID
    pub include_pids: HashSet<ProcessId>,
    /// Include processes with this command name; repeat to add more
    pub include_commands: HashSet<String>,
    /// Include RDK processes from the configured selection list
    pub include_rdk_processes: bool,
    /// Exclude kernel threads
    pub exclude_kernel_threads: bool,
    /// Exclude userspace processes
    pub exclude_userspace_processes: bool,
}

impl ProcessFilter {
    /// Returns true if process is allowed
    ///
    /// Priority goes as following (which first passes - wins, allowed by default)
    /// - PID
    /// - command
    /// - rdk process
    /// - not kernel thread
    /// - not userspace process
    pub fn filter(&self, pid: ProcessId, kernel_thread: bool, command: &str) -> bool {
        if self.include_pids.contains(&pid) {
            return true;
        }
        if self.include_commands.contains(command) {
            return true;
        }
        if self.include_rdk_processes && Self::is_rdk_process(command) {
            return true;
        }
        if self.exclude_kernel_threads && kernel_thread {
            return false;
        }
        if self.exclude_userspace_processes && !kernel_thread {
            return false;
        }
        true
    }

    /// Same as [ProcessFilter::filter] but for resolved process info
    pub fn allows(&self, process: &ProcessInfo) -> bool {
        self.filter(process.pid, process.kernel_thread, &process.command)
    }

    /// Checks if command is from rdk process
    pub fn is_rdk_process(command: &str) -> bool {
        RDK_PROCESS_PATTERNS.is_match(command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KERNEL: bool = true;
    const USERSPACE: bool = false;

    fn pid(pid: i32) -> ProcessId {
        ProcessId(pid)
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[test]
    fn default_allows_everything() {
        let filter = ProcessFilter::default();

        assert!(filter.filter(pid(1), USERSPACE, "init"));
        assert!(filter.filter(pid(2), KERNEL, "k_thread"));
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[test]
    fn includes_alone_do_not_narrow_scope() {
        let filter = ProcessFilter {
            include_pids: HashSet::from([pid(10)]),
            include_commands: HashSet::from(["app".to_string()]),
            include_rdk_processes: true,
            ..Default::default()
        };

        // everything is allowed by default, includes only override excludes
        assert!(filter.filter(pid(1), USERSPACE, "other"));
        assert!(filter.filter(pid(2), KERNEL, "kworker/0:0"));
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[test]
    fn excludes_kernel_threads() {
        let filter = ProcessFilter {
            exclude_kernel_threads: true,
            ..Default::default()
        };

        assert!(!filter.filter(pid(2), KERNEL, "k_thread"));
        assert!(filter.filter(pid(1), USERSPACE, "init"));
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[test]
    fn excludes_userspace_processes() {
        let filter = ProcessFilter {
            exclude_userspace_processes: true,
            ..Default::default()
        };

        assert!(!filter.filter(pid(1), USERSPACE, "init"));
        assert!(filter.filter(pid(2), KERNEL, "k_thread"));
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[test]
    fn included_pid_overrides_excludes() {
        let filter = ProcessFilter {
            include_pids: HashSet::from([pid(10), pid(20)]),
            exclude_kernel_threads: true,
            exclude_userspace_processes: true,
            ..Default::default()
        };

        assert!(filter.filter(pid(10), USERSPACE, "app"));
        assert!(filter.filter(pid(20), KERNEL, "kworker/0:0"));
        assert!(!filter.filter(pid(30), USERSPACE, "app"));
        assert!(!filter.filter(pid(40), KERNEL, "kworker/0:0"));
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[test]
    fn included_command_overrides_excludes_with_exact_match() {
        let filter = ProcessFilter {
            include_commands: HashSet::from(["app".to_string()]),
            exclude_kernel_threads: true,
            exclude_userspace_processes: true,
            ..Default::default()
        };

        assert!(filter.filter(pid(1), USERSPACE, "app"));
        assert!(filter.filter(pid(2), KERNEL, "app"));

        // command is matched exactly, not as a prefix or case-insensitively
        assert!(!filter.filter(pid(3), USERSPACE, "app2"));
        assert!(!filter.filter(pid(4), USERSPACE, "my-app"));
        assert!(!filter.filter(pid(5), USERSPACE, "APP"));
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[test]
    fn rdk_processes_override_excludes_by_case_insensitive_prefix() {
        let filter = ProcessFilter {
            include_rdk_processes: true,
            exclude_kernel_threads: true,
            exclude_userspace_processes: true,
            ..Default::default()
        };

        assert!(filter.filter(pid(1), USERSPACE, "CcspPandMSsp"));
        assert!(filter.filter(pid(2), USERSPACE, "ccsp-wifi"));
        assert!(filter.filter(pid(3), USERSPACE, "rdkLogger"));
        assert!(filter.filter(pid(4), USERSPACE, "RDKSHELL"));

        // patterns are anchored to the start of the command
        assert!(!filter.filter(pid(5), USERSPACE, "myrdk"));
        assert!(!filter.filter(pid(6), USERSPACE, "xccsp"));
        assert!(!filter.filter(pid(7), USERSPACE, "init"));
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[test]
    fn rdk_processes_are_not_included_when_disabled() {
        let filter = ProcessFilter {
            exclude_userspace_processes: true,
            ..Default::default()
        };

        assert!(!filter.filter(pid(1), USERSPACE, "CcspPandMSsp"));
        assert!(!filter.filter(pid(2), USERSPACE, "rdkLogger"));
    }

    ////////////////////////////////////////////////////////////////////////////////
    #[test]
    fn allows_uses_process_info() {
        let filter = ProcessFilter {
            include_commands: HashSet::from(["app".to_string()]),
            exclude_userspace_processes: true,
            ..Default::default()
        };
        let process = |command: &str, kernel_thread: bool| ProcessInfo {
            pid: pid(1),
            command: command.to_string(),
            kernel_thread,
            start_time: 1,
            exit_result: None,
        };

        assert!(filter.allows(&process("app", USERSPACE)));
        assert!(filter.allows(&process("k_thread", KERNEL)));
        assert!(!filter.allows(&process("other", USERSPACE)));
    }
}
