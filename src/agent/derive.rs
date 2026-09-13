use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    ArtifactMetadata, Capability, Job, JobOperation, LedgerEvent, Manifest, Policy, Provider,
    Recipe, RecipeKind, RecipeTarget, Reference, append_json_line,
    artifact_for as capture_artifact_for, captures_dir, file_size, lock_capture_ledger, now_secs,
    provider_path, read_job, read_json, reference_for_artifact, sha256_file, unique_id, unlock_job,
    write_json,
};
use crate::resource::CaptureBudget;

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Derivative {
    derive_id: String,
    capture_id: String,
    job_id: String,
    recipe: Recipe,
    provider: Provider,
    inputs: Vec<Reference>,
    artifact: ArtifactMetadata,
    reference: Reference,
    capability: Capability,
    created_at: u64,
}

#[derive(Serialize)]
struct ProviderRequest {
    version: u8,
    recipe: Recipe,
    parameters: Value,
    inputs: Vec<ProviderInput>,
}

#[derive(Serialize)]
struct ProviderInput {
    reference: Reference,
    path: PathBuf,
    mime: String,
}

#[derive(Deserialize)]
struct ProviderResponse {
    mime: String,
    effective_parameters: Value,
}

pub(super) fn admit(
    capture_id: &str,
    recipe: RecipeKind,
    provider: &str,
    target: &RecipeTarget,
    policy: &Policy,
) -> Result<()> {
    super::validate_id(capture_id, "capture")?;
    if !policy.snapshot.allowed_recipes.contains(&recipe) {
        bail!(
            "Recipe `{}` is not allowed by Policy {}@{}",
            recipe.as_str(),
            policy.id,
            policy.version
        );
    }
    if !policy
        .snapshot
        .allowed_providers
        .iter()
        .any(|allowed| allowed == provider)
    {
        bail!(
            "Provider `{provider}` is not allowed by Policy {}@{}",
            policy.id,
            policy.version
        );
    }
    if policy.snapshot.allows_remote_calls {
        bail!("local Derive Policy cannot allow remote calls");
    }
    let manifest = manifest(capture_id)?;
    resolve_inputs(capture_id, &manifest, target).map(drop)
}

pub(super) fn publish(job: &Job) -> Result<String> {
    let JobOperation::Derive {
        capture_id,
        recipe,
        provider,
        parameters,
    } = &job.operation
    else {
        bail!("Derive Worker received a non-Derive Job");
    };
    let manifest = manifest(capture_id)?;
    let inputs = resolve_inputs(capture_id, &manifest, &recipe.target)?;
    let derive_id = format!("derive-{}", unique_id());
    let capture_dir = captures_dir()?.join(capture_id);
    let derivatives_dir = capture_dir.join("derivatives");
    fs::create_dir_all(&derivatives_dir).with_context(|| {
        format!(
            "creating Derivative directory {}",
            derivatives_dir.display()
        )
    })?;
    let staging = derivatives_dir.join(format!(".{derive_id}"));
    fs::create_dir(&staging)
        .with_context(|| format!("creating Derivative staging {}", staging.display()))?;
    let result = publish_staged(
        job,
        capture_id,
        recipe,
        provider,
        parameters,
        &inputs,
        &capture_dir,
        &staging,
        &derive_id,
    );
    if result.is_err() {
        drop(fs::remove_dir_all(&staging));
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn publish_staged(
    job: &Job,
    capture_id: &str,
    recipe: &Recipe,
    provider_name: &str,
    parameters: &Value,
    inputs: &[(Reference, ArtifactMetadata)],
    capture_dir: &Path,
    staging: &Path,
    derive_id: &str,
) -> Result<String> {
    let provider_binary = provider_path(provider_name)?;
    let request_path = staging.join("request.json");
    let output_path = staging.join("content");
    let request = ProviderRequest {
        version: 1,
        recipe: recipe.clone(),
        parameters: parameters.clone(),
        inputs: inputs
            .iter()
            .map(|(reference, artifact)| {
                Ok(ProviderInput {
                    reference: reference.clone(),
                    path: artifact_path(capture_dir, artifact)?,
                    mime: artifact.mime.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?,
    };
    write_json(&request_path, &request)?;
    let response = invoke_provider(
        job,
        provider_name,
        &provider_binary,
        &request_path,
        &output_path,
        staging,
    )?;
    if !fs::symlink_metadata(&output_path)
        .context("reading Derive output metadata")?
        .file_type()
        .is_file()
    {
        bail!("Derive Provider did not produce its output artifact");
    }
    let relative_path = format!("derivatives/{derive_id}/content");
    let created_at = now_secs();
    let provider = Provider {
        name: provider_name.to_string(),
        version: format!("sha256:{}", sha256_file(&provider_binary)?),
        parameters: response.effective_parameters,
        dependencies: Vec::new(),
    };
    let artifact = ArtifactMetadata {
        artifact_id: format!("{derive_id}-content"),
        path: relative_path,
        mime: response.mime,
        sha256: sha256_file(&output_path)?,
        size_bytes: file_size(&output_path)?,
        locator: None,
        created_at,
        provider: Some(provider.clone()),
        proof_artifact_id: None,
    };
    let reference = reference_for_artifact(capture_id, &artifact);
    let derivative = Derivative {
        derive_id: derive_id.to_string(),
        capture_id: capture_id.to_string(),
        job_id: job.id.clone(),
        recipe: recipe.clone(),
        provider: provider.clone(),
        inputs: inputs
            .iter()
            .map(|(reference, _)| reference.clone())
            .collect(),
        artifact,
        reference: reference.clone(),
        capability: Capability {
            name: recipe.kind.as_str().to_string(),
            state: "succeeded".to_string(),
            provider,
            error: None,
        },
        created_at,
    };
    write_json(&staging.join("manifest.json"), &derivative)?;
    fs::remove_file(&request_path).context("removing transient Derive request")?;
    let final_dir = capture_dir.join("derivatives").join(derive_id);
    fs::rename(staging, &final_dir)
        .with_context(|| format!("publishing Derivative {derive_id}"))?;
    let ledger_lock = lock_capture_ledger(capture_id)?;
    let append = append_json_line(
        &capture_dir.join("ledger.jsonl"),
        &LedgerEvent {
            event: "derivative_published".to_string(),
            at: created_at,
            job_id: job.id.clone(),
            details: Some(json!({
                "derive_id": derive_id,
                "recipe": recipe,
                "provider": derivative.provider,
                "inputs": derivative.inputs,
                "reference": reference,
            })),
        },
    );
    let unlock = unlock_job(&ledger_lock);
    append?;
    unlock?;
    Ok(derive_id.to_string())
}

fn invoke_provider(
    job: &Job,
    provider_name: &str,
    provider_binary: &Path,
    request_path: &Path,
    output_path: &Path,
    staging: &Path,
) -> Result<ProviderResponse> {
    let is_cancelled = || Ok(read_job(&job.id)?.state == "cancelled");
    let budget = CaptureBudget::new(
        staging,
        job.created_at,
        job.policy.snapshot.limits.duration_limit_secs,
        job.policy.snapshot.limits.disk_byte_limit,
    )
    .with_cancellation(&is_cancelled);
    let process = budget
        .output(
            Command::new(provider_binary)
                .args(["--request"])
                .arg(request_path)
                .args(["--output"])
                .arg(output_path),
        )
        .with_context(|| format!("running Derive Provider `{provider_name}`"))?;
    if !process.status.success() {
        bail!(
            "Derive Provider `{provider_name}` failed: {}",
            String::from_utf8_lossy(&process.stderr).trim()
        );
    }
    serde_json::from_slice(&process.stdout).context("reading Derive Provider response")
}

pub(super) fn list(capture_id: &str) -> Result<Vec<Derivative>> {
    let directory = captures_dir()?.join(capture_id).join("derivatives");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut derivatives = Vec::new();
    for entry in fs::read_dir(&directory)
        .with_context(|| format!("listing Derivatives in {}", directory.display()))?
    {
        let entry = entry.context("reading Derivative directory entry")?;
        if entry
            .file_type()
            .context("reading Derivative entry type")?
            .is_dir()
            && !entry.file_name().to_string_lossy().starts_with('.')
        {
            derivatives.push(read_json::<Derivative>(
                &entry.path().join("manifest.json"),
            )?);
        }
    }
    derivatives.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.derive_id.cmp(&right.derive_id))
    });
    Ok(derivatives)
}

pub(super) fn artifact_for(
    capture_id: &str,
    artifact_id: &str,
) -> Result<Option<ArtifactMetadata>> {
    Ok(list(capture_id)?
        .into_iter()
        .find(|derivative| derivative.artifact.artifact_id == artifact_id)
        .map(|derivative| derivative.artifact))
}

fn manifest(capture_id: &str) -> Result<Manifest> {
    read_json(&captures_dir()?.join(capture_id).join("manifest.json"))
}

fn resolve_inputs(
    capture_id: &str,
    manifest: &Manifest,
    target: &RecipeTarget,
) -> Result<Vec<(Reference, ArtifactMetadata)>> {
    match target {
        RecipeTarget::Capture {
            capture_id: target_capture_id,
        } => {
            if target_capture_id != capture_id {
                bail!("Recipe Capture target does not match the requested Capture");
            }
            let mut artifacts = vec![super::artifact_from_proof(&manifest.proof)];
            artifacts.extend(manifest.artifacts.iter().map(super::artifact_from_proof));
            artifacts.extend(
                manifest
                    .extractions
                    .iter()
                    .map(super::artifact_from_extraction),
            );
            artifacts
                .into_iter()
                .map(|artifact| verified_input(capture_id, manifest, artifact))
                .collect()
        }
        RecipeTarget::References { references } => references
            .iter()
            .map(|reference| {
                if reference.capture_id != capture_id {
                    bail!("Derive Reference belongs to another Capture");
                }
                let artifact = capture_artifact_for(manifest, &reference.artifact_id)
                    .context("unknown artifact in Derive Reference")?;
                if reference.sha256 != artifact.sha256 || reference.locator != artifact.locator {
                    bail!("artifact metadata does not match its Reference");
                }
                let (verified, artifact) = verified_input(capture_id, manifest, artifact)?;
                if verified != *reference {
                    bail!("artifact metadata does not match its Reference");
                }
                Ok((verified, artifact))
            })
            .collect(),
    }
}

fn verified_input(
    capture_id: &str,
    manifest: &Manifest,
    artifact: ArtifactMetadata,
) -> Result<(Reference, ArtifactMetadata)> {
    let path = artifact_path(&captures_dir()?.join(capture_id), &artifact)?;
    if sha256_file(&path)? != artifact.sha256 {
        bail!("artifact hash does not match its Reference");
    }
    let reference = reference_for_artifact(&manifest.capture_id, &artifact);
    Ok((reference, artifact))
}

fn artifact_path(capture_dir: &Path, artifact: &ArtifactMetadata) -> Result<PathBuf> {
    let relative = Path::new(&artifact.path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("invalid artifact path in Capture manifest");
    }
    Ok(capture_dir.join(relative))
}
