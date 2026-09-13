use std::fs;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde_json::json;
use url::Url;

pub(super) const FINAL_URL_CYCLE: &str = "web_final_url_cycle";

use super::{
    Acquisition, AcquisitionResult, Discovery, Extraction, Job, Locator, Manifest, PreparedCapture,
    Proof, Provider, Reference, RemoteProvenance, RenderedDiscovery, SourceIdentity, discovery_id,
    now_secs, read_job, read_json, sha256_bytes, sha256_file,
};

pub(super) struct WebAcquisition<'a> {
    pub(super) job: &'a Job,
    pub(super) url: &'a str,
    pub(super) forbidden_final_urls: &'a [String],
}

impl Acquisition for WebAcquisition<'_> {
    fn source_hash(&self, _staging: &Path) -> Result<AcquisitionResult<String>> {
        Ok(AcquisitionResult::Ready(sha256_bytes(self.url.as_bytes())))
    }

    fn acquire(
        &self,
        capture: &super::publication::StagedCapture,
        source_hash: &str,
    ) -> Result<AcquisitionResult<PreparedCapture>> {
        fs::create_dir_all(capture.staging().join("proofs"))
            .context("creating Web Proof directory")?;
        fs::create_dir_all(capture.staging().join("extractions"))
            .context("creating Web Extraction directory")?;
        let deadline = UNIX_EPOCH
            .checked_add(Duration::from_secs(
                self.job
                    .created_at
                    .saturating_add(self.job.policy.snapshot.limits.duration_limit_secs),
            ))
            .context("calculating safe-web Policy deadline")?;
        let renderer_capture = crate::web::capture(
            self.url,
            capture.staging(),
            deadline,
            self.job.policy.snapshot.limits.disk_byte_limit,
            self.job.policy.snapshot.limits.download_byte_limit,
            || Ok(read_job(&self.job.id)?.state == "cancelled"),
        )?;
        let provenance = match renderer_capture {
            crate::web::Capture::Completed(provenance) => provenance,
            crate::web::Capture::Cancelled => return Ok(AcquisitionResult::Cancelled),
        };
        let mut final_url =
            Url::parse(&provenance.final_url).context("parsing rendered final URL")?;
        final_url.set_fragment(None);
        if self
            .forbidden_final_urls
            .iter()
            .any(|ancestor| ancestor == final_url.as_str())
        {
            bail!(FINAL_URL_CYCLE);
        }
        if read_job(&self.job.id)?.state == "cancelled" {
            return Ok(AcquisitionResult::Cancelled);
        }
        let artifacts = [
            ("proof-screenshot", "proofs/screenshot.png", "image/png"),
            ("discoveries", "discoveries.json", "application/json"),
            ("provenance", "provenance.json", "application/json"),
        ]
        .into_iter()
        .map(|(artifact_id, path, mime)| proof_for(capture.staging(), artifact_id, path, mime))
        .collect::<Result<Vec<_>>>()?;
        let proof = proof_for(
            capture.staging(),
            "proof-dom",
            "proofs/dom.html",
            "text/html",
        )?;
        let proof_hash = proof.sha256.clone();
        let proof_size = proof.size_bytes;
        let extraction = markdown_extraction(capture.staging(), &proof, &provenance.final_url)?;
        let discoveries = discoveries(capture.staging(), capture.capture_id(), &proof)?;
        let manifest = Manifest {
            capture_id: capture.capture_id().to_string(),
            source: SourceIdentity {
                locator: provenance.final_url.clone(),
                sha256: source_hash.to_string(),
            },
            policy: self.job.policy.clone(),
            published_at: now_secs(),
            proof,
            extractions: vec![extraction],
            capabilities: Vec::new(),
            artifacts,
            discoveries,
            remote_provenance: Some(RemoteProvenance {
                requested_url: if provenance.initial_url.is_empty() {
                    self.url.to_string()
                } else {
                    provenance.initial_url.clone()
                },
                final_url: provenance.final_url.clone(),
                mime: "text/html".to_string(),
                sha256: proof_hash,
                size_bytes: proof_size,
                redirect_chain: if provenance.redirect_chain.is_empty() {
                    vec![self.url.to_string(), provenance.final_url]
                } else {
                    provenance.redirect_chain
                },
            }),
        };
        Ok(AcquisitionResult::Ready(PreparedCapture::new(
            manifest, false,
        )))
    }
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
    rendered
        .into_iter()
        .map(|discovery| {
            let parent = Reference {
                capture_id: capture_id.to_string(),
                artifact_id: proof.artifact_id.clone(),
                sha256: proof.sha256.clone(),
                locator: Some(discovery.parent_locator),
            };
            Ok(Discovery {
                id: discovery_id(&parent, &discovery.url, &discovery.locator, discovery.order)?,
                source: discovery.url,
                parent,
                locator: discovery.locator,
                order: discovery.order,
                status: discovery.status,
                reason: discovery.reason,
                kind: discovery.kind,
            })
        })
        .collect::<Result<Vec<_>>>()
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
