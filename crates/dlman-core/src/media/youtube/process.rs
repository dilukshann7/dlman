//! Own yt-dlp and descendants as a Windows job or Unix process group.
use crate::DlmanError;
fn process_error(message: &str) -> DlmanError {
    DlmanError::InvalidOperation(message.into())
}
use std::{path::Path, process::Stdio};
use tokio::process::{Child, Command};

pub(crate) struct ProcessTree {
    pub child: Child,
    #[cfg(windows)]
    job: isize,
    #[cfg(unix)]
    group: i32,
}

impl ProcessTree {
    pub fn spawn(executable: &Path, args: &[String]) -> Result<Self, DlmanError> {
        let mut command = Command::new(executable);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(
            windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
                | windows_sys::Win32::System::Threading::CREATE_SUSPENDED,
        );
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.as_std_mut().process_group(0);
        }
        let mut child = command
            .spawn()
            .map_err(|_| process_error("Cannot start the bundled media tool."))?;
        #[cfg(windows)]
        {
            let pid = child
                .id()
                .ok_or_else(|| process_error("Cannot identify the media process."))?;
            match windows_job::attach_and_resume(pid) {
                Ok(job) => Ok(Self { child, job }),
                Err(_) => {
                    let _ = child.start_kill();
                    Err(process_error("Cannot contain the media process tree."))
                }
            }
        }
        #[cfg(not(windows))]
        {
            let group = child
                .id()
                .ok_or_else(|| process_error("Cannot identify media process."))?
                as i32;
            Ok(Self { child, group })
        }
    }

    pub fn terminate(&mut self) {
        #[cfg(unix)]
        if self.group > 0 {
            unsafe {
                libc::kill(-self.group, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job as _, 1);
        }
        let _ = self.child.start_kill();
    }

    pub async fn stop(&mut self) -> Result<(), DlmanError> {
        self.terminate();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            self.child.wait().await.map_err(|_| stop_error())?;
            #[cfg(windows)]
            loop {
                if windows_job::empty(self.job).map_err(|_| stop_error())? {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            #[cfg(unix)]
            loop {
                let result = unsafe { libc::kill(-self.group, 0) };
                if result == -1
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                {
                    break;
                }
                if result == -1 {
                    return Err(stop_error());
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            Ok::<(), DlmanError>(())
        })
        .await
        .map_err(|_| stop_error())??;
        #[cfg(unix)]
        {
            self.group = 0;
        }
        Ok(())
    }
}

fn stop_error() -> DlmanError {
    process_error("The media process tree did not stop cleanly. Its staging files were preserved.")
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        self.terminate();
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job as _);
        }
    }
}

#[cfg(windows)]
mod windows_job {
    use std::{
        io,
        mem::{size_of, zeroed},
        ptr::null,
    };
    use windows_sys::Win32::{
        Foundation::*,
        System::{Diagnostics::ToolHelp::*, JobObjects::*, Threading::*},
    };

    pub fn empty(job: isize) -> io::Result<bool> {
        active(job).map(|count| count == 0)
    }

    pub fn active(job: isize) -> io::Result<u32> {
        unsafe {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = zeroed();
            if QueryInformationJobObject(
                job as _,
                JobObjectBasicAccountingInformation,
                &mut info as *mut _ as _,
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(info.ActiveProcesses)
        }
    }

    pub fn attach_and_resume(pid: u32) -> io::Result<isize> {
        // The process is suspended until it belongs to a kill-on-close job.
        unsafe {
            let job = CreateJobObjectW(null(), null());
            if job.is_null() {
                return Err(io::Error::last_os_error());
            }
            let result = (|| {
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as _,
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                if process.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let assigned = AssignProcessToJobObject(job, process);
                let assign_error = io::Error::last_os_error();
                CloseHandle(process);
                if assigned == 0 {
                    return Err(assign_error);
                }
                let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
                if snapshot == INVALID_HANDLE_VALUE {
                    return Err(io::Error::last_os_error());
                }
                let mut entry: THREADENTRY32 = zeroed();
                entry.dwSize = size_of::<THREADENTRY32>() as u32;
                let mut found = false;
                let mut has_entry = Thread32First(snapshot, &mut entry) != 0;
                while has_entry {
                    if entry.th32OwnerProcessID == pid {
                        let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                        if !thread.is_null() {
                            found = ResumeThread(thread) != u32::MAX;
                            CloseHandle(thread);
                        }
                        break;
                    }
                    has_entry = Thread32Next(snapshot, &mut entry) != 0;
                }
                CloseHandle(snapshot);
                if !found {
                    return Err(io::Error::other("Cannot resume managed media process."));
                }
                Ok(())
            })();
            if let Err(error) = result {
                TerminateJobObject(job, 1);
                CloseHandle(job);
                return Err(error);
            }
            Ok(job as isize)
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    #[ignore = "subprocess fixture used by the process containment test"]
    fn leaf_fixture() {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }

    #[test]
    #[ignore = "subprocess fixture used by the process containment test"]
    fn tree_fixture() {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "media::youtube::process::tests::leaf_fixture",
                "--ignored",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let _ = child.wait();
    }

    #[tokio::test]
    async fn stop_reaps_descendants_before_a_paused_process_can_restart() {
        let executable = std::env::current_exe().unwrap();
        let args = vec![
            "--exact".into(),
            "media::youtube::process::tests::tree_fixture".into(),
            "--ignored".into(),
        ];
        let mut process = ProcessTree::spawn(&executable, &args).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while windows_job::active(process.job).unwrap() < 2 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        process.stop().await.unwrap();
        assert!(process.child.try_wait().unwrap().is_some());
        assert!(windows_job::empty(process.job).unwrap());
        let mut replacement = ProcessTree::spawn(&executable, &args).unwrap();
        replacement.stop().await.unwrap();
        assert!(windows_job::empty(replacement.job).unwrap());
    }
}
