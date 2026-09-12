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
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::unique_id::unique_id;
use crate::{audio, frames, transcribe};

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
    extractions: Vec<Extraction>,
    #[serde(default)]
    capabilities: Vec<Capability>,
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
    locator: Locator,
}

#[derive(Debug, Serialize, Deserialize)]
struct Extraction {
    artifact_id: String,
    path: String,
    mime: String,
    sha256: String,
    size_bytes: u64,
    locator: Option<Locator>,
    provider: Provider,
    proof_artifact_id: String,
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

#[derive(Debug, Serialize, Deserialize)]
struct Locator {
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamps_secs: Option<Vec<f64>>,
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
    let source_hash = sha256_file(source)?;
    if now_secs().saturating_sub(job.created_at) > job.policy.snapshot.limits.duration_limit_secs {
        bail!("Capture exceeds safe-local@1 duration budget");
    }
    if job.policy.snapshot.duplicate_mode == "reuse"
        && let Some(capture_id) = find_capture_by_source_hash(&source_hash)?
    {
        return Ok(Publication::Published {
            capture_id,
            partial: false,
        });
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
            mime: media_mime(source).to_string(),
            sha256: sha256_file(&proof_path)?,
            size_bytes: proof_size,
            locator: Locator {
                kind: "file".to_string(),
                timestamps_secs: None,
            },
        },
        extractions: Vec::new(),
        capabilities: Vec::new(),
    };
    if is_media_source(source) {
        capture_local_media(job, &proof_path, &staging, &mut manifest);
    }
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
    Ok(Publication::Published {
        capture_id,
        partial: manifest
            .capabilities
            .iter()
            .any(|capability| capability.state == "failed"),
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

fn capture_local_media(job: &Job, proof_path: &Path, staging: &Path, manifest: &mut Manifest) {
    if let Err(error) = ensure_media_providers_allowed(&job.policy) {
        record_media_setup_failure(manifest, &error);
        return;
    }
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
    if let Err(error) = capture_transcription(proof_path, staging, manifest, &work_dir, &config) {
        let _ = fs::remove_file(staging.join("extractions").join("transcription.txt"));
        record_capability_failure(manifest, "transcription", &error);
    }
    enforce_media_limits(job, staging, manifest, "transcription");
    if let Err(error) = capture_frames(proof_path, staging, manifest, &work_dir, &config) {
        let _ = fs::remove_dir_all(staging.join("extractions").join("frames"));
        record_capability_failure(manifest, "frames", &error);
    }
    enforce_media_limits(job, staging, manifest, "frames");
    if let Err(error) = fs::remove_dir_all(&work_dir)
        .with_context(|| format!("removing media work directory {}", work_dir.display()))
    {
        manifest.capabilities.push(failed_capability(
            "media-work-cleanup",
            unresolved_provider("filesystem"),
            &error,
        ));
    }
}

fn capture_transcription(
    proof_path: &Path,
    staging: &Path,
    manifest: &mut Manifest,
    work_dir: &Path,
    config: &Config,
) -> Result<()> {
    let audio_path = work_dir.join("audio.wav");
    let audio_parameters = json!({
        "format": "pcm_s16le",
        "sample_rate_hz": 16_000,
        "channels": 1,
    });
    let audio_provider = match provider("ffmpeg", audio_parameters, &[]) {
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
    match audio::extract_audio(proof_path, &audio_path) {
        Ok(()) => {
            manifest.capabilities.push(Capability {
                name: "audio-extraction".to_string(),
                state: "succeeded".to_string(),
                provider: audio_provider.clone(),
                error: None,
            });
            let transcription_provider = match transcription_parameters(config, &audio_provider)
                .and_then(|parameters| provider("whisper-cli", parameters, &[]))
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
            match transcribe::transcribe(
                &config.model_path(),
                &audio_path,
                &config.language,
                u32::try_from(config.threads).context("converting transcription thread count")?,
                &work_dir.join("transcription"),
            ) {
                Ok(transcription_path) => {
                    let destination = staging.join("extractions").join("transcription.txt");
                    copy_extraction(&transcription_path, &destination)?;
                    manifest.extractions.push(extraction(
                        "extraction-transcription",
                        "extractions/transcription.txt",
                        "text/plain",
                        &destination,
                        None,
                        transcription_provider.clone(),
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

fn capture_frames(
    proof_path: &Path,
    staging: &Path,
    manifest: &mut Manifest,
    work_dir: &Path,
    config: &Config,
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
    match frames::extract_frames(proof_path, work_dir, &frames_dir, frame_params) {
        Ok(frame_count) => {
            let destination = staging.join("extractions").join("frames");
            if frame_count > 0 {
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
                    let filename = frame.file_name().context("reading Frame filename")?;
                    let filename = filename.to_string_lossy();
                    let timestamp = frame_timestamp(&filename)?;
                    let extraction_path = destination.join(filename.as_ref());
                    copy_extraction(frame, &extraction_path)?;
                    let artifact_id = format!("extraction-frame-{index:04}");
                    let path = format!("extractions/frames/{filename}");
                    manifest.extractions.push(extraction(
                        &artifact_id,
                        &path,
                        "image/jpeg",
                        &extraction_path,
                        Some(Locator {
                            kind: "media-timestamp".to_string(),
                            timestamps_secs: Some(vec![timestamp]),
                        }),
                        frames_provider.clone(),
                    )?);
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

fn ensure_media_providers_allowed(policy: &Policy) -> Result<()> {
    for provider in ["ffmpeg", "ffprobe", "whisper-cli"] {
        if !policy
            .snapshot
            .allowed_providers
            .iter()
            .any(|allowed| allowed == provider)
        {
            bail!("Provider `{provider}` is not allowed by Policy");
        }
    }
    Ok(())
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

fn enforce_media_limits(job: &Job, staging: &Path, manifest: &mut Manifest, capability: &str) {
    let result = if now_secs().saturating_sub(job.created_at)
        > job.policy.snapshot.limits.duration_limit_secs
    {
        Err(anyhow::anyhow!(
            "Capture exceeds safe-local@1 duration budget"
        ))
    } else {
        directory_size(staging).and_then(|size| {
            if size > job.policy.snapshot.limits.disk_byte_limit {
                bail!("Capture exceeds safe-local@1 disk budget");
            }
            Ok(())
        })
    };
    if let Err(error) = result {
        manifest.extractions.retain(|extraction| {
            let affected = match capability {
                "transcription" => extraction.artifact_id == "extraction-transcription",
                "frames" => extraction.artifact_id.starts_with("extraction-frame-"),
                _ => false,
            };
            !affected
        });
        let path = if capability == "frames" {
            staging.join("extractions").join("frames")
        } else {
            staging.join("extractions").join("transcription.txt")
        };
        let _ = if capability == "frames" {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
        record_capability_failure(manifest, capability, &error);
    }
}

fn directory_size(path: &Path) -> Result<u64> {
    if path.is_file() {
        return file_size(path);
    }
    fs::read_dir(path)
        .with_context(|| format!("reading Capture directory {}", path.display()))?
        .try_fold(0_u64, |size, entry| {
            let entry = entry.context("reading Capture entry")?;
            size.checked_add(directory_size(&entry.path())?)
                .context("summing Capture disk usage")
        })
}

fn copy_extraction(source: &Path, destination: &Path) -> Result<()> {
    let parent = destination
        .parent()
        .context("resolving Extraction directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("creating Extraction directory {}", parent.display()))?;
    fs::copy(source, destination).with_context(|| {
        format!(
            "copying Extraction {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

fn extraction(
    artifact_id: &str,
    path: &str,
    mime: &str,
    file: &Path,
    locator: Option<Locator>,
    provider: Provider,
) -> Result<Extraction> {
    Ok(Extraction {
        artifact_id: artifact_id.to_string(),
        path: path.to_string(),
        mime: mime.to_string(),
        sha256: sha256_file(file)?,
        size_bytes: file_size(file)?,
        locator,
        provider,
        proof_artifact_id: "proof-source".to_string(),
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

fn transcription_parameters(config: &Config, audio_provider: &Provider) -> Result<Value> {
    let model_path = config.model_path();
    Ok(json!({
        "language": config.language,
        "threads": config.threads,
        "model": {
            "path": model_path,
            "sha256": sha256_file(&model_path)?,
        },
        "input": {
            "proof_artifact_id": "proof-source",
            "audio_extraction_provider": audio_provider,
        },
    }))
}

fn provider(name: &str, parameters: Value, dependencies: &[&str]) -> Result<Provider> {
    let path = provider_path(name)?;
    let dependencies = dependencies
        .iter()
        .map(|dependency| {
            let path = provider_path(dependency)?;
            Ok(ProviderDependency {
                name: (*dependency).to_string(),
                version: format!("sha256:{}", sha256_file(&path)?),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Provider {
        name: name.to_string(),
        version: format!("sha256:{}", sha256_file(&path)?),
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
        allowed_providers: vec![
            "ffmpeg".to_string(),
            "ffprobe".to_string(),
            "whisper-cli".to_string(),
        ],
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
