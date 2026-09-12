use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
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
    extractions: Vec<Extraction>,
    extraction_errors: Vec<ExtractionError>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SourceIdentity {
    locator: String,
    sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Proof {
    artifact_id: String,
    path: String,
    mime: String,
    sha256: String,
    size_bytes: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Extraction {
    artifact_id: String,
    path: String,
    mime: String,
    sha256: String,
    size_bytes: u64,
    provider: Provider,
    parameters: Vec<String>,
    locator_provider: Option<ProviderInvocation>,
    locator: Locator,
    input: ArtifactReference,
    created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Provider {
    id: String,
    version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProviderInvocation {
    provider: Provider,
    parameters: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ArtifactReference {
    capture_id: String,
    artifact_id: String,
    sha256: String,
    locator: Option<Locator>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ExtractionError {
    code: String,
    message: String,
    provider: Option<ProviderInvocation>,
    input: ArtifactReference,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Locator {
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

struct ExtractionOutcome {
    extractions: Vec<Extraction>,
    errors: Vec<ExtractionError>,
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
        Ok(Publication::Published {
            capture_id,
            partial,
        }) => complete_job(job_id, capture_id, partial),
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
    if now_secs().saturating_sub(job.created_at) > job.policy.snapshot.limits.duration_limit_secs {
        bail!("Capture exceeds safe-local@1 duration budget");
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
    let proof_hash = sha256_file(&proof_path)?;
    if job.policy.snapshot.duplicate_mode == "reuse"
        && let Some((capture_id, partial)) = find_capture_by_source_hash(&proof_hash)?
    {
        fs::remove_dir_all(&staging).context("discarding duplicate Capture staging directory")?;
        return Ok(Publication::Published {
            capture_id,
            partial,
        });
    }
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
    let proof_reference = ArtifactReference {
        capture_id: capture_id.clone(),
        artifact_id: "proof-source".to_string(),
        sha256: proof_hash.clone(),
        locator: None,
    };
    let extraction_outcome =
        extract_local_document(source, &proof_path, &staging, &proof_reference);
    let partial = !extraction_outcome.errors.is_empty();
    let manifest = Manifest {
        capture_id: capture_id.clone(),
        source: SourceIdentity {
            locator: job.source.clone(),
            sha256: proof_hash.clone(),
        },
        policy: job.policy.clone(),
        published_at: now_secs(),
        proof: Proof {
            artifact_id: "proof-source".to_string(),
            path: "proofs/source".to_string(),
            mime: source_mime(source).to_string(),
            sha256: proof_hash,
            size_bytes: proof_size,
        },
        extractions: extraction_outcome.extractions,
        extraction_errors: extraction_outcome.errors,
    };
    write_json(&staging.join("manifest.json"), &manifest)?;
    append_json_line(
        &staging.join("ledger.jsonl"),
        &LedgerEvent {
            event: if partial {
                "capture_published_partial".to_string()
            } else {
                "capture_published".to_string()
            },
            at: now_secs(),
            job_id: job.id.clone(),
        },
    )?;
    fs::rename(&staging, &final_dir).with_context(|| format!("publishing Capture {capture_id}"))?;
    Ok(Publication::Published {
        capture_id,
        partial,
    })
}

fn extract_local_document(
    source: &Path,
    proof: &Path,
    staging: &Path,
    input: &ArtifactReference,
) -> ExtractionOutcome {
    match source_mime(source) {
        "application/pdf" => extract_pdf(proof, staging, input),
        mime if mime.starts_with("image/") => extract_image(proof, staging, input),
        _ => ExtractionOutcome {
            extractions: Vec::new(),
            errors: Vec::new(),
        },
    }
}

fn extract_pdf(proof: &Path, staging: &Path, input: &ArtifactReference) -> ExtractionOutcome {
    let parameters = vec!["-layout".to_string()];
    let provider = match provider_version("pdftotext", "-v") {
        Ok(version) => Provider {
            id: "pdftotext".to_string(),
            version,
        },
        Err(error) => return extraction_failure(&error, None, input),
    };
    let provider_invocation = ProviderInvocation {
        provider: provider.clone(),
        parameters: parameters.clone(),
    };
    let locator_provider = match provider_version("pdfinfo", "-v") {
        Ok(version) => ProviderInvocation {
            provider: Provider {
                id: "pdfinfo".to_string(),
                version,
            },
            parameters: Vec::new(),
        },
        Err(error) => return extraction_failure(&error, Some(provider_invocation), input),
    };
    let page_count = match pdf_page_count(proof) {
        Ok(page_count) => page_count,
        Err(error) => return extraction_failure(&error, Some(locator_provider), input),
    };
    match persist_pdf_extraction(
        proof,
        staging,
        provider,
        parameters,
        locator_provider,
        page_count,
        input,
    ) {
        Ok(extraction) => extraction_success(extraction),
        Err(error) => extraction_failure(&error, Some(provider_invocation), input),
    }
}

fn persist_pdf_extraction(
    proof: &Path,
    staging: &Path,
    provider: Provider,
    parameters: Vec<String>,
    locator_provider: ProviderInvocation,
    page_count: u32,
    input: &ArtifactReference,
) -> Result<Extraction> {
    let directory = staging.join("extractions");
    fs::create_dir_all(&directory)
        .with_context(|| format!("creating Extraction directory {}", directory.display()))?;
    let output = directory.join("pdf-text.txt");
    run_provider("pdftotext", &["-layout"], proof, &output)?;
    let size_bytes = fs::metadata(&output)
        .context("reading PDF Extraction metadata")?
        .len();
    Ok(Extraction {
        artifact_id: "extraction-pdf-text".to_string(),
        path: "extractions/pdf-text.txt".to_string(),
        mime: "text/plain; charset=utf-8".to_string(),
        sha256: sha256_file(&output)?,
        size_bytes,
        provider,
        parameters,
        locator_provider: Some(locator_provider),
        locator: Locator::PdfPages {
            first_page: 1,
            last_page: page_count,
        },
        input: input.clone(),
        created_at: now_secs(),
    })
}

fn extract_image(proof: &Path, staging: &Path, input: &ArtifactReference) -> ExtractionOutcome {
    let parameters = vec!["tsv".to_string()];
    let provider = match provider_version("tesseract", "--version") {
        Ok(version) => Provider {
            id: "tesseract".to_string(),
            version,
        },
        Err(error) => return extraction_failure(&error, None, input),
    };
    let provider_invocation = ProviderInvocation {
        provider: provider.clone(),
        parameters: parameters.clone(),
    };
    match persist_image_extraction(proof, staging, provider, parameters, input) {
        Ok(extraction) => extraction_success(extraction),
        Err(error) => extraction_failure(&error, Some(provider_invocation), input),
    }
}

fn persist_image_extraction(
    proof: &Path,
    staging: &Path,
    provider: Provider,
    parameters: Vec<String>,
    input: &ArtifactReference,
) -> Result<Extraction> {
    let output = Command::new("tesseract")
        .args([proof, Path::new("stdout"), Path::new("tsv")])
        .output()
        .context("running tesseract")?;
    if !output.status.success() {
        bail!("{}", provider_command_error("tesseract", &output));
    }
    let (text, regions) = parse_tesseract_tsv(&output.stdout)?;
    let directory = staging.join("extractions");
    fs::create_dir_all(&directory)
        .with_context(|| format!("creating Extraction directory {}", directory.display()))?;
    let path = directory.join("ocr.txt");
    fs::write(&path, text).with_context(|| format!("writing OCR Extraction {}", path.display()))?;
    let size_bytes = fs::metadata(&path)
        .context("reading OCR Extraction metadata")?
        .len();
    Ok(Extraction {
        artifact_id: "extraction-image-ocr".to_string(),
        path: "extractions/ocr.txt".to_string(),
        mime: "text/plain; charset=utf-8".to_string(),
        sha256: sha256_file(&path)?,
        size_bytes,
        provider,
        parameters,
        locator_provider: None,
        locator: Locator::ImageRegions { regions },
        input: input.clone(),
        created_at: now_secs(),
    })
}

fn extraction_success(extraction: Extraction) -> ExtractionOutcome {
    ExtractionOutcome {
        extractions: vec![extraction],
        errors: Vec::new(),
    }
}

fn extraction_failure(
    error: &anyhow::Error,
    provider: Option<ProviderInvocation>,
    input: &ArtifactReference,
) -> ExtractionOutcome {
    ExtractionOutcome {
        extractions: Vec::new(),
        errors: vec![ExtractionError {
            code: "extraction_failed".to_string(),
            message: format!("{error:#}"),
            provider,
            input: input.clone(),
        }],
    }
}

fn provider_version(binary: &str, version_argument: &str) -> Result<String> {
    let output = Command::new(binary)
        .arg(version_argument)
        .output()
        .with_context(|| format!("running {binary} version command"))?;
    if !output.status.success() {
        bail!("{}", provider_command_error(binary, &output));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    text.lines()
        .chain(stderr.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
        .context("reading Provider version")
}

fn pdf_page_count(proof: &Path) -> Result<u32> {
    let output = Command::new("pdfinfo")
        .arg(proof)
        .output()
        .context("running pdfinfo")?;
    if !output.status.success() {
        bail!("{}", provider_command_error("pdfinfo", &output));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .find_map(|line| line.strip_prefix("Pages:")?.trim().parse::<u32>().ok())
        .filter(|pages| *pages > 0)
        .context("reading PDF page count")
}

fn run_provider(binary: &str, parameters: &[&str], input: &Path, output: &Path) -> Result<()> {
    let status = Command::new(binary)
        .args(parameters)
        .arg(input)
        .arg(output)
        .status()
        .with_context(|| format!("running {binary}"))?;
    if !status.success() {
        bail!("{binary} failed with {status}");
    }
    Ok(())
}

fn provider_command_error(binary: &str, output: &Output) -> anyhow::Error {
    let stderr = String::from_utf8_lossy(&output.stderr);
    anyhow::anyhow!("{binary} failed with {}: {}", output.status, stderr.trim())
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
        let region = ImageRegion {
            left: parse_tsv_coordinate(&fields, 6, "left")?,
            top: parse_tsv_coordinate(&fields, 7, "top")?,
            width: parse_tsv_coordinate(&fields, 8, "width")?,
            height: parse_tsv_coordinate(&fields, 9, "height")?,
        };
        text.push(fields.get(11).context("reading tesseract text")?.trim());
        regions.push(region);
    }
    Ok((text.join("\n"), regions))
}

fn parse_tsv_coordinate(fields: &[&str], index: usize, name: &str) -> Result<u32> {
    fields
        .get(index)
        .context("reading tesseract TSV coordinate")?
        .parse()
        .with_context(|| format!("parsing tesseract {name}"))
}

fn source_mime(source: &Path) -> &'static str {
    match source
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("tif" | "tiff") => "image/tiff",
        _ => "application/octet-stream",
    }
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

fn find_capture_by_source_hash(source_hash: &str) -> Result<Option<(String, bool)>> {
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
            return Ok(Some((
                manifest.capture_id,
                !manifest.extraction_errors.is_empty(),
            )));
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
