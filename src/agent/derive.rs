use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{AgentErrorCode, coded_error};
use super::{
    ArtifactMetadata, Capability, Job, JobOperation, LedgerEvent, Manifest, Policy, Provider,
    Recipe, RecipeKind, RecipeTarget, Reference, append_job_event, append_json_line,
    artifact_for as capture_artifact_for, captures_dir, file_size, lock_capture_ledger, lock_job,
    now_secs, provider_path, read_job, read_json, reference_for_artifact, sha256_file, unique_id,
    unlock_job, write_job, write_json,
};
use crate::resource::ResourceBudget;

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

struct StagedPublication<'a> {
    job: &'a Job,
    capture_id: &'a str,
    recipe: &'a Recipe,
    provider_name: &'a str,
    parameters: &'a Value,
    inputs: &'a [(Reference, ArtifactMetadata)],
    capture_dir: &'a Path,
    staging: &'a Path,
    derive_id: &'a str,
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
        return Err(coded_error(
            AgentErrorCode::RecipeNotAllowed,
            format!(
                "Recipe `{}` is not allowed by Policy {}@{}",
                recipe.as_str(),
                policy.id,
                policy.version
            ),
        ));
    }
    if !super::policy_allows(policy, provider) {
        return Err(coded_error(
            AgentErrorCode::ProviderNotAllowed,
            format!(
                "Provider `{provider}` is not allowed by Policy {}@{}",
                policy.id, policy.version
            ),
        ));
    }
    if policy.snapshot.allows_remote_calls {
        bail!("local Derive Policy cannot allow remote calls");
    }
    let manifest = manifest(capture_id)?;
    resolve_inputs(capture_id, &manifest, target).map(drop)
}

pub(super) fn admit_retry(
    job_id: &str,
    capture_id: &str,
    recipe: &Recipe,
    provider: &str,
    parameters: &Value,
) -> Result<()> {
    let previous = read_job(job_id).context("reading retried Derive Job")?;
    let JobOperation::Derive {
        capture_id: previous_capture,
        recipe: previous_recipe,
        provider: previous_provider,
        parameters: previous_parameters,
    } = previous.operation
    else {
        return Err(coded_error(
            AgentErrorCode::InvalidRetry,
            "retry target is not a Derive Job",
        ));
    };
    if previous_capture != capture_id
        || previous_recipe != *recipe
        || previous_provider != provider
        || previous_parameters != *parameters
    {
        return Err(coded_error(
            AgentErrorCode::InvalidRetry,
            "retry target does not match the Derive request",
        ));
    }
    Ok(())
}

pub(super) fn publish(job: &Job) -> Result<()> {
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
    let result = publish_staged(&StagedPublication {
        job,
        capture_id,
        recipe,
        provider_name: provider,
        parameters,
        inputs: &inputs,
        capture_dir: &capture_dir,
        staging: &staging,
        derive_id: &derive_id,
    });
    if result.is_err() || result.as_ref().is_ok_and(|published| !published) {
        drop(fs::remove_dir_all(&staging));
    }
    result.map(drop)
}

fn publish_staged(publication: &StagedPublication<'_>) -> Result<bool> {
    let StagedPublication {
        job,
        capture_id: _,
        recipe,
        provider_name,
        parameters,
        inputs,
        capture_dir,
        staging,
        derive_id: _,
    } = publication;
    let provider_binary = provider_path(provider_name)?;
    let request_path = staging.join("request.json");
    let output_path = staging.join("content");
    let request = ProviderRequest {
        version: 1,
        recipe: (*recipe).clone(),
        parameters: (*parameters).clone(),
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
    validate_parameters(&response.effective_parameters)?;
    if !fs::symlink_metadata(&output_path)
        .context("reading Derive output metadata")?
        .file_type()
        .is_file()
    {
        bail!("Derive Provider did not produce its output artifact");
    }
    let derivative = build_derivative(publication, response, &provider_binary, &output_path)?;
    write_json(&staging.join("manifest.json"), &derivative)?;
    fs::remove_file(&request_path).context("removing transient Derive request")?;
    commit_derivative(publication, &derivative)
}

fn build_derivative(
    publication: &StagedPublication<'_>,
    response: ProviderResponse,
    provider_binary: &Path,
    output_path: &Path,
) -> Result<Derivative> {
    let created_at = now_secs();
    let provider = Provider {
        name: publication.provider_name.to_string(),
        version: format!("sha256:{}", sha256_file(provider_binary)?),
        parameters: response.effective_parameters,
        dependencies: Vec::new(),
    };
    let artifact = ArtifactMetadata {
        artifact_id: format!("{}-content", publication.derive_id),
        path: format!("derivatives/{}/content", publication.derive_id),
        mime: response.mime,
        sha256: sha256_file(output_path)?,
        size_bytes: file_size(output_path)?,
        locator: None,
        created_at,
        provider: Some(provider.clone()),
        proof_artifact_id: None,
    };
    let reference = reference_for_artifact(publication.capture_id, &artifact);
    Ok(Derivative {
        derive_id: publication.derive_id.to_string(),
        capture_id: publication.capture_id.to_string(),
        job_id: publication.job.id.clone(),
        recipe: publication.recipe.clone(),
        provider: provider.clone(),
        inputs: publication
            .inputs
            .iter()
            .map(|(reference, _)| reference.clone())
            .collect(),
        artifact,
        reference,
        capability: Capability {
            name: publication.recipe.kind.as_str().to_string(),
            state: "succeeded".to_string(),
            provider,
            error: None,
        },
        created_at,
    })
}

fn commit_derivative(publication: &StagedPublication<'_>, derivative: &Derivative) -> Result<bool> {
    let job_lock = lock_job(&publication.job.id)?;
    let mut current_job = read_job(&publication.job.id)?;
    if current_job.state == "cancelled" {
        unlock_job(&job_lock)?;
        return Ok(false);
    }
    let ledger_lock = lock_capture_ledger(publication.capture_id)?;
    let final_dir = publication
        .capture_dir
        .join("derivatives")
        .join(publication.derive_id);
    fs::rename(publication.staging, &final_dir)
        .with_context(|| format!("publishing Derivative {}", publication.derive_id))?;
    let append = append_json_line(
        &publication.capture_dir.join("ledger.jsonl"),
        &LedgerEvent {
            event: "derivative_published".to_string(),
            at: derivative.created_at,
            job_id: publication.job.id.clone(),
            details: Some(json!({
                "derive_id": publication.derive_id,
                "recipe": publication.recipe,
                "provider": derivative.provider,
                "inputs": derivative.inputs,
                "reference": derivative.reference,
            })),
        },
    );
    if let Err(error) = append {
        fs::rename(&final_dir, publication.staging)
            .context("rolling back Derivative publication")?;
        unlock_job(&ledger_lock)?;
        unlock_job(&job_lock)?;
        return Err(error);
    }
    current_job.state = "succeeded".to_string();
    current_job.updated_at = now_secs();
    current_job.derive_id = Some(publication.derive_id.to_string());
    current_job.worker_pid = None;
    write_job(&current_job)?;
    append_job_event(&publication.job.id, "succeeded")?;
    let unlock_ledger = unlock_job(&ledger_lock);
    let unlock_job_result = unlock_job(&job_lock);
    unlock_ledger?;
    unlock_job_result?;
    Ok(true)
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
    let budget = ResourceBudget::new(
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

pub(super) fn validate_parameters(parameters: &Value) -> Result<()> {
    let Some(fields) = parameters.as_object() else {
        return Err(coded_error(
            AgentErrorCode::SensitiveParameters,
            "Derive parameters must follow the non-secret Provider schema",
        ));
    };
    for (name, value) in fields {
        let valid = match name.as_str() {
            "language" | "model" | "model_sha256" | "style" => value.is_string(),
            "max_tokens" | "seed" => value.as_u64().is_some(),
            "temperature" | "top_p" => value.is_number(),
            _ => false,
        };
        if !valid {
            return Err(coded_error(
                AgentErrorCode::SensitiveParameters,
                format!("Derive parameter `{name}` is not allowed by the Provider schema"),
            ));
        }
    }
    Ok(())
}

pub(super) fn list(capture_id: &str) -> Result<Vec<Derivative>> {
    let directory = captures_dir()?.join(capture_id).join("derivatives");
    if !directory
        .try_exists()
        .with_context(|| format!("checking Derivative directory {}", directory.display()))?
    {
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
