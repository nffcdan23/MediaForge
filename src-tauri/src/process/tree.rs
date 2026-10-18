//! Keeps track of a helper and everything it starts, so the whole tree can be stopped together.
//!
//! yt-dlp starts FFmpeg and a JavaScript runtime of its own. Stopping only yt-dlp would leave those running,
//! so on macOS and Linux every helper leads its own process group, and on Windows it runs inside a Job
//! Object that is closed (killing every member) when MediaForge lets go of it, even if MediaForge crashes.

use tokio::process::{Child, Command};

pub fn configure(command: &mut Command) {
    #[cfg(unix)]
    {
        command.process_group(0);
        // Leave the foreground to the app: helpers run at a lower scheduling priority.
        unsafe {
            command.pre_exec(|| {
                libc::setpriority(libc::PRIO_PROCESS, 0, 5);
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW};
        // No console window flashes up for each tool, and the tools don't compete with the UI.
        command.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
    }
}

#[cfg(unix)]
pub struct ProcessTree {
    pid: i32,
    is_group_leader: bool,
}

#[cfg(unix)]
impl ProcessTree {
    pub fn attach(child: &Child) -> std::io::Result<Self> {
        let pid = child.id().map(|id| id as i32).unwrap_or(0);
        // Verify rather than assume, and never signal our own group.
        let is_group_leader = pid > 1 && unsafe { libc::getpgid(pid) == pid && pid != libc::getpgrp() };
        Ok(Self { pid, is_group_leader })
    }

    /// Asks every process in the tree to stop.
    pub fn terminate(&self) {
        self.signal(libc::SIGTERM);
    }

    /// Stops every process in the tree immediately.
    pub fn kill(&self) {
        self.signal(libc::SIGKILL);
    }

    fn signal(&self, signal: i32) {
        if self.pid <= 1 {
            return;
        }
        unsafe {
            if self.is_group_leader {
                libc::kill(-self.pid, signal);
            } else {
                libc::kill(self.pid, signal);
            }
        }
    }
}

#[cfg(windows)]
pub struct ProcessTree {
    job: windows_sys::Win32::Foundation::HANDLE,
}

// The handle is only ever used through thread-safe Win32 calls.
#[cfg(windows)]
unsafe impl Send for ProcessTree {}
#[cfg(windows)]
unsafe impl Sync for ProcessTree {}

#[cfg(windows)]
impl ProcessTree {
    pub fn attach(child: &Child) -> std::io::Result<Self> {
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectExtendedLimitInformation, SetInformationJobObject,
        };
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let tree = Self { job };
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            let Some(handle) = child.raw_handle() else {
                return Err(std::io::Error::other("the process has already exited"));
            };
            if AssignProcessToJobObject(job, handle as _) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(tree)
        }
    }

    /// Windows has no polite "please stop" for console tools without a console, so this is immediate.
    /// Partial output lives in the job's private folder, which is removed afterwards anyway.
    pub fn terminate(&self) {
        self.kill();
    }

    pub fn kill(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        // Closing the last handle kills anything still in the job: nothing a helper started outlives it.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job);
        }
    }
}
