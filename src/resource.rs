use std::fs;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Budget partagé par les Providers d'une Capture. Chaque process est isolé
/// dans son groupe afin que son arrêt emporte les éventuels enfants qu'il a
/// lancés.
pub struct CaptureBudget<'a> {
    root: &'a Path,
    deadline_secs: u64,
    disk_byte_limit: u64,
}

impl<'a> CaptureBudget<'a> {
    pub const fn new(
        root: &'a Path,
        created_at: u64,
        duration_limit_secs: u64,
        disk_byte_limit: u64,
    ) -> Self {
        Self {
            root,
            deadline_secs: created_at.saturating_add(duration_limit_secs),
            disk_byte_limit,
        }
    }

    pub fn check(&self) -> Result<()> {
        if now_secs() >= self.deadline_secs {
            bail!("Capture exceeds safe-local@1 duration budget");
        }
        if directory_size(self.root)? > self.disk_byte_limit {
            bail!("Capture exceeds safe-local@1 disk budget");
        }
        Ok(())
    }

    pub fn check_disk_capacity(&self, additional_bytes: u64) -> Result<()> {
        self.check()?;
        let used_bytes = directory_size(self.root)?;
        let total_bytes = used_bytes
            .checked_add(additional_bytes)
            .context("summing Capture disk usage")?;
        if total_bytes > self.disk_byte_limit {
            bail!("Capture exceeds safe-local@1 disk budget");
        }
        Ok(())
    }

    pub fn output(&self, command: &mut Command) -> Result<Output> {
        self.check()?;
        command
            .process_group(0)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().context("launching Provider")?;
        let stdout = child.stdout.take().context("capturing Provider stdout")?;
        let stderr = child.stderr.take().context("capturing Provider stderr")?;
        let stdout_reader = thread::spawn(move || read_pipe(stdout));
        let stderr_reader = thread::spawn(move || read_pipe(stderr));
        loop {
            if let Some(status) = child.try_wait().context("waiting for Provider")? {
                let output = collect_output(status, stdout_reader, stderr_reader)?;
                self.check()?;
                return Ok(output);
            }
            if let Err(error) = self.check() {
                let status = terminate_process_group(&mut child)?;
                let _ = collect_output(status, stdout_reader, stderr_reader)?;
                return Err(error);
            }
            thread::sleep(POLL_INTERVAL);
        }
    }
}

fn terminate_process_group(child: &mut Child) -> Result<ExitStatus> {
    let pid = i32::try_from(child.id()).context("converting Provider process id")?;
    match killpg(Pid::from_raw(pid), Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => {}
        Err(error) => return Err(error).context("stopping budget-exhausted Provider"),
    }
    child.wait().context("reaping budget-exhausted Provider")
}

fn read_pipe(mut pipe: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn collect_output(
    status: ExitStatus,
    stdout_reader: thread::JoinHandle<std::io::Result<Vec<u8>>>,
    stderr_reader: thread::JoinHandle<std::io::Result<Vec<u8>>>,
) -> Result<Output> {
    let stdout = stdout_reader
        .join()
        .map_err(|_| anyhow!("joining Provider stdout reader"))?
        .context("reading Provider stdout")?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow!("joining Provider stderr reader"))?
        .context("reading Provider stderr")?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn directory_size(path: &Path) -> Result<u64> {
    if path.is_file() {
        return Ok(fs::metadata(path)
            .with_context(|| format!("reading Capture artifact metadata {}", path.display()))?
            .len());
    }
    fs::read_dir(path)
        .with_context(|| format!("reading Capture directory {}", path.display()))?
        .try_fold(0_u64, |size, entry| {
            let entry = entry.context("reading Capture entry")?;
            size.checked_add(directory_size(&entry.path())?)
                .context("summing Capture disk usage")
        })
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}
