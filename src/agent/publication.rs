use std::fs;
use std::path::{Path, PathBuf};

use super::{
    Job, LedgerEvent, Manifest, RemoteProvenance, captures_dir, find_capture_by_source_hash,
    lock_capture_key, next_capture_version, now_secs, read_job, rebuild_search_index,
    record_index_degradation,
};
use crate::resource::directory_size;
use anyhow::{Context, Result, bail};

pub(super) enum AcquisitionResult<T> {
    Ready(T),
    Cancelled,
}

pub(super) enum Publication {
    Published {
        capture_id: String,
        partial: bool,
        reused: bool,
    },
    Cancelled,
}

pub(super) struct PreparedCapture {
    manifest: Manifest,
    partial: bool,
}

impl PreparedCapture {
    pub(super) const fn new(manifest: Manifest, partial: bool) -> Self {
        Self { manifest, partial }
    }

    pub(super) fn with_remote_provenance(
        mut self,
        locator: String,
        provenance: RemoteProvenance,
    ) -> Self {
        self.manifest.source.locator = locator;
        self.manifest.remote_provenance = Some(provenance);
        self
    }

    const fn set_capture_version(&mut self, capture_version: u64) {
        self.manifest.capture_version = capture_version;
    }
}

pub(super) trait Acquisition {
    fn source_hash(&self, staging: &Path) -> Result<AcquisitionResult<String>>;
    fn acquire(
        &self,
        capture: &StagedCapture,
        source_hash: &str,
    ) -> Result<AcquisitionResult<PreparedCapture>>;
}

pub(super) struct StagedCapture {
    capture_id: String,
    staging: PathBuf,
}

impl StagedCapture {
    pub(super) fn capture_id(&self) -> &str {
        &self.capture_id
    }

    pub(super) fn staging(&self) -> &Path {
        &self.staging
    }
}

pub(super) fn publish(job: &Job, acquisition: &dyn Acquisition) -> Result<Publication> {
    let capture_id = format!("capture-{}", super::unique_id());
    let captures = captures_dir()?;
    let staging = captures.join(format!(".{capture_id}"));
    fs::create_dir_all(&staging)
        .with_context(|| format!("creating Capture staging directory {}", staging.display()))?;
    let mut staging_guard = StagingGuard::new(staging.clone());
    let capture = StagedCapture {
        capture_id: capture_id.clone(),
        staging,
    };
    let source_hash = match acquisition.source_hash(capture.staging())? {
        AcquisitionResult::Ready(source_hash) => source_hash,
        AcquisitionResult::Cancelled => return Ok(Publication::Cancelled),
    };
    let capture_lock = lock_capture_key(&source_hash)?;
    if let Some(capture_id) = find_capture_by_source_hash(&source_hash)? {
        match job.policy.snapshot.duplicate_mode.as_str() {
            "reuse" => {
                drop(capture_lock);
                return Ok(Publication::Published {
                    capture_id,
                    partial: false,
                    reused: true,
                });
            }
            "fail" => bail!("Capture already exists for this Source"),
            "create" => {}
            _ => bail!("invalid Policy duplicate_mode"),
        }
    }
    let capture_version = next_capture_version(&source_hash)?;
    let mut prepared = match acquisition.acquire(&capture, &source_hash)? {
        AcquisitionResult::Ready(prepared) => prepared,
        AcquisitionResult::Cancelled => return Ok(Publication::Cancelled),
    };
    prepared.set_capture_version(capture_version);
    if is_cancelled(job)? {
        return Ok(Publication::Cancelled);
    }
    check_limits(job, capture.staging())?;
    write_metadata(job, capture.staging(), &prepared.manifest)?;
    check_limits(job, capture.staging())?;
    let final_dir = captures.join(&capture_id);
    fs::rename(capture.staging(), &final_dir)
        .with_context(|| format!("publishing Capture {capture_id}"))?;
    staging_guard.publish();
    drop(capture_lock);
    if let Err(error) = rebuild_search_index() {
        record_index_degradation(&error);
    }
    Ok(Publication::Published {
        capture_id,
        partial: prepared.partial,
        reused: false,
    })
}

fn is_cancelled(job: &Job) -> Result<bool> {
    Ok(read_job(&job.id)?.state == "cancelled")
}

fn check_limits(job: &Job, staging: &Path) -> Result<()> {
    if directory_size(staging)? > job.policy.snapshot.limits.disk_byte_limit {
        bail!(
            "Capture exceeds {}@{} disk budget",
            job.policy.id,
            job.policy.version
        );
    }
    Ok(())
}

fn write_metadata(job: &Job, staging: &Path, manifest: &Manifest) -> Result<()> {
    let manifest_data =
        serde_json::to_vec_pretty(manifest).context("serializing Capture manifest")?;
    let mut ledger_data = serde_json::to_vec(&LedgerEvent {
        event: "capture_published".to_string(),
        at: now_secs(),
        job_id: job.id.clone(),
        details: None,
    })
    .context("serializing Capture ledger")?;
    ledger_data.push(b'\n');
    let metadata_bytes = u64::try_from(manifest_data.len())
        .context("converting Capture manifest size")?
        .checked_add(u64::try_from(ledger_data.len()).context("converting Capture ledger size")?)
        .context("summing Capture metadata size")?;
    let total_bytes = directory_size(staging)?
        .checked_add(metadata_bytes)
        .context("summing Capture metadata size")?;
    if total_bytes > job.policy.snapshot.limits.disk_byte_limit {
        bail!(
            "Capture exceeds {}@{} disk budget",
            job.policy.id,
            job.policy.version
        );
    }
    fs::write(staging.join("manifest.json"), manifest_data).context("writing Capture manifest")?;
    fs::write(staging.join("ledger.jsonl"), ledger_data).context("writing Capture ledger")?;
    Ok(())
}

struct StagingGuard {
    path: PathBuf,
    published: bool,
}

impl StagingGuard {
    const fn new(path: PathBuf) -> Self {
        Self {
            path,
            published: false,
        }
    }

    const fn publish(&mut self) {
        self.published = true;
    }
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
