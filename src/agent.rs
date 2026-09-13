use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::unique_id::unique_id;

const POLICY_NAME: &str = "safe-local@1";

#[derive(Parser)]
#[command(name = "scriptor")]
struct AgentCli {
    #[command(subcommand)]
    command: AgentCommand,
}

#[derive(Subcommand)]
enum AgentCommand {
    Capture(CaptureCommand),
    Job(JobCommand),
    #[command(name = "capture-worker", hide = true)]
    CaptureWorker {
        #[arg(long)]
        job_id: String,
    },
}

#[derive(Args)]
struct CaptureCommand {
    source: Option<PathBuf>,
    #[arg(long)]
    policy: Option<String>,
    #[command(subcommand)]
    command: Option<CaptureSubcommand>,
}

#[derive(Subcommand)]
enum CaptureSubcommand {
    #[command(alias = "get")]
    Inspect { capture_id: String },
}

#[derive(Args)]
struct JobCommand {
    #[command(subcommand)]
    command: JobSubcommand,
}

#[derive(Subcommand)]
enum JobSubcommand {
    Get {
        job_id: String,
    },
    Wait {
        job_id: String,
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
    },
    Cancel {
        job_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Policy {
    id: String,
    version: u8,
    sha256: String,
    snapshot: PolicySnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PolicySnapshot {
    duplicate_mode: String,
    limits: Limits,
    allows_remote_calls: bool,
    allowed_providers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Limits {
    #[serde(rename = "max_depth")]
    depth_limit: u8,
    #[serde(rename = "max_sources")]
    source_limit: u8,
    #[serde(rename = "max_download_bytes")]
    download_byte_limit: u64,
    #[serde(rename = "max_disk_bytes")]
    disk_byte_limit: u64,
    #[serde(rename = "max_duration_secs")]
    duration_limit_secs: u64,
    #[serde(rename = "max_concurrency")]
    concurrency_limit: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Job {
    #[serde(rename = "job_id")]
    id: String,
    state: String,
    source: String,
    policy: Policy,
    created_at: u64,
    updated_at: u64,
    worker_pid: Option<u32>,
    capture_id: Option<String>,
    error: Option<StructuredError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StructuredError {
    code: String,
    message: String,
}

#[derive(Serialize)]
struct CreatedJob<'a> {
    job: &'a Job,
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    capture_id: String,
    source: SourceIdentity,
    policy: Policy,
    published_at: u64,
    proof: Proof,
    #[serde(default)]
    artifacts: Vec<Proof>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SourceIdentity {
    locator: String,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Proof {
    artifact_id: String,
    path: String,
    mime: String,
    sha256: String,
    size_bytes: u64,
}

#[derive(Serialize)]
struct InspectedCapture {
    manifest: Manifest,
    ledger: Vec<LedgerEvent>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LedgerEvent {
    event: String,
    at: u64,
    job_id: String,
}

enum Publication {
    Published(String),
    Cancelled,
}

pub fn is_agent_command(arguments: &[OsString]) -> bool {
    arguments.get(1).is_some_and(|argument| {
        matches!(
            argument.to_str(),
            Some("capture" | "job" | "capture-worker")
        )
    })
}

pub fn run(arguments: Vec<OsString>) -> Result<()> {
    let cli = AgentCli::parse_from(arguments);
    match cli.command {
        AgentCommand::Capture(command) => run_capture_command(command),
        AgentCommand::Job(command) => run_job_command(command),
        AgentCommand::CaptureWorker { job_id } => run_worker(&job_id),
    }
}

fn run_capture_command(command: CaptureCommand) -> Result<()> {
    match command.command {
        Some(CaptureSubcommand::Inspect { capture_id }) => inspect_capture(&capture_id),
        None => create_capture_job(
            command
                .source
                .as_deref()
                .context("capture requires a local Source")?,
            command
                .policy
                .as_deref()
                .context("capture requires --policy")?,
        ),
    }
}

fn run_job_command(command: JobCommand) -> Result<()> {
    match command.command {
        JobSubcommand::Get { job_id } => print_job(&job_id),
        JobSubcommand::Wait {
            job_id,
            timeout_secs,
        } => wait_for_job(&job_id, timeout_secs),
        JobSubcommand::Cancel { job_id } => cancel_job(&job_id),
    }
}

fn create_capture_job(source: &Path, policy_name: &str) -> Result<()> {
    let policy = policy_for(policy_name)?;
    if policy.snapshot.allows_remote_calls {
        crate::binary::ensure_present("scriptor-page-renderer")
            .context("safe-web@1 requires the Nix PageRenderer runtime")?;
    }
    let source = if policy.snapshot.allows_remote_calls {
        let url = source.to_str().context("Web Source must be valid UTF-8")?;
        url.to_string()
    } else {
        let source = source
            .canonicalize()
            .with_context(|| format!("resolving local Source {}", source.display()))?;
        if !source.is_file() {
            bail!("local Source must be a regular file: {}", source.display());
        }
        source.to_string_lossy().into_owned()
    };

    let now = now_secs();
    let job = Job {
        id: format!("job-{}", unique_id()),
        state: "queued".to_string(),
        source,
        policy,
        created_at: now,
        updated_at: now,
        worker_pid: None,
        capture_id: None,
        error: None,
    };
    write_job(&job)?;
    append_job_event(&job.id, "queued")?;

    let executable = std::env::current_exe().context("resolving current executable")?;
    let worker = Command::new(executable)
        .args(["capture-worker", "--job-id", &job.id])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let worker = match worker {
        Ok(worker) => worker,
        Err(error) => {
            fail_job(&job.id, &error.into())?;
            return print_json(&CreatedJob {
                job: &read_job(&job.id)?,
            });
        }
    };
    register_worker(&job.id, worker.id())?;

    print_json(&CreatedJob {
        job: &read_job(&job.id)?,
    })
}

fn run_worker(job_id: &str) -> Result<()> {
    let Some(job) = start_job(job_id)? else {
        return Ok(());
    };

    match publish_capture(&job) {
        Ok(Publication::Published(capture_id)) => complete_job(job_id, capture_id),
        Ok(Publication::Cancelled) => Ok(()),
        Err(error) => fail_job(job_id, &error),
    }
}

fn publish_capture(job: &Job) -> Result<Publication> {
    if job.policy.snapshot.allows_remote_calls {
        return publish_web_capture(job);
    }
    let source = Path::new(&job.source);
    let source_count = 1_u8;
    let depth = 0_u8;
    let downloaded_bytes = 0_u64;
    if source_count > job.policy.snapshot.limits.source_limit
        || depth > job.policy.snapshot.limits.depth_limit
        || downloaded_bytes > job.policy.snapshot.limits.download_byte_limit
    {
        bail!("local Source exceeds safe-local@1 acquisition budget");
    }
    let metadata = fs::metadata(source)
        .with_context(|| format!("reading local Source metadata {}", source.display()))?;
    if metadata.len() > job.policy.snapshot.limits.disk_byte_limit {
        bail!("local Source exceeds safe-local@1 disk budget");
    }
    let source_hash = sha256_file(source)?;
    if now_secs().saturating_sub(job.created_at) > job.policy.snapshot.limits.duration_limit_secs {
        bail!("Capture exceeds safe-local@1 duration budget");
    }
    if job.policy.snapshot.duplicate_mode == "reuse"
        && let Some(capture_id) = find_capture_by_source_hash(&source_hash)?
    {
        return Ok(Publication::Published(capture_id));
    }

    let capture_id = format!("capture-{}", unique_id());
    let captures = captures_dir()?;
    let staging = captures.join(format!(".{capture_id}"));
    let final_dir = captures.join(&capture_id);
    fs::create_dir_all(staging.join("proofs"))
        .with_context(|| format!("creating Capture staging directory {}", staging.display()))?;
    let proof_path = staging.join("proofs").join("source");
    fs::copy(source, &proof_path)
        .with_context(|| format!("copying local Source {}", source.display()))?;
    if now_secs().saturating_sub(job.created_at) > job.policy.snapshot.limits.duration_limit_secs {
        bail!("Capture exceeds safe-local@1 duration budget");
    }
    if read_job(&job.id)?.state == "cancelled" {
        fs::remove_dir_all(&staging).context("discarding cancelled Capture staging directory")?;
        return Ok(Publication::Cancelled);
    }
    let proof_size = fs::metadata(&proof_path)
        .context("reading copied Proof metadata")?
        .len();
    let manifest = Manifest {
        capture_id: capture_id.clone(),
        source: SourceIdentity {
            locator: job.source.clone(),
            sha256: source_hash,
        },
        policy: job.policy.clone(),
        published_at: now_secs(),
        proof: Proof {
            artifact_id: "proof-source".to_string(),
            path: "proofs/source".to_string(),
            mime: "application/octet-stream".to_string(),
            sha256: sha256_file(&proof_path)?,
            size_bytes: proof_size,
        },
        artifacts: Vec::new(),
    };
    write_json(&staging.join("manifest.json"), &manifest)?;
    append_json_line(
        &staging.join("ledger.jsonl"),
        &LedgerEvent {
            event: "capture_published".to_string(),
            at: now_secs(),
            job_id: job.id.clone(),
        },
    )?;
    fs::rename(&staging, &final_dir).with_context(|| format!("publishing Capture {capture_id}"))?;
    Ok(Publication::Published(capture_id))
}

fn publish_web_capture(job: &Job) -> Result<Publication> {
    if job
        .policy
        .snapshot
        .allowed_providers
        .iter()
        .any(|provider| !provider.is_empty())
    {
        bail!("safe-web@1 does not permit remote Providers");
    }
    let capture_id = format!("capture-{}", unique_id());
    let captures = captures_dir()?;
    let staging = captures.join(format!(".{capture_id}"));
    let final_dir = captures.join(&capture_id);
    fs::create_dir_all(&staging)
        .with_context(|| format!("creating Capture staging directory {}", staging.display()))?;
    fs::create_dir_all(staging.join("proofs")).context("creating Web Proof directory")?;
    fs::create_dir_all(staging.join("extractions")).context("creating Web Extraction directory")?;
    let deadline = UNIX_EPOCH
        .checked_add(Duration::from_secs(
            job.created_at
                .saturating_add(job.policy.snapshot.limits.duration_limit_secs),
        ))
        .context("calculating safe-web Policy deadline")?;
    let renderer_capture = crate::web::capture(
        &job.source,
        &staging,
        deadline,
        job.policy.snapshot.limits.disk_byte_limit,
        job.policy.snapshot.limits.download_byte_limit,
        || Ok(read_job(&job.id)?.state == "cancelled"),
    )?;
    let provenance = match renderer_capture {
        crate::web::Capture::Completed(provenance) => provenance,
        crate::web::Capture::Cancelled => {
            fs::remove_dir_all(&staging)
                .context("discarding cancelled Capture staging directory")?;
            return Ok(Publication::Cancelled);
        }
    };
    if directory_size(&staging)? > job.policy.snapshot.limits.disk_byte_limit {
        bail!("Capture exceeds safe-web@1 disk budget");
    }
    if read_job(&job.id)?.state == "cancelled" {
        fs::remove_dir_all(&staging).context("discarding cancelled Capture staging directory")?;
        return Ok(Publication::Cancelled);
    }
    let artifacts = [
        ("proof-dom", "proofs/dom.html", "text/html"),
        ("proof-screenshot", "proofs/screenshot.png", "image/png"),
        (
            "extraction-markdown",
            "extractions/page.md",
            "text/markdown",
        ),
        ("discoveries", "discoveries.json", "application/json"),
        ("provenance", "provenance.json", "application/json"),
    ]
    .into_iter()
    .map(|(artifact_id, path, mime)| proof_for(&staging, artifact_id, path, mime))
    .collect::<Result<Vec<_>>>()?;
    let proof = artifacts
        .first()
        .cloned()
        .context("page renderer produced no DOM Proof")?;
    let manifest = Manifest {
        capture_id: capture_id.clone(),
        source: SourceIdentity {
            locator: provenance.final_url,
            sha256: sha256_bytes(job.source.as_bytes()),
        },
        policy: job.policy.clone(),
        published_at: now_secs(),
        proof,
        artifacts,
    };
    write_json(&staging.join("manifest.json"), &manifest)?;
    append_json_line(
        &staging.join("ledger.jsonl"),
        &LedgerEvent {
            event: "capture_published".to_string(),
            at: now_secs(),
            job_id: job.id.clone(),
        },
    )?;
    if directory_size(&staging)? > job.policy.snapshot.limits.disk_byte_limit {
        bail!("Capture exceeds safe-web@1 disk budget");
    }
    fs::rename(&staging, &final_dir).with_context(|| format!("publishing Capture {capture_id}"))?;
    Ok(Publication::Published(capture_id))
}

fn directory_size(path: &Path) -> Result<u64> {
    let mut total = 0_u64;
    for entry in fs::read_dir(path)
        .with_context(|| format!("reading Capture directory {}", path.display()))?
    {
        let entry = entry.context("reading Capture entry")?;
        let metadata = entry.metadata().context("reading Capture entry metadata")?;
        if metadata.is_dir() {
            total = total
                .checked_add(directory_size(&entry.path())?)
                .context("summing Capture directory size")?;
        } else if metadata.is_file() {
            total = total
                .checked_add(metadata.len())
                .context("summing Capture file size")?;
        }
    }
    Ok(total)
}

fn proof_for(staging: &Path, artifact_id: &str, path: &str, mime: &str) -> Result<Proof> {
    let file = staging.join(path);
    Ok(Proof {
        artifact_id: artifact_id.to_string(),
        path: path.to_string(),
        mime: mime.to_string(),
        sha256: sha256_file(&file)?,
        size_bytes: fs::metadata(&file)
            .with_context(|| format!("reading Proof metadata {}", file.display()))?
            .len(),
    })
}

fn start_job(job_id: &str) -> Result<Option<Job>> {
    let lock = lock_job(job_id)?;
    let mut job = read_job(job_id)?;
    if job.state == "cancelled" {
        unlock_job(&lock)?;
        return Ok(None);
    }
    if active_jobs()? > usize::from(job.policy.snapshot.limits.concurrency_limit) {
        job.state = "failed".to_string();
        job.updated_at = now_secs();
        job.error = Some(StructuredError {
            code: "concurrency_limit_exceeded".to_string(),
            message: format!(
                "{}@{} concurrency budget exceeded",
                job.policy.id, job.policy.version
            ),
        });
        write_job(&job)?;
        append_job_event(job_id, "failed")?;
        unlock_job(&lock)?;
        return Ok(None);
    }
    job.state = "running".to_string();
    job.updated_at = now_secs();
    job.worker_pid = Some(std::process::id());
    write_job(&job)?;
    append_job_event(job_id, "running")?;
    unlock_job(&lock)?;
    Ok(Some(job))
}

fn complete_job(job_id: &str, capture_id: String) -> Result<()> {
    let lock = lock_job(job_id)?;
    let mut job = read_job(job_id)?;
    if job.state != "cancelled" {
        job.state = "succeeded".to_string();
        job.updated_at = now_secs();
        job.capture_id = Some(capture_id);
        job.worker_pid = None;
        write_job(&job)?;
        append_job_event(job_id, "succeeded")?;
    }
    unlock_job(&lock)
}

fn register_worker(job_id: &str, worker_pid: u32) -> Result<()> {
    let lock = lock_job(job_id)?;
    let mut job = read_job(job_id)?;
    if job.state == "queued" {
        job.worker_pid = Some(worker_pid);
        job.updated_at = now_secs();
        write_job(&job)?;
    }
    unlock_job(&lock)
}

fn fail_job(job_id: &str, error: &anyhow::Error) -> Result<()> {
    let lock = lock_job(job_id)?;
    let mut job = read_job(job_id)?;
    if job.state == "cancelled" {
        return unlock_job(&lock);
    }
    job.state = "failed".to_string();
    job.updated_at = now_secs();
    job.worker_pid = None;
    job.error = Some(StructuredError {
        code: error_code(error),
        message: format!("{error:#}"),
    });
    write_job(&job)?;
    append_job_event(&job.id, "failed")?;
    unlock_job(&lock)
}

fn error_code(error: &anyhow::Error) -> String {
    let message = format!("{error:#}");
    [
        "web_url_scheme_refused",
        "web_url_credentials_refused",
        "web_url_host_refused",
        "web_private_target_refused",
        "web_dns_resolution_failed",
        "web_port_refused",
        "web_websocket_refused",
        "web_download_budget_exceeded",
        "web_disk_budget_exceeded",
        "web_renderer_firefox_unavailable",
        "web_renderer_chromium_unavailable",
        "web_renderer_unknown",
    ]
    .into_iter()
    .find(|code| message.contains(code))
    .unwrap_or("capture_failed")
    .to_string()
}

fn print_job(job_id: &str) -> Result<()> {
    let job = reconcile_interrupted(read_job(job_id)?)?;
    print_json(&job)
}

fn wait_for_job(job_id: &str, timeout_secs: u64) -> Result<()> {
    let deadline = SystemTime::now()
        .checked_add(Duration::from_secs(timeout_secs))
        .context("calculating Job wait deadline")?;
    loop {
        let job = reconcile_interrupted(read_job(job_id)?)?;
        if is_terminal(&job.state) {
            return print_json(&job);
        }
        if SystemTime::now() >= deadline {
            return print_json(&job);
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn cancel_job(job_id: &str) -> Result<()> {
    let lock = lock_job(job_id)?;
    let mut job = read_job(job_id)?;
    if !is_terminal(&job.state) {
        job.state = "cancelled".to_string();
        job.updated_at = now_secs();
        write_job(&job)?;
        append_job_event(job_id, "cancelled")?;
    }
    unlock_job(&lock)?;
    print_json(&job)
}

fn inspect_capture(capture_id: &str) -> Result<()> {
    validate_id(capture_id, "capture")?;
    let directory = captures_dir()?.join(capture_id);
    let manifest: Manifest = read_json(&directory.join("manifest.json"))?;
    let ledger = read_json_lines(&directory.join("ledger.jsonl"))?;
    print_json(&InspectedCapture { manifest, ledger })
}

fn reconcile_interrupted(mut job: Job) -> Result<Job> {
    if matches!(job.state.as_str(), "queued" | "running")
        && job
            .worker_pid
            .is_some_and(|pid| !Path::new("/proc").join(pid.to_string()).exists())
    {
        job.state = "interrupted".to_string();
        job.updated_at = now_secs();
        job.worker_pid = None;
        write_job(&job)?;
        append_job_event(&job.id, "interrupted")?;
    }
    Ok(job)
}

fn policy_for(name: &str) -> Result<Policy> {
    if name != POLICY_NAME && name != "safe-web@1" {
        bail!("unsupported Policy `{name}`; expected `{POLICY_NAME}` or `safe-web@1`");
    }
    let limits = Limits {
        depth_limit: 2,
        source_limit: 50,
        download_byte_limit: 2 * 1024 * 1024 * 1024,
        disk_byte_limit: 10 * 1024 * 1024 * 1024,
        duration_limit_secs: 30 * 60,
        concurrency_limit: 2,
    };
    let snapshot = PolicySnapshot {
        duplicate_mode: "reuse".to_string(),
        limits,
        allows_remote_calls: name == "safe-web@1",
        allowed_providers: Vec::new(),
    };
    let canonical_snapshot =
        serde_json::to_value(&snapshot).context("normalizing Policy snapshot")?;
    let snapshot_bytes =
        serde_json::to_vec(&canonical_snapshot).context("serializing Policy snapshot")?;
    Ok(Policy {
        id: if name == POLICY_NAME {
            "safe-local"
        } else {
            "safe-web"
        }
        .to_string(),
        version: 1,
        sha256: sha256_bytes(&snapshot_bytes),
        snapshot,
    })
}

fn repository_dir() -> Result<PathBuf> {
    let data = dirs::data_dir().context("resolving data directory")?;
    Ok(data.join("scriptor").join("v2"))
}

fn jobs_dir() -> Result<PathBuf> {
    let directory = repository_dir()?.join("jobs");
    fs::create_dir_all(&directory).with_context(|| format!("creating {}", directory.display()))?;
    Ok(directory)
}

fn captures_dir() -> Result<PathBuf> {
    let directory = repository_dir()?.join("captures");
    fs::create_dir_all(&directory).with_context(|| format!("creating {}", directory.display()))?;
    Ok(directory)
}

fn job_path(job_id: &str) -> Result<PathBuf> {
    validate_id(job_id, "job")?;
    Ok(jobs_dir()?.join(format!("{job_id}.json")))
}

fn lock_job(job_id: &str) -> Result<File> {
    validate_id(job_id, "job")?;
    let path = jobs_dir()?.join(format!("{job_id}.lock"));
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening Job lock {}", path.display()))?;
    file.lock_exclusive()
        .with_context(|| format!("locking Job {job_id}"))?;
    Ok(file)
}

fn validate_id(id: &str, prefix: &str) -> Result<()> {
    let valid = id
        .strip_prefix(&format!("{prefix}-"))
        .is_some_and(|suffix| {
            !suffix.is_empty()
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'-')
        });
    if !valid {
        bail!("invalid {prefix} identifier");
    }
    Ok(())
}

fn active_jobs() -> Result<usize> {
    let mut active: usize = 0;
    for entry in fs::read_dir(jobs_dir()?).context("listing Jobs")? {
        let entry = entry.context("reading Job directory entry")?;
        if entry
            .path()
            .extension()
            .is_none_or(|extension| extension != "json")
        {
            continue;
        }
        let job: Job = read_json(&entry.path())?;
        if matches!(job.state.as_str(), "queued" | "running") {
            active = active.checked_add(1).context("counting active Jobs")?;
        }
    }
    Ok(active)
}

fn unlock_job(file: &File) -> Result<()> {
    FileExt::unlock(file).context("unlocking Job")
}

fn write_job(job: &Job) -> Result<()> {
    write_json(&job_path(&job.id)?, job)
}

fn read_job(job_id: &str) -> Result<Job> {
    read_json(&job_path(job_id)?)
}

fn append_job_event(job_id: &str, event: &str) -> Result<()> {
    append_json_line(
        &jobs_dir()?.join(format!("{job_id}.events.jsonl")),
        &LedgerEvent {
            event: event.to_string(),
            at: now_secs(),
            job_id: job_id.to_string(),
        },
    )
}

fn find_capture_by_source_hash(source_hash: &str) -> Result<Option<String>> {
    for entry in fs::read_dir(captures_dir()?).context("listing Captures")? {
        let entry = entry.context("reading Capture directory entry")?;
        if !entry
            .file_type()
            .context("reading Capture entry type")?
            .is_dir()
        {
            continue;
        }
        let manifest_path = entry.path().join("manifest.json");
        if !manifest_path.exists() {
            continue;
        }
        let manifest: Manifest = read_json(&manifest_path)?;
        if manifest.source.sha256 == source_hash {
            return Ok(Some(manifest.capture_id));
        }
    }
    Ok(None)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let temporary = path.with_extension("tmp");
    let data = serde_json::to_vec_pretty(value).context("serializing JSON")?;
    fs::write(&temporary, data).with_context(|| format!("writing {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("publishing JSON {}", path.display()))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    serde_json::from_reader(BufReader::new(file))
        .with_context(|| format!("reading {}", path.display()))
}

fn append_json_line<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    serde_json::to_writer(&mut file, value).context("serializing ledger event")?;
    file.write_all(b"\n").context("writing ledger newline")
}

fn read_json_lines<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Vec<T>> {
    let content =
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    content
        .lines()
        .map(|line| serde_json::from_str(line).context("reading ledger event"))
        .collect()
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("reading {}", path.display()))?;
        if read == 0 {
            break;
        }
        hash.update(buffer.get(..read).context("reading hash buffer")?);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn is_terminal(state: &str) -> bool {
    matches!(
        state,
        "succeeded" | "partial" | "failed" | "cancelled" | "interrupted"
    )
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    let rendered = serde_json::to_string(value).context("serializing agent response")?;
    println!("{rendered}");
    Ok(())
}
