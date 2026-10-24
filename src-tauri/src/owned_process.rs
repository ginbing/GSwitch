//! The lifetime of one GSwitch-created command and its ordinary descendants.
//! No name-based process termination and no unbounded wait in cleanup.
use std::{
    io,
    process::{Child, Command, ExitStatus},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

pub const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
static CLEANUP_UNCONFIRMED: AtomicBool = AtomicBool::new(false);

pub fn ensure_quiet() -> Result<(), String> {
    if CLEANUP_UNCONFIRMED.load(Ordering::SeqCst) {
        Err("A GSwitch helper could not be stopped safely. Close its processes and restart GSwitch before another account operation.".into())
    } else {
        Ok(())
    }
}

pub struct OwnedProcess {
    pub child: Child,
    group: platform::Group,
    cleaned: bool,
    status: Option<ExitStatus>,
}

impl OwnedProcess {
    pub fn spawn(command: &mut Command) -> io::Result<Self> {
        let group = platform::Group::prepare(command)?;
        let child = command.spawn()?;
        let mut owned = Self {
            child,
            group,
            cleaned: false,
            status: None,
        };
        if let Err(error) = owned.group.attach(&owned.child) {
            owned.cleanup(Instant::now() + CLEANUP_TIMEOUT);
            return Err(error);
        }
        Ok(owned)
    }

    /// On Unix leave the leader waitable until group cleanup, so its PID/PGID
    /// cannot be recycled underneath a later group signal.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.status.is_none() {
            self.status = platform::try_wait(&mut self.child)?;
        }
        Ok(self.status)
    }

    pub fn wait_until(&mut self, deadline: Instant) -> io::Result<Option<ExitStatus>> {
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(Some(status));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            thread::sleep(
                Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }

    pub fn cleanup(&mut self, deadline: Instant) -> bool {
        if self.cleaned {
            return true;
        }
        // Darwin may refuse a group signal after its last live member exits.
        // Confirm the resulting process state rather than requiring a signal
        // to have been delivered to an already-empty group.
        let _termination = self.group.terminate(&mut self.child);
        let result = loop {
            let exited = self.try_wait().ok().flatten().is_some();
            let quiet = self.group.quiet(self.child.id());
            if exited && quiet {
                // try_wait already established exit; this wait only reaps it.
                break self.child.wait().is_ok();
            }
            if Instant::now() >= deadline {
                #[cfg(test)]
                eprintln!("Owned helper cleanup unconfirmed: termination={_termination:?}, exited={exited}, group_quiet={quiet}");
                break false;
            }
            thread::sleep(
                Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
            );
        };
        // Do not repeat another five-second cleanup in Drop after a deadline.
        self.cleaned = true;
        if !result {
            CLEANUP_UNCONFIRMED.store(true, Ordering::SeqCst);
        }
        result
    }
}

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        if !self.cleaned {
            self.cleanup(Instant::now() + CLEANUP_TIMEOUT);
        }
    }
}

#[cfg(test)]
pub(crate) fn fixture_command(kind: &str, root: &std::path::Path) -> Command {
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .args([
            "--exact",
            "owned_process::tests::process_fixture",
            "--ignored",
            "--nocapture",
        ])
        .env("GSWITCH_PROCESS_FIXTURE", kind)
        .env("GSWITCH_PROCESS_FIXTURE_ROOT", root);
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Read, process::Stdio, sync::mpsc};

    #[test]
    #[ignore = "executed only as a scoped subprocess by lifecycle tests"]
    #[allow(clippy::zombie_processes)] // Intentionally outlives its launcher; the owner must stop it.
    fn process_fixture() {
        let Some(kind) = std::env::var_os("GSWITCH_PROCESS_FIXTURE") else {
            return;
        };
        let root = std::path::PathBuf::from(
            std::env::var_os("GSWITCH_PROCESS_FIXTURE_ROOT").expect("fixture root"),
        );
        if kind == "failure" {
            std::process::exit(23);
        }
        if kind == "success" {
            return;
        }
        if kind == "leaf" {
            fs::write(root.join("leaf-ready"), b"ready").expect("ready");
        } else {
            let child = fixture_command("leaf", &root).spawn().expect("leaf");
            fs::write(root.join("leaf-pid"), child.id().to_string()).expect("pid");
            let deadline = Instant::now() + Duration::from_secs(3);
            while !root.join("leaf-ready").exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            assert!(root.join("leaf-ready").exists());
            if kind == "exit-with-pipe" {
                return;
            }
        }
        thread::sleep(Duration::from_secs(60));
    }

    fn root() -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("gswitch-process-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).expect("fixture directory");
        root
    }

    #[test]
    fn stops_owned_descendants_without_stopping_an_unrelated_process() {
        let root = root();
        let other_root = self::root();
        let mut unrelated =
            OwnedProcess::spawn(&mut fixture_command("leaf", &other_root)).expect("unrelated");
        let mut process =
            OwnedProcess::spawn(&mut fixture_command("launcher", &root)).expect("owned launcher");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !root.join("leaf-ready").exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(root.join("leaf-ready").exists());
        assert!(process
            .wait_until(Instant::now() + Duration::from_millis(50))
            .expect("wait")
            .is_none());
        assert!(process.cleanup(Instant::now() + CLEANUP_TIMEOUT));
        assert!(unrelated.try_wait().expect("unrelated state").is_none());
        assert!(unrelated.cleanup(Instant::now() + CLEANUP_TIMEOUT));
        fs::remove_dir_all(root).expect("cleanup fixture");
        fs::remove_dir_all(other_root).expect("cleanup unrelated fixture");
    }

    #[test]
    fn an_exited_launcher_cannot_leave_a_descendant_holding_the_pipe() {
        let root = root();
        let mut command = fixture_command("exit-with-pipe", &root);
        command.stdout(Stdio::piped());
        let mut process = OwnedProcess::spawn(&mut command).expect("launcher");
        let mut stdout = process.child.stdout.take().expect("pipe");
        let (sender, receiver) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = stdout.read_to_end(&mut bytes);
            sender.send(result.is_ok()).expect("receiver");
        });
        assert!(process
            .wait_until(Instant::now() + Duration::from_secs(5))
            .expect("wait")
            .expect("exit")
            .success());
        assert!(receiver.try_recv().is_err());
        assert!(process.cleanup(Instant::now() + CLEANUP_TIMEOUT));
        assert!(receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("bounded EOF"));
        reader.join().expect("reader");
        fs::remove_dir_all(root).expect("cleanup fixture");
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    };
    use windows_sys::Win32::{
        Foundation::{HANDLE, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD,
                THREADENTRY32,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
                JobObjectExtendedLimitInformation, QueryInformationJobObject,
                SetInformationJobObject, TerminateJobObject,
                JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            },
            Threading::{
                OpenThread, ResumeThread, CREATE_NO_WINDOW, CREATE_SUSPENDED, THREAD_SUSPEND_RESUME,
            },
        },
    };

    pub struct Group {
        job: OwnedHandle,
        attached: bool,
    }
    impl Group {
        pub fn prepare(command: &mut Command) -> io::Result<Self> {
            // The primary thread must not execute before assignment: a fast
            // launcher could otherwise escape with a child before the job exists.
            command.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
            unsafe {
                let raw = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if raw.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let job = OwnedHandle::from_raw_handle(raw);
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    raw,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&limits) as u32,
                ) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(Self {
                    job,
                    attached: false,
                })
            }
        }
        fn handle(&self) -> HANDLE {
            self.job.as_raw_handle()
        }
        pub fn attach(&mut self, child: &Child) -> io::Result<()> {
            unsafe {
                if AssignProcessToJobObject(self.handle(), child.as_raw_handle()) == 0 {
                    return Err(io::Error::last_os_error());
                }
                self.attached = true;
                let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
                if snapshot == INVALID_HANDLE_VALUE {
                    return Err(io::Error::last_os_error());
                }
                let snapshot = OwnedHandle::from_raw_handle(snapshot);
                let mut entry: THREADENTRY32 = std::mem::zeroed();
                entry.dwSize = std::mem::size_of_val(&entry) as u32;
                let mut found = Thread32First(snapshot.as_raw_handle(), &mut entry);
                while found != 0 {
                    if entry.th32OwnerProcessID == child.id() {
                        // Only the still-suspended, owned child's primary thread
                        // exists. It cannot exit/reuse the ID before resumption.
                        let raw = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                        if raw.is_null() {
                            return Err(io::Error::last_os_error());
                        }
                        let thread = OwnedHandle::from_raw_handle(raw);
                        if ResumeThread(thread.as_raw_handle()) != 1 {
                            return Err(io::Error::other("Unable to resume owned helper"));
                        }
                        return Ok(());
                    }
                    found = Thread32Next(snapshot.as_raw_handle(), &mut entry);
                }
                Err(io::Error::other("Unable to find owned helper thread"))
            }
        }
        pub fn terminate(&self, child: &mut Child) -> io::Result<()> {
            if !self.attached {
                return child.kill();
            }
            if unsafe { TerminateJobObject(self.handle(), 1) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
        pub fn quiet(&self, _leader: u32) -> bool {
            if !self.attached {
                return true;
            }
            unsafe {
                let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
                QueryInformationJobObject(
                    self.handle(),
                    JobObjectBasicAccountingInformation,
                    (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    std::mem::size_of_val(&info) as u32,
                    std::ptr::null_mut(),
                ) != 0
                    && info.ActiveProcesses == 0
            }
        }
    }
    pub fn try_wait(child: &mut Child) -> io::Result<Option<ExitStatus>> {
        child.try_wait()
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use sysinfo::{ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System};

    pub struct Group;
    impl Group {
        #[allow(clippy::unnecessary_wraps)] // Matches the fallible Windows boundary.
        pub fn prepare(command: &mut Command) -> io::Result<Self> {
            command.process_group(0);
            Ok(Self)
        }
        #[allow(clippy::unnecessary_wraps)]
        pub fn attach(&mut self, _child: &Child) -> io::Result<()> {
            Ok(())
        }
        pub fn terminate(&self, child: &mut Child) -> io::Result<()> {
            // The unreaped leader keeps this PGID reserved until cleanup ends.
            if unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) } == -1
                && io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
        pub fn quiet(&self, leader: u32) -> bool {
            let mut system = System::new();
            system.refresh_processes_specifics(
                ProcessesToUpdate::All,
                true,
                ProcessRefreshKind::nothing().without_tasks(),
            );
            system.processes().iter().all(|(pid, process)| {
                pid.as_u32() == leader
                    || (unsafe { libc::getpgid(pid.as_u32() as libc::pid_t) })
                        != leader as libc::pid_t
                    || process.status() == ProcessStatus::Zombie
            })
        }
    }
    pub fn try_wait(child: &mut Child) -> io::Result<Option<ExitStatus>> {
        unsafe {
            let mut info: libc::siginfo_t = std::mem::zeroed();
            if libc::waitid(
                libc::P_PID,
                child.id(),
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            ) != 0
            {
                return Err(io::Error::last_os_error());
            }
            if info.si_pid() == 0 {
                return Ok(None);
            }
            let status = if info.si_code == libc::CLD_EXITED {
                info.si_status() << 8
            } else {
                info.si_status()
            };
            Ok(Some(ExitStatus::from_raw(status)))
        }
    }
}
