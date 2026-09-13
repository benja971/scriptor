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
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::resource::CaptureBudget;
use crate::unique_id::unique_id;
use crate::{audio, frames, transcribe};

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

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    capture_id: String,
    source: SourceIdentity,
    policy: Policy,
    published_at: u64,
    proof: Proof,
    #[serde(default)]
    extractions: Vec<Extraction>,
    #[serde(default)]
    capabilities: Vec<Capability>,
    #[serde(default)]
    artifacts: Vec<Proof>,
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
    locator: Locator,
    created_at: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Extraction {
    artifact_id: String,
    path: String,
    mime: String,
    sha256: String,
    size_bytes: u64,
    locator: Option<Locator>,
    #[serde(skip_serializing_if = "Option::is_none")]
    locator_provider: Option<Provider>,
    provider: Provider,
    proof_artifact_id: String,
    created_at: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Capability {
    name: String,
    state: String,
    provider: Provider,
    error: Option<StructuredError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Provider {
    name: String,
    version: String,
    parameters: Value,
    dependencies: Vec<ProviderDependency>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProviderDependency {
    name: String,
    version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum Locator {
    File,
    MediaTimestamp { timestamps_secs: Vec<f64> },
    PdfPages { first_page: u32, last_page: u32 },
    ImageRegions { regions: Vec<ImageRegion> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ImageRegion {
    left: u32,
    top: u32,
    width: u32,
    height: u32,
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
    locator: Option<Locator>,
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
    artifact: ArtifactMetadata,
    reference: Reference,
    content: ReadContent,
}

#[derive(Serialize)]
struct ArtifactMetadata {
    artifact_id: String,
    path: String,
    mime: String,
    sha256: String,
    size_bytes: u64,
    locator: Option<Locator>,
    created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<Provider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    proof_artifact_id: Option<String>,
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
    Published { capture_id: String, partial: bool },
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
        Ok(Publication::Published {
            capture_id,
            partial,
        }) => complete_job(job_id, capture_id, partial),
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
    let capture_id = format!("capture-{}", unique_id());
    let captures = captures_dir()?;
    let staging = captures.join(format!(".{capture_id}"));
    let final_dir = captures.join(&capture_id);
    fs::create_dir_all(staging.join("proofs"))
        .with_context(|| format!("creating Capture staging directory {}", staging.display()))?;
    let budget = CaptureBudget::new(
        &staging,
        job.created_at,
        job.policy.snapshot.limits.duration_limit_secs,
        job.policy.snapshot.limits.disk_byte_limit,
    );
    let source_hash = sha256_file_limited(source, &budget)?;
    if job.policy.snapshot.duplicate_mode == "reuse"
        && let Some(capture_id) = find_capture_by_source_hash(&source_hash)?
    {
        fs::remove_dir_all(&staging).context("discarding duplicate Capture staging directory")?;
        return Ok(Publication::Published {
            capture_id,
            partial: false,
        });
    }
    let proof_path = staging.join("proofs").join("source");
    copy_file_limited(source, &proof_path, &budget)
        .with_context(|| format!("copying local Source {}", source.display()))?;
    let proof_created_at = now_secs();
    if read_job(&job.id)?.state == "cancelled" {
        fs::remove_dir_all(&staging).context("discarding cancelled Capture staging directory")?;
        return Ok(Publication::Cancelled);
    }
    let proof_size = fs::metadata(&proof_path)
        .context("reading copied Proof metadata")?
        .len();
    let mut manifest = Manifest {
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
            sha256: sha256_file_limited(&proof_path, &budget)?,
            size_bytes: proof_size,
            locator: Locator::File,
            created_at: proof_created_at,
        },
        extractions: Vec::new(),
        capabilities: Vec::new(),
        artifacts: Vec::new(),
    };
    if is_media_source(source) {
        capture_local_media(job, &proof_path, &staging, &mut manifest, &budget);
    } else if is_document_source(source) {
        capture_local_document(job, &proof_path, &staging, &mut manifest, &budget);
    }
    let ledger_event = LedgerEvent {
        event: "capture_published".to_string(),
        at: now_secs(),
        job_id: job.id.clone(),
    };
    write_capture_metadata(&staging, &manifest, &ledger_event, &budget)?;
    fs::rename(&staging, &final_dir).with_context(|| format!("publishing Capture {capture_id}"))?;
    if let Err(error) = rebuild_search_index() {
        record_index_degradation(&error);
    }
    Ok(Publication::Published {
        capture_id,
        partial: manifest
            .capabilities
            .iter()
            .any(|capability| capability.state != "succeeded"),
    })
}

fn is_media_source(source: &Path) -> bool {
    source.extension().is_some_and(|extension| {
        matches!(
            extension.to_string_lossy().to_ascii_lowercase().as_str(),
            "aac"
                | "avi"
                | "flac"
                | "m4a"
                | "m4v"
                | "mkv"
                | "mov"
                | "mp3"
                | "mp4"
                | "mpeg"
                | "mpg"
                | "ogg"
                | "opus"
                | "wav"
                | "webm"
        )
    })
}

fn is_document_source(source: &Path) -> bool {
    matches!(
        mime_for_source(source),
        "application/pdf" | "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/tiff"
    )
}

fn capture_local_document(
    job: &Job,
    proof_path: &Path,
    staging: &Path,
    manifest: &mut Manifest,
    budget: &CaptureBudget<'_>,
) {
    let result = match mime_for_source(Path::new(&job.source)) {
        "application/pdf" => extract_pdf_document(job, proof_path, staging, manifest, budget),
        mime if mime.starts_with("image/") => {
            extract_image_document(job, proof_path, staging, manifest, budget)
        }
        _ => return,
    };
    if let Err(error) = result {
        let (name, provider, code, error) = *error;
        discard_document_extractions(staging, manifest);
        manifest.capabilities.push(Capability {
            name: name.to_string(),
            state: if code == "provider_not_allowed" {
                "not_attempted".to_string()
            } else {
                "failed".to_string()
            },
            provider,
            error: Some(StructuredError {
                code: code.to_string(),
                message: format!("{error:#}"),
            }),
        });
    }
}

type DocumentFailure = Box<(&'static str, Provider, &'static str, anyhow::Error)>;

fn document_failure(
    capability: &'static str,
    provider: Provider,
    code: &'static str,
    error: anyhow::Error,
) -> DocumentFailure {
    Box::new((capability, provider, code, error))
}

fn extract_pdf_document(
    job: &Job,
    proof_path: &Path,
    staging: &Path,
    manifest: &mut Manifest,
    budget: &CaptureBudget<'_>,
) -> std::result::Result<(), DocumentFailure> {
    const CAPABILITY: &str = "pdf-text-extraction";
    if !policy_allows(&job.policy, "pdftotext") {
        return Err(document_failure(
            CAPABILITY,
            unresolved_provider("pdftotext"),
            "provider_not_allowed",
            denied_provider_error("pdftotext"),
        ));
    }
    if !policy_allows(&job.policy, "pdfinfo") {
        return Err(document_failure(
            CAPABILITY,
            unresolved_provider("pdfinfo"),
            "provider_not_allowed",
            denied_provider_error("pdfinfo"),
        ));
    }
    let text_provider = document_provider("pdftotext", json!({ "arguments": ["-layout"] }), budget)
        .map_err(|error| {
            document_failure(
                CAPABILITY,
                unresolved_provider("pdftotext"),
                "extraction_failed",
                error,
            )
        })?;
    let locator_provider = document_provider("pdfinfo", Value::Null, budget).map_err(|error| {
        document_failure(
            CAPABILITY,
            unresolved_provider("pdfinfo"),
            "extraction_failed",
            error,
        )
    })?;
    let page_count = pdf_page_count(proof_path, budget).map_err(|error| {
        document_failure(
            CAPABILITY,
            locator_provider.clone(),
            "extraction_failed",
            error,
        )
    })?;
    let output_dir = staging.join("extractions");
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("creating PDF Extraction directory {}", output_dir.display()))
        .map_err(|error| {
            document_failure(
                CAPABILITY,
                text_provider.clone(),
                "extraction_failed",
                error,
            )
        })?;
    let output_path = output_dir.join("pdf-text.txt");
    let mut command = Command::new("pdftotext");
    command.args(["-layout"]).arg(proof_path).arg(&output_path);
    let output = budget.output(&mut command).map_err(|error| {
        document_failure(
            CAPABILITY,
            text_provider.clone(),
            "extraction_failed",
            error,
        )
    })?;
    provider_succeeded("pdftotext", &output).map_err(|error| {
        document_failure(
            CAPABILITY,
            text_provider.clone(),
            "extraction_failed",
            error,
        )
    })?;
    let extraction = document_extraction(
        "extraction-pdf-text",
        "extractions/pdf-text.txt",
        &output_path,
        Locator::PdfPages {
            first_page: 1,
            last_page: page_count,
        },
        Some(locator_provider),
        text_provider.clone(),
        budget,
    )
    .map_err(|error| {
        document_failure(
            CAPABILITY,
            text_provider.clone(),
            "extraction_failed",
            error,
        )
    })?;
    manifest.extractions.push(extraction);
    manifest.capabilities.push(Capability {
        name: CAPABILITY.to_string(),
        state: "succeeded".to_string(),
        provider: text_provider,
        error: None,
    });
    Ok(())
}

fn extract_image_document(
    job: &Job,
    proof_path: &Path,
    staging: &Path,
    manifest: &mut Manifest,
    budget: &CaptureBudget<'_>,
) -> std::result::Result<(), DocumentFailure> {
    const CAPABILITY: &str = "image-ocr";
    if !policy_allows(&job.policy, "tesseract") {
        return Err(document_failure(
            CAPABILITY,
            unresolved_provider("tesseract"),
            "provider_not_allowed",
            denied_provider_error("tesseract"),
        ));
    }
    let provider =
        document_provider("tesseract", json!({ "format": "tsv" }), budget).map_err(|error| {
            document_failure(
                CAPABILITY,
                unresolved_provider("tesseract"),
                "extraction_failed",
                error,
            )
        })?;
    let mut command = Command::new("tesseract");
    command.args([proof_path, Path::new("stdout"), Path::new("tsv")]);
    let output = budget.output(&mut command).map_err(|error| {
        document_failure(CAPABILITY, provider.clone(), "extraction_failed", error)
    })?;
    provider_succeeded("tesseract", &output).map_err(|error| {
        document_failure(CAPABILITY, provider.clone(), "extraction_failed", error)
    })?;
    let (text, regions) = parse_tesseract_tsv(&output.stdout).map_err(|error| {
        document_failure(CAPABILITY, provider.clone(), "extraction_failed", error)
    })?;
    let output_dir = staging.join("extractions");
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("creating OCR Extraction directory {}", output_dir.display()))
        .map_err(|error| {
            document_failure(CAPABILITY, provider.clone(), "extraction_failed", error)
        })?;
    let output_path = output_dir.join("ocr.txt");
    budget
        .check_disk_capacity(
            u64::try_from(text.len())
                .context("converting OCR output size")
                .map_err(|error| {
                    document_failure(CAPABILITY, provider.clone(), "extraction_failed", error)
                })?,
        )
        .map_err(|error| {
            document_failure(CAPABILITY, provider.clone(), "extraction_failed", error)
        })?;
    fs::write(&output_path, text)
        .with_context(|| format!("writing OCR Extraction {}", output_path.display()))
        .map_err(|error| {
            document_failure(CAPABILITY, provider.clone(), "extraction_failed", error)
        })?;
    let extraction = document_extraction(
        "extraction-image-ocr",
        "extractions/ocr.txt",
        &output_path,
        Locator::ImageRegions { regions },
        None,
        provider.clone(),
        budget,
    )
    .map_err(|error| (CAPABILITY, provider.clone(), "extraction_failed", error))?;
    manifest.extractions.push(extraction);
    manifest.capabilities.push(Capability {
        name: CAPABILITY.to_string(),
        state: "succeeded".to_string(),
        provider,
        error: None,
    });
    Ok(())
}

fn document_provider(
    name: &str,
    parameters: Value,
    budget: &CaptureBudget<'_>,
) -> Result<Provider> {
    provider(name, parameters, &[], budget)
}

fn pdf_page_count(proof_path: &Path, budget: &CaptureBudget<'_>) -> Result<u32> {
    let mut command = Command::new("pdfinfo");
    command.arg(proof_path);
    let output = budget.output(&mut command)?;
    provider_succeeded("pdfinfo", &output)?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .find_map(|line| line.strip_prefix("Pages:")?.trim().parse::<u32>().ok())
        .filter(|pages| *pages > 0)
        .context("reading PDF page count")
}

fn provider_succeeded(name: &str, output: &std::process::Output) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!("{name} failed with {}: {}", output.status, stderr.trim());
}

fn parse_tesseract_tsv(bytes: &[u8]) -> Result<(String, Vec<ImageRegion>)> {
    let tsv = std::str::from_utf8(bytes).context("reading tesseract TSV")?;
    let mut text = Vec::new();
    let mut regions = Vec::new();
    for line in tsv.lines().skip(1) {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 12 || fields.get(11).is_none_or(|value| value.trim().is_empty()) {
            continue;
        }
        let coordinate = |index: usize, name: &str| -> Result<u32> {
            fields
                .get(index)
                .context("reading tesseract TSV coordinate")?
                .parse()
                .with_context(|| format!("parsing tesseract {name}"))
        };
        regions.push(ImageRegion {
            left: coordinate(6, "left")?,
            top: coordinate(7, "top")?,
            width: coordinate(8, "width")?,
            height: coordinate(9, "height")?,
        });
        text.push(fields.get(11).context("reading tesseract text")?.trim());
    }
    Ok((text.join("\n"), regions))
}

fn document_extraction(
    artifact_id: &str,
    path: &str,
    file: &Path,
    locator: Locator,
    locator_provider: Option<Provider>,
    provider: Provider,
    budget: &CaptureBudget<'_>,
) -> Result<Extraction> {
    Ok(Extraction {
        artifact_id: artifact_id.to_string(),
        path: path.to_string(),
        mime: "text/plain; charset=utf-8".to_string(),
        sha256: sha256_file_limited(file, budget)?,
        size_bytes: file_size(file)?,
        locator: Some(locator),
        locator_provider,
        provider,
        proof_artifact_id: "proof-source".to_string(),
        created_at: now_secs(),
    })
}

fn discard_document_extractions(staging: &Path, manifest: &mut Manifest) {
    let path = staging.join("extractions");
    match fs::remove_dir_all(&path) {
        Ok(()) => manifest.extractions.retain(|extraction| {
            !matches!(
                extraction.artifact_id.as_str(),
                "extraction-pdf-text" | "extraction-image-ocr"
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => record_capability_failure(
            manifest,
            "document-extraction-cleanup",
            &anyhow::Error::from(error).context("discarding invalid document Extraction"),
        ),
    }
}

fn capture_local_media(
    job: &Job,
    proof_path: &Path,
    staging: &Path,
    manifest: &mut Manifest,
    budget: &CaptureBudget<'_>,
) {
    let config = match Config::load().context("loading configuration for local media Capture") {
        Ok(config) => config,
        Err(error) => {
            record_media_setup_failure(manifest, &error);
            return;
        }
    };
    let work_dir = staging.join("work");
    if let Err(error) = fs::create_dir_all(&work_dir)
        .with_context(|| format!("creating media work directory {}", work_dir.display()))
    {
        record_media_setup_failure(manifest, &error);
        return;
    }
    let audio_allowed = policy_allows(&job.policy, "ffmpeg");
    let transcription_allowed = audio_allowed && policy_allows(&job.policy, "whisper-cli");
    if audio_allowed {
        if let Err(error) = capture_transcription(
            proof_path,
            staging,
            manifest,
            &work_dir,
            &config,
            budget,
            transcription_allowed,
        ) {
            cleanup_media_extraction(staging, "transcription", manifest);
            record_capability_failure(manifest, "transcription", &error);
        }
    } else {
        let error = denied_provider_error("ffmpeg");
        manifest.capabilities.push(blocked_capability(
            "audio-extraction",
            unresolved_provider("ffmpeg"),
            &error,
        ));
        manifest.capabilities.push(blocked_capability(
            "transcription",
            unresolved_provider("ffmpeg"),
            &error,
        ));
    }
    if !enforce_media_limits(budget, staging, manifest, "transcription") {
        cleanup_work_dir(&work_dir, manifest);
        return;
    }
    if let Some(provider) = first_denied_provider(&job.policy, &["ffmpeg", "ffprobe"]) {
        let error = denied_provider_error(provider);
        manifest.capabilities.push(blocked_capability(
            "frames",
            unresolved_provider(provider),
            &error,
        ));
    } else if let Err(error) =
        capture_frames(proof_path, staging, manifest, &work_dir, &config, budget)
    {
        cleanup_media_extraction(staging, "frames", manifest);
        record_capability_failure(manifest, "frames", &error);
    }
    enforce_media_limits(budget, staging, manifest, "frames");
    cleanup_work_dir(&work_dir, manifest);
}

fn cleanup_work_dir(work_dir: &Path, manifest: &mut Manifest) {
    if let Err(error) = fs::remove_dir_all(work_dir)
        .with_context(|| format!("removing media work directory {}", work_dir.display()))
    {
        manifest.capabilities.push(failed_capability(
            "media-work-cleanup",
            unresolved_provider("filesystem"),
            &error,
        ));
    }
}

fn cleanup_media_extraction(staging: &Path, capability: &str, manifest: &mut Manifest) {
    let path = if capability == "frames" {
        staging.join("extractions").join("frames")
    } else {
        staging.join("extractions").join("transcription.txt")
    };
    let result = if capability == "frames" {
        fs::remove_dir_all(&path)
    } else {
        fs::remove_file(&path)
    };
    match result {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            let error = anyhow::Error::from(error)
                .context(format!("discarding invalid {capability} Extraction"));
            manifest.capabilities.push(failed_capability(
                &format!("{capability}-cleanup"),
                unresolved_provider("filesystem"),
                &error,
            ));
        }
    }
}

fn capture_transcription(
    proof_path: &Path,
    staging: &Path,
    manifest: &mut Manifest,
    work_dir: &Path,
    config: &Config,
    budget: &CaptureBudget<'_>,
    transcription_allowed: bool,
) -> Result<()> {
    let audio_path = work_dir.join("audio.wav");
    let audio_parameters = json!({
        "format": "pcm_s16le",
        "sample_rate_hz": 16_000,
        "channels": 1,
    });
    let audio_provider = match provider("ffmpeg", audio_parameters, &[], budget) {
        Ok(provider) => provider,
        Err(error) => {
            manifest.capabilities.push(failed_capability(
                "audio-extraction",
                unresolved_provider_for(&error, "ffmpeg"),
                &error,
            ));
            manifest.capabilities.push(blocked_capability(
                "transcription",
                unresolved_provider("whisper-cli"),
                &error,
            ));
            return Ok(());
        }
    };
    match audio::extract_audio_limited(proof_path, &audio_path, budget) {
        Ok(()) => {
            manifest.capabilities.push(Capability {
                name: "audio-extraction".to_string(),
                state: "succeeded".to_string(),
                provider: audio_provider.clone(),
                error: None,
            });
            if transcription_allowed {
                capture_transcription_text(
                    staging,
                    manifest,
                    &audio_path,
                    work_dir,
                    config,
                    budget,
                    &audio_provider,
                )?;
            } else {
                let error = denied_provider_error("whisper-cli");
                manifest.capabilities.push(blocked_capability(
                    "transcription",
                    unresolved_provider("whisper-cli"),
                    &error,
                ));
            }
        }
        Err(error) => {
            manifest.capabilities.push(failed_capability(
                "audio-extraction",
                audio_provider,
                &error,
            ));
            manifest.capabilities.push(blocked_capability(
                "transcription",
                unresolved_provider("whisper-cli"),
                &error,
            ));
        }
    }
    Ok(())
}

fn capture_transcription_text(
    staging: &Path,
    manifest: &mut Manifest,
    audio_path: &Path,
    work_dir: &Path,
    config: &Config,
    budget: &CaptureBudget<'_>,
    audio_provider: &Provider,
) -> Result<()> {
    let transcription_provider = match transcription_parameters(config, audio_provider, budget)
        .and_then(|parameters| provider("whisper-cli", parameters, &[], budget))
    {
        Ok(provider) => provider,
        Err(error) => {
            manifest.capabilities.push(failed_capability(
                "transcription",
                unresolved_provider("whisper-cli"),
                &error,
            ));
            return Ok(());
        }
    };
    match transcribe::transcribe_limited(
        &config.model_path(),
        audio_path,
        &config.language,
        u32::try_from(config.threads).context("converting transcription thread count")?,
        &work_dir.join("transcription"),
        budget,
    ) {
        Ok(transcription_path) => {
            let destination = staging.join("extractions").join("transcription.txt");
            copy_extraction(&transcription_path, &destination, budget)?;
            manifest.extractions.push(extraction(
                "extraction-transcription",
                "extractions/transcription.txt",
                "text/plain",
                &destination,
                None,
                transcription_provider.clone(),
                budget,
            )?);
            manifest.capabilities.push(Capability {
                name: "transcription".to_string(),
                state: "succeeded".to_string(),
                provider: transcription_provider,
                error: None,
            });
        }
        Err(error) => manifest.capabilities.push(failed_capability(
            "transcription",
            transcription_provider,
            &error,
        )),
    }
    Ok(())
}

fn capture_frames(
    proof_path: &Path,
    staging: &Path,
    manifest: &mut Manifest,
    work_dir: &Path,
    config: &Config,
    budget: &CaptureBudget<'_>,
) -> Result<()> {
    let frames_dir = work_dir.join("frames");
    let frames_provider = match provider(
        "ffmpeg",
        json!({
            "interval_secs": config.frame_interval_secs,
            "scene_threshold": config.frame_scene_threshold,
            "dedup_window_secs": config.frame_dedup_window_secs,
        }),
        &["ffprobe"],
        budget,
    ) {
        Ok(provider) => provider,
        Err(error) => {
            manifest.capabilities.push(failed_capability(
                "frames",
                unresolved_provider("ffmpeg"),
                &error,
            ));
            return Ok(());
        }
    };
    let frame_params = frames::FrameExtractionParams {
        interval_secs: config.frame_interval_secs,
        scene_threshold: config.frame_scene_threshold,
        dedup_window_secs: config.frame_dedup_window_secs,
    };
    match frames::extract_frames_limited(proof_path, work_dir, &frames_dir, frame_params, budget) {
        Ok(frame_count) => {
            let destination = staging.join("extractions").join("frames");
            if frame_count > 0 {
                budget.check()?;
                fs::create_dir_all(&destination).with_context(|| {
                    format!(
                        "creating Frames Extraction directory {}",
                        destination.display()
                    )
                })?;
                let mut frames = fs::read_dir(&frames_dir)
                    .with_context(|| format!("reading Frames {}", frames_dir.display()))?
                    .map(|entry| entry.map(|entry| entry.path()))
                    .collect::<std::io::Result<Vec<_>>>()
                    .context("reading Frame paths")?;
                frames.sort();
                for (index, frame) in frames.iter().enumerate() {
                    budget.check()?;
                    let filename = frame.file_name().context("reading Frame filename")?;
                    let filename = filename.to_string_lossy();
                    let timestamp = frame_timestamp(&filename)?;
                    let extraction_path = destination.join(filename.as_ref());
                    copy_extraction(frame, &extraction_path, budget)?;
                    budget.check()?;
                    let artifact_id = format!("extraction-frame-{index:04}");
                    let path = format!("extractions/frames/{filename}");
                    manifest.extractions.push(extraction(
                        &artifact_id,
                        &path,
                        "image/jpeg",
                        &extraction_path,
                        Some(Locator::MediaTimestamp {
                            timestamps_secs: vec![timestamp],
                        }),
                        frames_provider.clone(),
                        budget,
                    )?);
                    budget.check()?;
                }
            }
            manifest.capabilities.push(Capability {
                name: "frames".to_string(),
                state: "succeeded".to_string(),
                provider: frames_provider,
                error: None,
            });
        }
        Err(error) => {
            manifest
                .capabilities
                .push(failed_capability("frames", frames_provider, &error));
        }
    }
    Ok(())
}

fn failed_capability(name: &str, provider: Provider, error: &anyhow::Error) -> Capability {
    Capability {
        name: name.to_string(),
        state: "failed".to_string(),
        provider,
        error: Some(StructuredError {
            code: "capability_failed".to_string(),
            message: format!("{error:#}"),
        }),
    }
}

fn blocked_capability(name: &str, provider: Provider, error: &anyhow::Error) -> Capability {
    Capability {
        name: name.to_string(),
        state: "not_attempted".to_string(),
        provider,
        error: Some(StructuredError {
            code: "capability_blocked".to_string(),
            message: format!("required capability failed: {error:#}"),
        }),
    }
}

fn record_media_setup_failure(manifest: &mut Manifest, error: &anyhow::Error) {
    for capability in ["audio-extraction", "transcription", "frames"] {
        record_capability_failure(manifest, capability, error);
    }
}

fn policy_allows(policy: &Policy, provider: &str) -> bool {
    policy
        .snapshot
        .allowed_providers
        .iter()
        .any(|allowed| allowed == provider)
}

fn first_denied_provider<'a>(policy: &Policy, providers: &[&'a str]) -> Option<&'a str> {
    providers
        .iter()
        .copied()
        .find(|provider| !policy_allows(policy, provider))
}

fn denied_provider_error(provider: &str) -> anyhow::Error {
    anyhow::anyhow!("Provider `{provider}` is not allowed by Policy")
}

fn record_capability_failure(manifest: &mut Manifest, name: &str, error: &anyhow::Error) {
    if let Some(capability) = manifest
        .capabilities
        .iter_mut()
        .find(|capability| capability.name == name)
    {
        capability.state = "failed".to_string();
        capability.error = Some(StructuredError {
            code: "capability_failed".to_string(),
            message: format!("{error:#}"),
        });
    } else {
        manifest.capabilities.push(failed_capability(
            name,
            unresolved_provider("unknown"),
            error,
        ));
    }
}

fn enforce_media_limits(
    budget: &CaptureBudget<'_>,
    staging: &Path,
    manifest: &mut Manifest,
    capability: &str,
) -> bool {
    let result = budget.check();
    if let Err(error) = result {
        manifest.extractions.retain(|extraction| {
            let affected = match capability {
                "transcription" => extraction.artifact_id == "extraction-transcription",
                "frames" => extraction.artifact_id.starts_with("extraction-frame-"),
                _ => false,
            };
            !affected
        });
        cleanup_media_extraction(staging, capability, manifest);
        if manifest
            .capabilities
            .iter()
            .all(|existing| existing.name != capability || existing.state != "not_attempted")
        {
            record_capability_failure(manifest, capability, &error);
        }
        return false;
    }
    true
}

fn copy_extraction(source: &Path, destination: &Path, budget: &CaptureBudget<'_>) -> Result<()> {
    let parent = destination
        .parent()
        .context("resolving Extraction directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("creating Extraction directory {}", parent.display()))?;
    copy_file_limited(source, destination, budget).with_context(|| {
        format!(
            "copying Extraction {} to {}",
            source.display(),
            destination.display()
        )
    })
}

fn extraction(
    artifact_id: &str,
    path: &str,
    mime: &str,
    file: &Path,
    locator: Option<Locator>,
    provider: Provider,
    budget: &CaptureBudget<'_>,
) -> Result<Extraction> {
    Ok(Extraction {
        artifact_id: artifact_id.to_string(),
        path: path.to_string(),
        mime: mime.to_string(),
        sha256: sha256_file_limited(file, budget)?,
        size_bytes: file_size(file)?,
        locator,
        locator_provider: None,
        provider,
        proof_artifact_id: "proof-source".to_string(),
        created_at: now_secs(),
    })
}

fn file_size(path: &Path) -> Result<u64> {
    Ok(fs::metadata(path)
        .with_context(|| format!("reading artifact metadata {}", path.display()))?
        .len())
}

fn frame_timestamp(filename: &str) -> Result<f64> {
    filename
        .rsplit_once('-')
        .context("parsing Frame filename")?
        .1
        .strip_suffix("s.jpg")
        .context("parsing Frame timestamp suffix")?
        .parse::<f64>()
        .context("parsing Frame timestamp")
}

fn media_mime(source: &Path) -> &'static str {
    match source
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("aac") => "audio/aac",
        Some("avi") => "video/x-msvideo",
        Some("m4a") => "audio/mp4",
        Some("mkv") => "video/x-matroska",
        Some("mp4" | "m4v") => "video/mp4",
        Some("mpeg" | "mpg") => "video/mpeg",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("ogg") => "audio/ogg",
        Some("opus") => "audio/opus",
        Some("flac") => "audio/flac",
        _ => "application/octet-stream",
    }
}

fn transcription_parameters(
    config: &Config,
    audio_provider: &Provider,
    budget: &CaptureBudget<'_>,
) -> Result<Value> {
    let model_path = config.model_path();
    Ok(json!({
        "language": config.language,
        "threads": config.threads,
        "model": {
            "path": model_path,
            "sha256": sha256_file_limited(&model_path, budget)?,
        },
        "input": {
            "proof_artifact_id": "proof-source",
            "audio_extraction_provider": audio_provider,
        },
    }))
}

fn provider(
    name: &str,
    parameters: Value,
    dependencies: &[&str],
    budget: &CaptureBudget<'_>,
) -> Result<Provider> {
    let path = provider_path(name)?;
    let dependencies = dependencies
        .iter()
        .map(|dependency| {
            let path = provider_path(dependency)?;
            Ok(ProviderDependency {
                name: (*dependency).to_string(),
                version: format!("sha256:{}", sha256_file_limited(&path, budget)?),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Provider {
        name: name.to_string(),
        version: format!("sha256:{}", sha256_file_limited(&path, budget)?),
        parameters,
        dependencies,
    })
}

fn provider_path(name: &str) -> Result<PathBuf> {
    std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .map(|directory| directory.join(name))
                .find(|candidate| candidate.is_file())
        })
        .with_context(|| format!("resolving Provider binary `{name}`"))
}

fn unresolved_provider(name: &str) -> Provider {
    Provider {
        name: name.to_string(),
        version: "unresolved".to_string(),
        parameters: Value::Null,
        dependencies: Vec::new(),
    }
}

fn unresolved_provider_for(error: &anyhow::Error, fallback: &str) -> Provider {
    let name = if format!("{error:#}").contains("`ffprobe`") {
        "ffprobe"
    } else {
        fallback
    };
    unresolved_provider(name)
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
        extractions: Vec::new(),
        capabilities: Vec::new(),
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
    Ok(Publication::Published {
        capture_id,
        partial: false,
    })
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
        locator: Locator::File,
        created_at: now_secs(),
    })
}

fn start_job(job_id: &str) -> Result<Option<Job>> {
    let repository_lock = lock_jobs()?;
    let lock = lock_job(job_id)?;
    let mut job = read_job(job_id)?;
    if job.state == "cancelled" {
        unlock_job(&lock)?;
        unlock_jobs(&repository_lock)?;
        return Ok(None);
    }
    if running_jobs()? >= usize::from(job.policy.snapshot.limits.concurrency_limit) {
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
        unlock_jobs(&repository_lock)?;
        return Ok(None);
    }
    job.state = "running".to_string();
    job.updated_at = now_secs();
    job.worker_pid = Some(std::process::id());
    write_job(&job)?;
    append_job_event(job_id, "running")?;
    unlock_job(&lock)?;
    unlock_jobs(&repository_lock)?;
    Ok(Some(job))
}

fn complete_job(job_id: &str, capture_id: String, partial: bool) -> Result<()> {
    let lock = lock_job(job_id)?;
    let mut job = read_job(job_id)?;
    if job.state != "cancelled" {
        job.state = if partial {
            "partial".to_string()
        } else {
            "succeeded".to_string()
        };
        job.updated_at = now_secs();
        job.capture_id = Some(capture_id);
        job.worker_pid = None;
        write_job(&job)?;
        append_job_event(job_id, &job.state)?;
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
            extractions: Vec::new(),
            capabilities: Vec::new(),
            artifacts: Vec::new(),
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
    let artifact =
        artifact_for(&manifest, &request.artifact_id).context("unknown artifact identifier")?;
    if !is_text_mime(&artifact.mime) {
        bail!("binary artifacts cannot be read on stdout");
    }
    let path = directory.join(&artifact.path);
    if sha256_file(&path)? != artifact.sha256
        || request
            .expected_sha256
            .is_some_and(|expected| expected != artifact.sha256)
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
    let reference = reference_for_artifact(&manifest.capture_id, &artifact);
    print_json(&ReadArtifact {
        artifact,
        reference,
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
    reference_for_proof(&manifest.capture_id, &manifest.proof)
}

fn artifact_for(manifest: &Manifest, artifact_id: &str) -> Option<ArtifactMetadata> {
    if manifest.proof.artifact_id == artifact_id {
        return Some(artifact_from_proof(&manifest.proof));
    }
    if let Some(proof) = manifest
        .artifacts
        .iter()
        .find(|proof| proof.artifact_id == artifact_id)
    {
        return Some(artifact_from_proof(proof));
    }
    manifest
        .extractions
        .iter()
        .find(|extraction| extraction.artifact_id == artifact_id)
        .map(artifact_from_extraction)
}

fn artifact_from_proof(proof: &Proof) -> ArtifactMetadata {
    ArtifactMetadata {
        artifact_id: proof.artifact_id.clone(),
        path: proof.path.clone(),
        mime: proof.mime.clone(),
        sha256: proof.sha256.clone(),
        size_bytes: proof.size_bytes,
        locator: Some(proof.locator.clone()),
        created_at: proof.created_at,
        provider: None,
        proof_artifact_id: None,
    }
}

fn artifact_from_extraction(extraction: &Extraction) -> ArtifactMetadata {
    ArtifactMetadata {
        artifact_id: extraction.artifact_id.clone(),
        path: extraction.path.clone(),
        mime: extraction.mime.clone(),
        sha256: extraction.sha256.clone(),
        size_bytes: extraction.size_bytes,
        locator: extraction.locator.clone(),
        created_at: extraction.created_at,
        provider: Some(extraction.provider.clone()),
        proof_artifact_id: Some(extraction.proof_artifact_id.clone()),
    }
}

fn reference_for_proof(capture_id: &str, proof: &Proof) -> Reference {
    reference_for_artifact(capture_id, &artifact_from_proof(proof))
}

fn reference_for_artifact(capture_id: &str, artifact: &ArtifactMetadata) -> Reference {
    Reference {
        capture_id: capture_id.to_string(),
        artifact_id: artifact.artifact_id.clone(),
        sha256: artifact.sha256.clone(),
        locator: artifact.locator.clone(),
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
    match source
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("txt" | "md" | "csv" | "log") => "text/plain",
        Some("json") => "application/json",
        Some("xml") => "application/xml",
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("tif" | "tiff") => "image/tiff",
        _ if is_media_source(source) => media_mime(source),
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
        allowed_providers: vec![
            "ffmpeg".to_string(),
            "ffprobe".to_string(),
            "whisper-cli".to_string(),
            "pdftotext".to_string(),
            "pdfinfo".to_string(),
            "tesseract".to_string(),
        ],
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

fn lock_jobs() -> Result<File> {
    let path = jobs_dir()?.join("admission.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening Job admission lock {}", path.display()))?;
    file.lock_exclusive().context("locking Job admission")?;
    Ok(file)
}

fn running_jobs() -> Result<usize> {
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
        if job.state == "running" {
            active = active.checked_add(1).context("counting active Jobs")?;
        }
    }
    Ok(active)
}

fn unlock_job(file: &File) -> Result<()> {
    FileExt::unlock(file).context("unlocking Job")
}

fn unlock_jobs(file: &File) -> Result<()> {
    FileExt::unlock(file).context("unlocking Job admission")
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

fn write_capture_metadata(
    staging: &Path,
    manifest: &Manifest,
    ledger_event: &LedgerEvent,
    budget: &CaptureBudget<'_>,
) -> Result<()> {
    let manifest_data =
        serde_json::to_vec_pretty(manifest).context("serializing Capture manifest")?;
    let mut ledger_data = serde_json::to_vec(ledger_event).context("serializing Capture ledger")?;
    ledger_data.push(b'\n');
    let metadata_bytes = u64::try_from(manifest_data.len())
        .context("converting Capture manifest size")?
        .checked_add(u64::try_from(ledger_data.len()).context("converting Capture ledger size")?)
        .context("summing Capture metadata size")?;
    budget.check_disk_capacity_without_duration(metadata_bytes)?;
    fs::write(staging.join("manifest.json"), manifest_data).context("writing Capture manifest")?;
    fs::write(staging.join("ledger.jsonl"), ledger_data).context("writing Capture ledger")?;
    budget.check_disk_capacity_without_duration(0)
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

fn sha256_file_limited(path: &Path, budget: &CaptureBudget<'_>) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        budget.check()?;
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("reading {}", path.display()))?;
        if read == 0 {
            break;
        }
        hash.update(buffer.get(..read).context("reading hash buffer")?);
        budget.check()?;
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn copy_file_limited(source: &Path, destination: &Path, budget: &CaptureBudget<'_>) -> Result<()> {
    let mut input = File::open(source).with_context(|| format!("opening {}", source.display()))?;
    let mut output =
        File::create(destination).with_context(|| format!("creating {}", destination.display()))?;
    let mut buffer = [0_u8; 8192];
    loop {
        budget.check()?;
        let read = input
            .read(&mut buffer)
            .with_context(|| format!("reading {}", source.display()))?;
        if read == 0 {
            break;
        }
        output
            .write_all(buffer.get(..read).context("reading copy buffer")?)
            .with_context(|| format!("writing {}", destination.display()))?;
        budget.check()?;
    }
    output
        .sync_all()
        .with_context(|| format!("syncing {}", destination.display()))?;
    budget.check()
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
