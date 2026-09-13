use std::fs;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde_json::json;

use super::{
    Discovery, Extraction, Job, LedgerEvent, Locator, Manifest, Proof, Provider, Publication,
    Reference, RenderedDiscovery, SourceIdentity, StagingGuard, append_json_line, captures_dir,
    directory_size, find_capture_by_source_hash, lock_capture_key, now_secs, read_job, read_json,
    rebuild_search_index, record_index_degradation, sha256_bytes, sha256_file, unique_id,
    write_json,
};

pub(super) fn publish(job: &Job) -> Result<Publication> {
    validate_policy(job)?;
    let source_hash = sha256_bytes(job.source.as_bytes());
    let _capture_lock = lock_capture_key(&source_hash)?;
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
    fs::create_dir_all(&staging)
        .with_context(|| format!("creating Capture staging directory {}", staging.display()))?;
    fs::create_dir_all(staging.join("proofs")).context("creating Web Proof directory")?;
    fs::create_dir_all(staging.join("extractions")).context("creating Web Extraction directory")?;
    let mut staging_guard = StagingGuard::new(staging.clone());
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
        crate::web::Capture::Cancelled => return Ok(Publication::Cancelled),
    };
    if directory_size(&staging)? > job.policy.snapshot.limits.disk_byte_limit {
        bail!("Capture exceeds safe-web@1 disk budget");
    }
    if read_job(&job.id)?.state == "cancelled" {
        return Ok(Publication::Cancelled);
    }
    let artifacts = [
        ("proof-screenshot", "proofs/screenshot.png", "image/png"),
        ("discoveries", "discoveries.json", "application/json"),
        ("provenance", "provenance.json", "application/json"),
    ]
    .into_iter()
    .map(|(artifact_id, path, mime)| proof_for(&staging, artifact_id, path, mime))
    .collect::<Result<Vec<_>>>()?;
    let proof = proof_for(&staging, "proof-dom", "proofs/dom.html", "text/html")?;
    let extraction = markdown_extraction(&staging, &proof, &provenance.final_url)?;
    let discoveries = discoveries(&staging, &capture_id, &proof)?;
    let manifest = Manifest {
        capture_id: capture_id.clone(),
        source: SourceIdentity {
            locator: provenance.final_url,
            sha256: source_hash,
        },
        policy: job.policy.clone(),
        published_at: now_secs(),
        proof,
        extractions: vec![extraction],
        capabilities: Vec::new(),
        artifacts,
        discoveries,
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
    staging_guard.publish();
    if let Err(error) = rebuild_search_index() {
        record_index_degradation(&error);
    }
    Ok(Publication::Published {
        capture_id,
        partial: false,
    })
}

fn validate_policy(job: &Job) -> Result<()> {
    if job
        .policy
        .snapshot
        .allowed_providers
        .iter()
        .any(|provider| !provider.is_empty())
    {
        bail!("safe-web@1 does not permit remote Providers");
    }
    Ok(())
}

fn markdown_extraction(staging: &Path, proof: &Proof, final_url: &str) -> Result<Extraction> {
    let path = staging.join("extractions/page.md");
    Ok(Extraction {
        artifact_id: "extraction-markdown".to_string(),
        path: "extractions/page.md".to_string(),
        mime: "text/markdown".to_string(),
        sha256: sha256_file(&path)?,
        size_bytes: fs::metadata(&path)
            .context("reading Markdown Extraction metadata")?
            .len(),
        locator: Some(Locator::Url {
            value: final_url.to_string(),
        }),
        locator_provider: None,
        provider: Provider {
            name: "page-renderer".to_string(),
            version: "1".to_string(),
            parameters: json!({ "browser": "firefox" }),
            dependencies: Vec::new(),
        },
        proof_artifact_id: proof.artifact_id.clone(),
        created_at: now_secs(),
    })
}

fn discoveries(staging: &Path, capture_id: &str, proof: &Proof) -> Result<Vec<Discovery>> {
    let rendered: Vec<RenderedDiscovery> = read_json(&staging.join("discoveries.json"))?;
    Ok(rendered
        .into_iter()
        .map(|discovery| Discovery {
            source: discovery.url,
            parent: Reference {
                capture_id: capture_id.to_string(),
                artifact_id: proof.artifact_id.clone(),
                sha256: proof.sha256.clone(),
                locator: Some(discovery.parent_locator),
            },
            locator: discovery.locator,
            order: discovery.order,
            status: discovery.status,
            reason: discovery.reason,
        })
        .collect())
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
