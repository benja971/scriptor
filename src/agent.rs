use std::ffi::OsString;
use std::fmt::Write as FmtWrite;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
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
const DEFAULT_PAGE_LIMIT: usize = 20;
const MAX_PAGE_LIMIT: usize = 100;
const DEFAULT_READ_LENGTH: usize = 8 * 1024;
const MAX_READ_LENGTH: usize = 1024 * 1024;

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
    Inspect {
        capture_id: String,
    },
    List {
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long, default_value_t = DEFAULT_PAGE_LIMIT)]
        limit: usize,
    },
    Search {
        query: String,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long, default_value_t = DEFAULT_PAGE_LIMIT)]
        limit: usize,
    },
    Read {
        capture_id: Option<String>,
        artifact_id: Option<String>,
        #[arg(long, conflicts_with_all = ["capture_id", "artifact_id"])]
        reference: Option<String>,
        #[arg(long, default_value_t = 0)]
        offset: u64,
        #[arg(long)]
        length: Option<usize>,
    },
    Index(IndexCommand),
}

#[derive(Args)]
struct IndexCommand {
    #[command(subcommand)]
    command: IndexSubcommand,
}

#[derive(Subcommand)]
enum IndexSubcommand {
    Rebuild,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    capture_id: String,
    source: SourceIdentity,
    policy: Policy,
    published_at: u64,
    proof: Proof,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Clone, Deserialize, Serialize)]
struct Reference {
    capture_id: String,
    artifact_id: String,
    sha256: String,
    locator: Option<String>,
}

#[derive(Serialize)]
struct CaptureSummary {
    capture_id: String,
    source: SourceIdentity,
    published_at: u64,
    reference: Reference,
}

#[derive(Serialize)]
struct CapturePage {
    captures: Vec<CaptureSummary>,
    next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct SearchIndex {
    version: u8,
    captures: Vec<IndexedCapture>,
}

#[derive(Serialize, Deserialize)]
struct IndexedCapture {
    capture_id: String,
    source: SourceIdentity,
    policy: Policy,
    published_at: u64,
    proof: Proof,
    search_text: String,
}

#[derive(Deserialize, Serialize)]
struct AgentError {
    error: StructuredError,
}

#[derive(Serialize)]
struct ReadArtifact {
    artifact: Proof,
    reference: Reference,
    content: ReadContent,
}

#[derive(Serialize)]
struct ReadContent {
    offset: u64,
    offset_unit: &'static str,
    length: usize,
    text: String,
    truncated: bool,
}

#[derive(Deserialize, Serialize)]
struct Cursor {
    version: u8,
    operation: String,
    query: Option<String>,
    snapshot: String,
    index_version: Option<u8>,
    capture_id: String,
}

struct CursorBinding {
    operation: &'static str,
    query: Option<String>,
    snapshot: String,
    index_version: Option<u8>,
}

#[derive(Serialize)]
struct RebuiltSearchIndex {
    captures: usize,
}

struct ReadRequest {
    capture_id: String,
    artifact_id: String,
    expected_sha256: Option<String>,
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
    let cli = match AgentCli::try_parse_from(arguments) {
        Ok(cli) => cli,
        Err(error) => return print_agent_error("invalid_command", error.to_string()),
    };
    let result = match cli.command {
        AgentCommand::Capture(command) => run_capture_command(command),
        AgentCommand::Job(command) => run_job_command(command),
        AgentCommand::CaptureWorker { job_id } => run_worker(&job_id),
    };
    match result {
        Ok(()) => Ok(()),
        Err(error) => print_json(&AgentError {
            error: agent_error(&error),
        }),
    }
}

fn print_agent_error(code: &str, message: String) -> Result<()> {
    print_json(&AgentError {
        error: StructuredError {
            code: code.to_string(),
            message,
        },
    })
}

fn agent_error(error: &anyhow::Error) -> StructuredError {
    let message = format!("{error:#}");
    let code = if message.contains("pagination cursor") {
        "invalid_cursor"
    } else if message.contains("requires --policy") {
        "policy_required"
    } else if message.contains("limit must") {
        "invalid_pagination"
    } else if message.contains("length must") || message.contains("UTF-8 character boundary") {
        "invalid_range"
    } else if message.contains("binary artifacts") {
        "binary_artifact"
    } else if message.contains("unknown artifact") {
        "artifact_not_found"
    } else if message.contains("does not match its Reference") {
        "reference_mismatch"
    } else if message.contains("invalid Reference") {
        "invalid_reference"
    } else if message.contains("invalid capture identifier")
        || message.contains("invalid job identifier")
    {
        "invalid_identifier"
    } else if message.contains("manifest.json") {
        "capture_not_found"
    } else if message.contains(".json") && message.contains("opening") {
        "job_not_found"
    } else {
        "agent_command_failed"
    };
    StructuredError {
        code: code.to_string(),
        message,
    }
}

fn run_capture_command(command: CaptureCommand) -> Result<()> {
    match command.command {
        Some(CaptureSubcommand::Inspect { capture_id }) => inspect_capture(&capture_id),
        Some(CaptureSubcommand::List { cursor, limit }) => list_captures(cursor.as_deref(), limit),
        Some(CaptureSubcommand::Search {
            query,
            cursor,
            limit,
        }) => search_captures(&query, cursor.as_deref(), limit),
        Some(CaptureSubcommand::Read {
            capture_id,
            artifact_id,
            reference,
            offset,
            length,
        }) => read_artifact(
            capture_id.as_deref(),
            artifact_id.as_deref(),
            reference.as_deref(),
            offset,
            length,
        ),
        Some(CaptureSubcommand::Index(command)) => run_index_command(&command),
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
    let policy = safe_local_policy(policy_name)?;
    let source = source
        .canonicalize()
        .with_context(|| format!("resolving local Source {}", source.display()))?;
    if !source.is_file() {
        bail!("local Source must be a regular file: {}", source.display());
    }

    let now = now_secs();
    let job = Job {
        id: format!("job-{}", unique_id()),
        state: "queued".to_string(),
        source: source.to_string_lossy().into_owned(),
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
            mime: mime_for_source(source).to_string(),
            sha256: sha256_file(&proof_path)?,
            size_bytes: proof_size,
        },
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
    if let Err(error) = rebuild_search_index() {
        record_index_degradation(&error);
    }
    Ok(Publication::Published(capture_id))
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
            message: "safe-local@1 concurrency budget exceeded".to_string(),
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
        code: "capture_failed".to_string(),
        message: format!("{error:#}"),
    });
    write_job(&job)?;
    append_job_event(&job.id, "failed")?;
    unlock_job(&lock)
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

fn run_index_command(command: &IndexCommand) -> Result<()> {
    match &command.command {
        IndexSubcommand::Rebuild => print_json(&RebuiltSearchIndex {
            captures: rebuild_search_index()?,
        }),
    }
}

fn list_captures(cursor: Option<&str>, limit: usize) -> Result<()> {
    let manifests = list_manifests()?;
    let binding = CursorBinding {
        operation: "list",
        query: None,
        snapshot: snapshot_for(&manifests)?,
        index_version: None,
    };
    print_json(&capture_page(manifests, cursor, limit, &binding)?)
}

fn search_captures(query: &str, cursor: Option<&str>, limit: usize) -> Result<()> {
    validate_page_limit(limit)?;
    if let Some(error) = read_index_degradation()? {
        return print_json(&AgentError { error });
    }
    let index: SearchIndex = match read_json(&search_index_path()?) {
        Ok(index) => index,
        Err(error) => {
            return print_json(&AgentError {
                error: StructuredError {
                    code: "index_unavailable".to_string(),
                    message: format!("search Index is unavailable: {error:#}"),
                },
            });
        }
    };
    if index.version != 1 {
        return print_json(&AgentError {
            error: StructuredError {
                code: "index_unavailable".to_string(),
                message: "search Index version is unsupported".to_string(),
            },
        });
    }
    let normalized_query = query.to_lowercase();
    let binding = CursorBinding {
        operation: "search",
        query: Some(normalized_query.clone()),
        snapshot: sha256_bytes(
            &serde_json::to_vec(&index).context("serializing search Index snapshot")?,
        ),
        index_version: Some(index.version),
    };
    let manifests = index
        .captures
        .into_iter()
        .filter(|capture| {
            capture
                .source
                .locator
                .to_lowercase()
                .contains(&normalized_query)
                || capture
                    .search_text
                    .to_lowercase()
                    .contains(&normalized_query)
        })
        .map(|capture| Manifest {
            capture_id: capture.capture_id,
            source: capture.source,
            policy: capture.policy,
            published_at: capture.published_at,
            proof: capture.proof,
        })
        .collect();
    print_json(&capture_page(manifests, cursor, limit, &binding)?)
}

fn read_artifact(
    capture_id: Option<&str>,
    artifact_id: Option<&str>,
    reference: Option<&str>,
    offset: u64,
    requested_length: Option<usize>,
) -> Result<()> {
    let request = resolve_read_request(capture_id, artifact_id, reference)?;
    validate_id(&request.capture_id, "capture")?;
    let length = requested_length.unwrap_or(DEFAULT_READ_LENGTH);
    if length > MAX_READ_LENGTH {
        bail!("length must not exceed {MAX_READ_LENGTH}");
    }
    let directory = captures_dir()?.join(&request.capture_id);
    let manifest: Manifest = read_json(&directory.join("manifest.json"))?;
    if request.artifact_id != manifest.proof.artifact_id {
        bail!("unknown artifact identifier");
    }
    if !is_text_mime(&manifest.proof.mime) {
        bail!("binary artifacts cannot be read on stdout");
    }
    let path = directory.join(&manifest.proof.path);
    if sha256_file(&path)? != manifest.proof.sha256
        || request
            .expected_sha256
            .is_some_and(|expected| expected != manifest.proof.sha256)
    {
        bail!("artifact hash does not match its Reference");
    }
    let size = fs::metadata(&path)
        .with_context(|| format!("reading artifact metadata {}", path.display()))?
        .len();
    let remaining = size.saturating_sub(offset);
    let amount =
        usize::try_from(remaining.min(u64::try_from(length).context("converting read length")?))
            .context("converting bounded artifact length")?;
    let mut file =
        File::open(&path).with_context(|| format!("opening artifact {}", path.display()))?;
    if offset < size {
        file.seek(SeekFrom::Start(offset))
            .with_context(|| format!("checking artifact offset {}", path.display()))?;
        let mut current = [0_u8; 1];
        file.read_exact(&mut current)
            .with_context(|| format!("checking artifact offset {}", path.display()))?;
        if current.first().context("reading artifact offset byte")? & 0b1100_0000 == 0b1000_0000 {
            bail!("offset must align to a UTF-8 character boundary");
        }
    }
    file.seek(SeekFrom::Start(offset))
        .with_context(|| format!("seeking artifact {}", path.display()))?;
    let mut bytes = vec![0; amount];
    file.read_exact(&mut bytes)
        .with_context(|| format!("reading artifact {}", path.display()))?;
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) if error.utf8_error().error_len().is_none() => {
            let valid = error.utf8_error().valid_up_to();
            let bytes = error.into_bytes();
            String::from_utf8(
                bytes
                    .get(..valid)
                    .context("trimming bounded artifact text")?
                    .to_vec(),
            )
            .context("trimming bounded artifact text")?
        }
        Err(error) => return Err(error).context("artifact text is not valid UTF-8"),
    };
    let text_length = text.len();
    print_json(&ReadArtifact {
        artifact: manifest.proof.clone(),
        reference: reference_for(&manifest),
        content: ReadContent {
            offset,
            offset_unit: "bytes",
            length: text_length,
            text,
            truncated: u64::try_from(text_length).context("converting read result length")?
                < remaining,
        },
    })
}

fn resolve_read_request(
    capture_id: Option<&str>,
    artifact_id: Option<&str>,
    reference: Option<&str>,
) -> Result<ReadRequest> {
    if let Some(reference) = reference {
        let reference: Reference =
            serde_json::from_str(reference).context("invalid Reference JSON")?;
        return Ok(ReadRequest {
            capture_id: reference.capture_id,
            artifact_id: reference.artifact_id,
            expected_sha256: Some(reference.sha256),
        });
    }
    Ok(ReadRequest {
        capture_id: capture_id
            .context("invalid Reference: capture_id is required")?
            .to_string(),
        artifact_id: artifact_id
            .context("invalid Reference: artifact_id is required")?
            .to_string(),
        expected_sha256: None,
    })
}

fn capture_page(
    manifests: Vec<Manifest>,
    cursor: Option<&str>,
    limit: usize,
    binding: &CursorBinding,
) -> Result<CapturePage> {
    validate_page_limit(limit)?;
    let after = cursor.map(decode_cursor).transpose()?;
    if let Some(cursor) = &after
        && (cursor.version != 1
            || cursor.operation != binding.operation
            || cursor.query != binding.query
            || cursor.snapshot != binding.snapshot
            || cursor.index_version != binding.index_version)
    {
        bail!("pagination cursor does not match this operation or snapshot");
    }
    let start = after.as_ref().map_or(0, |cursor| {
        let capture_id = cursor.capture_id.as_str();
        manifests.partition_point(|manifest| manifest.capture_id.as_str() <= capture_id)
    });
    let mut captures = manifests
        .into_iter()
        .skip(start)
        .take(limit.saturating_add(1))
        .collect::<Vec<_>>();
    let has_next = captures.len() > limit;
    if has_next {
        captures.pop();
    }
    let next_cursor = if has_next {
        captures
            .last()
            .map(|manifest| {
                encode_cursor(&Cursor {
                    version: 1,
                    operation: binding.operation.to_string(),
                    query: binding.query.clone(),
                    snapshot: binding.snapshot.clone(),
                    index_version: binding.index_version,
                    capture_id: manifest.capture_id.clone(),
                })
            })
            .transpose()?
    } else {
        None
    };
    Ok(CapturePage {
        captures: captures.iter().map(summary_for).collect(),
        next_cursor,
    })
}

fn validate_page_limit(limit: usize) -> Result<()> {
    if limit == 0 {
        bail!("limit must be at least 1");
    }
    if limit > MAX_PAGE_LIMIT {
        bail!("limit must not exceed {MAX_PAGE_LIMIT}");
    }
    Ok(())
}

fn list_manifests() -> Result<Vec<Manifest>> {
    let mut manifests: Vec<Manifest> = Vec::new();
    for entry in fs::read_dir(captures_dir()?).context("listing Captures")? {
        let entry = entry.context("reading Capture directory entry")?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if !entry
            .file_type()
            .context("reading Capture entry type")?
            .is_dir()
        {
            continue;
        }
        let manifest_path = entry.path().join("manifest.json");
        if manifest_path
            .try_exists()
            .with_context(|| format!("checking Capture manifest {}", manifest_path.display()))?
        {
            manifests.push(read_json(&manifest_path)?);
        }
    }
    manifests.sort_unstable_by(|left, right| left.capture_id.cmp(&right.capture_id));
    Ok(manifests)
}

fn rebuild_search_index() -> Result<usize> {
    let lock = lock_search_index()?;
    let captures: Vec<IndexedCapture> = list_manifests()?
        .into_iter()
        .map(|manifest| {
            let search_text = search_text_for(&manifest)?;
            Ok(IndexedCapture {
                capture_id: manifest.capture_id,
                source: manifest.source,
                policy: manifest.policy,
                published_at: manifest.published_at,
                proof: manifest.proof,
                search_text,
            })
        })
        .collect::<Result<_>>()?;
    let count = captures.len();
    write_search_index(&SearchIndex {
        version: 1,
        captures,
    })?;
    clear_index_degradation()?;
    unlock_search_index(&lock)?;
    Ok(count)
}

fn search_index_path() -> Result<PathBuf> {
    Ok(repository_dir()?.join("index.json"))
}

fn lock_search_index() -> Result<File> {
    let directory = repository_dir()?;
    fs::create_dir_all(&directory)
        .with_context(|| format!("creating search Index directory {}", directory.display()))?;
    let path = directory.join("index.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening search Index lock {}", path.display()))?;
    file.lock_exclusive().context("locking search Index")?;
    Ok(file)
}

fn unlock_search_index(file: &File) -> Result<()> {
    FileExt::unlock(file).context("unlocking search Index")
}

fn write_search_index(index: &SearchIndex) -> Result<()> {
    let path = search_index_path()?;
    let temporary = path.with_file_name(format!(".index-{}.tmp", unique_id()));
    let data = serde_json::to_vec_pretty(index).context("serializing search Index")?;
    fs::write(&temporary, data)
        .with_context(|| format!("writing search Index {}", temporary.display()))?;
    fs::rename(&temporary, &path)
        .with_context(|| format!("publishing search Index {}", path.display()))
}

fn record_index_degradation(error: &anyhow::Error) {
    let result = (|| -> Result<()> {
        let directory = repository_dir()?;
        fs::create_dir_all(&directory)
            .with_context(|| format!("creating Index status directory {}", directory.display()))?;
        write_json(
            &index_status_path()?,
            &AgentError {
                error: StructuredError {
                    code: "index_degraded".to_string(),
                    message: format!("{error:#}"),
                },
            },
        )
    })();
    drop(result);
}

fn clear_index_degradation() -> Result<()> {
    let path = index_status_path()?;
    if path
        .try_exists()
        .with_context(|| format!("checking Index status {}", path.display()))?
    {
        fs::remove_file(&path)
            .with_context(|| format!("clearing Index status {}", path.display()))?;
    }
    Ok(())
}

fn read_index_degradation() -> Result<Option<StructuredError>> {
    let path = index_status_path()?;
    if !path
        .try_exists()
        .with_context(|| format!("checking Index status {}", path.display()))?
    {
        return Ok(None);
    }
    let status: AgentError = read_json(&path)?;
    Ok(Some(status.error))
}

fn index_status_path() -> Result<PathBuf> {
    Ok(repository_dir()?.join("index-status.json"))
}

fn snapshot_for(manifests: &[Manifest]) -> Result<String> {
    let data = serde_json::to_vec(manifests).context("serializing Capture snapshot")?;
    Ok(sha256_bytes(&data))
}

fn search_text_for(manifest: &Manifest) -> Result<String> {
    if !is_text_mime(&manifest.proof.mime) {
        return Ok(String::new());
    }
    let path = captures_dir()?
        .join(&manifest.capture_id)
        .join(&manifest.proof.path);
    let file =
        File::open(&path).with_context(|| format!("opening text artifact {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(u64::try_from(MAX_READ_LENGTH).context("converting Index text limit")?)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading text artifact {}", path.display()))?;
    Ok(String::from_utf8(bytes).unwrap_or_default())
}

fn summary_for(manifest: &Manifest) -> CaptureSummary {
    CaptureSummary {
        capture_id: manifest.capture_id.clone(),
        source: manifest.source.clone(),
        published_at: manifest.published_at,
        reference: reference_for(manifest),
    }
}

fn reference_for(manifest: &Manifest) -> Reference {
    Reference {
        capture_id: manifest.capture_id.clone(),
        artifact_id: manifest.proof.artifact_id.clone(),
        sha256: manifest.proof.sha256.clone(),
        locator: None,
    }
}

fn encode_cursor(cursor: &Cursor) -> Result<String> {
    let encoded = serde_json::to_vec(cursor).context("serializing pagination cursor")?;
    let mut hex = String::with_capacity(encoded.len().saturating_mul(2));
    for byte in encoded {
        FmtWrite::write_fmt(&mut hex, format_args!("{byte:02x}"))
            .context("encoding pagination cursor")?;
    }
    Ok(format!("v1-{hex}"))
}

fn decode_cursor(cursor: &str) -> Result<Cursor> {
    let encoded = cursor
        .strip_prefix("v1-")
        .context("invalid pagination cursor")?;
    if encoded.is_empty() || encoded.len() % 2 != 0 {
        bail!("invalid pagination cursor");
    }
    let bytes = encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .context("invalid pagination cursor")
                .and_then(|hex| u8::from_str_radix(hex, 16).context("invalid pagination cursor"))
        })
        .collect::<Result<Vec<_>>>()?;
    serde_json::from_slice(&bytes).context("invalid pagination cursor")
}

fn is_text_mime(mime: &str) -> bool {
    mime.starts_with("text/") || matches!(mime, "application/json" | "application/xml")
}

fn mime_for_source(source: &Path) -> &'static str {
    match source.extension().and_then(|extension| extension.to_str()) {
        Some("txt" | "md" | "csv" | "log") => "text/plain",
        Some("json") => "application/json",
        Some("xml") => "application/xml",
        _ => "application/octet-stream",
    }
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

fn safe_local_policy(name: &str) -> Result<Policy> {
    if name != POLICY_NAME {
        bail!("unsupported Policy `{name}`; expected `{POLICY_NAME}`");
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
        allows_remote_calls: false,
        allowed_providers: Vec::new(),
    };
    let canonical_snapshot =
        serde_json::to_value(&snapshot).context("normalizing Policy snapshot")?;
    let snapshot_bytes =
        serde_json::to_vec(&canonical_snapshot).context("serializing Policy snapshot")?;
    Ok(Policy {
        id: "safe-local".to_string(),
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
