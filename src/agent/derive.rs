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
    now_secs, provider_path, read_job, read_json, read_json_lines, rebuild_search_index,
    record_index_degradation, reference_for_artifact, sha256_file, unique_id, unlock_job,
    write_job, write_json,
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
    #[serde(default)]
    context: Option<ArtifactMetadata>,
    #[serde(default)]
    context_reference: Option<Reference>,
    artifact: ArtifactMetadata,
    reference: Reference,
    capability: Capability,
    created_at: u64,
}

#[derive(Serialize)]
struct ProviderRequest {
    version: u8,
    context: ProviderContext,
    recipe: Recipe,
    parameters: Value,
    inputs: Vec<ProviderInput>,
}

#[derive(Serialize)]
struct ProviderContext {
    reference: Reference,
    path: PathBuf,
}

#[derive(Serialize)]
struct ProviderInput {
    reference: Reference,
    path: PathBuf,
    mime: String,
}

#[derive(Serialize)]
struct InferenceContext {
    format_version: u8,
    recipe: Recipe,
    budget: ContextBudget,
    selected: Vec<ContextSelection>,
    excluded_candidates: Vec<ExcludedCandidate>,
}

#[derive(Serialize)]
struct ContextBudget {
    duration_limit_secs: u64,
    disk_byte_limit: u64,
}

#[derive(Serialize)]
struct ContextSelection {
    reference: Reference,
    role: String,
    selection_reason: String,
}

#[derive(Serialize, Clone)]
struct ExcludedCandidate {
    reference: Reference,
    reason: String,
}

struct SelectedInput {
    reference: Reference,
    artifact: ArtifactMetadata,
    role: String,
    selection_reason: String,
}

struct InputSelection {
    selected: Vec<SelectedInput>,
    excluded_candidates: Vec<ExcludedCandidate>,
}

#[derive(Deserialize)]
struct ProviderResponse {
    mime: String,
    effective_parameters: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeOutput {
    format_version: u8,
    recipe: RecipeKind,
    #[serde(default)]
    claims: Vec<RecipeClaim>,
    #[serde(default)]
    knowledge_core: Option<KnowledgeCore>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KnowledgeCore {
    coverage: Vec<CoverageEntry>,
    statements: Vec<KnowledgeStatement>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CoverageEntry {
    reference: Reference,
    state: CoverageState,
    reason: String,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum CoverageState {
    Examined,
    Excluded,
    Unusable,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KnowledgeStatement {
    id: String,
    kind: StatementKind,
    text: String,
    #[serde(default)]
    anchors: Vec<ProofAnchor>,
    #[serde(default)]
    premises: Vec<String>,
    #[serde(default)]
    limitation: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProofAnchor {
    reference: Reference,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum StatementKind {
    AttributedDeclaration,
    Observation,
    AttributedRecommendation,
    Interpretation,
    Uncertainty,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeClaim {
    kind: ClaimKind,
    text: String,
    citations: Vec<Reference>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ClaimKind {
    Summary,
    ImportantIdea,
    VisibleText,
    VisualObservation,
    Uncertainty,
}

struct StagedPublication<'a> {
    job: &'a Job,
    capture_id: &'a str,
    recipe: &'a Recipe,
    provider_name: &'a str,
    parameters: &'a Value,
    inputs: &'a [SelectedInput],
    excluded_candidates: &'a [ExcludedCandidate],
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
    select_inputs(capture_id, &manifest, target).map(drop)
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
    let inputs = select_inputs(capture_id, &manifest, &recipe.target)?;
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
        inputs: &inputs.selected,
        excluded_candidates: &inputs.excluded_candidates,
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
        excluded_candidates,
        capture_dir,
        staging,
        derive_id: _,
    } = publication;
    let provider_binary = provider_path(provider_name)?;
    let context_path = staging.join("context.json");
    let context = InferenceContext {
        format_version: 1,
        recipe: (*recipe).clone(),
        budget: ContextBudget {
            duration_limit_secs: job.policy.snapshot.limits.duration_limit_secs,
            disk_byte_limit: job.policy.snapshot.limits.disk_byte_limit,
        },
        selected: inputs
            .iter()
            .map(|input| ContextSelection {
                reference: input.reference.clone(),
                role: input.role.clone(),
                selection_reason: input.selection_reason.clone(),
            })
            .collect(),
        excluded_candidates: excluded_candidates.to_vec(),
    };
    write_json(&context_path, &context)?;
    let context_artifact = context_artifact(publication, &context_path)?;
    let context_reference = reference_for_artifact(publication.capture_id, &context_artifact);
    let request_path = staging.join("request.json");
    let output_path = staging.join("content");
    let request = ProviderRequest {
        version: 1,
        context: ProviderContext {
            reference: context_reference.clone(),
            path: context_path,
        },
        recipe: (*recipe).clone(),
        parameters: (*parameters).clone(),
        inputs: inputs
            .iter()
            .map(|input| {
                Ok(ProviderInput {
                    reference: input.reference.clone(),
                    path: artifact_path(capture_dir, &input.artifact)?,
                    mime: input.artifact.mime.clone(),
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
    validate_recipe_output(
        &output_path,
        &response,
        recipe,
        publication.capture_id,
        inputs,
        excluded_candidates,
    )?;
    let derivative = build_derivative(
        publication,
        response,
        &provider_binary,
        &output_path,
        context_artifact,
        context_reference,
    )?;
    write_json(&staging.join("manifest.json"), &derivative)?;
    fs::remove_file(&request_path).context("removing transient Derive request")?;
    commit_derivative(publication, &derivative)
}

fn validate_recipe_output(
    output_path: &Path,
    response: &ProviderResponse,
    recipe: &Recipe,
    capture_id: &str,
    inputs: &[SelectedInput],
    excluded_candidates: &[ExcludedCandidate],
) -> Result<()> {
    if response.mime != "application/json" {
        return Err(coded_error(
            AgentErrorCode::InvalidRecipeOutput,
            "Recipe output must use application/json MIME",
        ));
    }
    let bytes = fs::read(output_path).context("reading Recipe output")?;
    let output: RecipeOutput = serde_json::from_slice(&bytes).map_err(|error| {
        coded_error(
            AgentErrorCode::InvalidRecipeOutput,
            format!("Recipe output is not valid structured JSON: {error}"),
        )
    })?;
    if output.format_version != 1 {
        return Err(coded_error(
            AgentErrorCode::InvalidRecipeOutput,
            format!(
                "Recipe output format_version `{}` is unsupported",
                output.format_version
            ),
        ));
    }
    if output.recipe != recipe.kind {
        return Err(coded_error(
            AgentErrorCode::InvalidRecipeOutput,
            "Recipe output does not match the requested Recipe",
        ));
    }
    if recipe.kind == RecipeKind::KnowledgeCard {
        if !output.claims.is_empty() {
            return invalid_recipe_output("knowledge-card output must not contain generic claims");
        }
        let Some(knowledge_core) = output.knowledge_core else {
            return invalid_recipe_output("knowledge-card output must contain a knowledge_core");
        };
        return validate_knowledge_core(knowledge_core, capture_id, inputs, excluded_candidates);
    }
    if output.knowledge_core.is_some() {
        return invalid_recipe_output("only knowledge-card output may contain a knowledge_core");
    }
    for claim in output.claims {
        if claim.text.trim().is_empty() {
            return Err(coded_error(
                AgentErrorCode::InvalidRecipeCitation,
                format!("Recipe {} claim has empty text", claim.kind.as_str()),
            ));
        }
        if claim.citations.is_empty() {
            return Err(coded_error(
                AgentErrorCode::InvalidRecipeCitation,
                format!("Recipe {} claim has no citation", claim.kind.as_str()),
            ));
        }
        for citation in claim.citations {
            if citation.capture_id != capture_id {
                return Err(coded_error(
                    AgentErrorCode::InvalidRecipeCitation,
                    "Recipe citation Capture is outside the inference corpus",
                ));
            }
            let Some(selected) = inputs
                .iter()
                .find(|input| input.reference.artifact_id == citation.artifact_id)
            else {
                return Err(coded_error(
                    AgentErrorCode::InvalidRecipeCitation,
                    "Recipe citation artifact is not selected for inference",
                ));
            };
            if selected.reference != citation {
                return Err(coded_error(
                    AgentErrorCode::InvalidRecipeCitation,
                    "Recipe citation does not match the selected artifact Reference",
                ));
            }
        }
    }
    Ok(())
}

fn invalid_recipe_output<T>(message: impl Into<String>) -> Result<T> {
    Err(coded_error(AgentErrorCode::InvalidRecipeOutput, message))
}

fn validate_knowledge_core(
    knowledge_core: KnowledgeCore,
    capture_id: &str,
    inputs: &[SelectedInput],
    excluded_candidates: &[ExcludedCandidate],
) -> Result<()> {
    validate_coverage(
        knowledge_core.coverage,
        capture_id,
        inputs,
        excluded_candidates,
    )?;
    validate_statements(knowledge_core.statements, capture_id, inputs)
}

fn validate_coverage(
    coverage: Vec<CoverageEntry>,
    capture_id: &str,
    inputs: &[SelectedInput],
    excluded_candidates: &[ExcludedCandidate],
) -> Result<()> {
    let mut covered_selected = std::collections::HashSet::new();
    let mut covered_excluded = std::collections::HashSet::new();
    for entry in coverage {
        if entry.reason.trim().is_empty() {
            return invalid_recipe_output("knowledge-card coverage has an empty reason");
        }
        if entry.reference.capture_id != capture_id {
            return invalid_recipe_output(
                "knowledge-card coverage is outside the inference corpus",
            );
        }
        if let Some(input) = inputs
            .iter()
            .find(|input| input.reference == entry.reference)
        {
            if entry.state == CoverageState::Excluded {
                return invalid_recipe_output("selected artifact cannot be excluded from coverage");
            }
            if !covered_selected.insert(input.reference.artifact_id.as_str()) {
                return invalid_recipe_output(
                    "knowledge-card coverage repeats a selected artifact",
                );
            }
            continue;
        }
        if let Some(excluded) = excluded_candidates
            .iter()
            .find(|excluded| excluded.reference == entry.reference)
        {
            if entry.state != CoverageState::Excluded || entry.reason != excluded.reason {
                return invalid_recipe_output(
                    "excluded artifact coverage does not match its exclusion",
                );
            }
            if !covered_excluded.insert(excluded.reference.artifact_id.as_str()) {
                return invalid_recipe_output(
                    "knowledge-card coverage repeats an excluded artifact",
                );
            }
            continue;
        }
        return invalid_recipe_output("knowledge-card coverage references an unselected artifact");
    }
    if inputs
        .iter()
        .any(|input| !covered_selected.contains(input.reference.artifact_id.as_str()))
        || excluded_candidates
            .iter()
            .any(|excluded| !covered_excluded.contains(excluded.reference.artifact_id.as_str()))
    {
        return invalid_recipe_output("knowledge-card coverage does not describe every input");
    }

    Ok(())
}

fn validate_statements(
    statements: Vec<KnowledgeStatement>,
    capture_id: &str,
    inputs: &[SelectedInput],
) -> Result<()> {
    let statement_ids = statements
        .iter()
        .map(|statement| statement.id.clone())
        .collect::<std::collections::HashSet<_>>();
    if statement_ids.len() != statements.len()
        || statement_ids.iter().any(|id| id.trim().is_empty())
    {
        return invalid_recipe_output("knowledge-card statement ids must be unique and non-empty");
    }
    for statement in statements {
        if statement.text.trim().is_empty() {
            return invalid_recipe_output("knowledge-card statement has empty text");
        }
        let is_uncertainty = statement.kind == StatementKind::Uncertainty;
        let is_interpretation = statement.kind == StatementKind::Interpretation;
        if is_uncertainty {
            if statement
                .limitation
                .as_deref()
                .is_none_or(|limitation| limitation.trim().is_empty())
            {
                return invalid_recipe_output(
                    "knowledge-card uncertainty has no concrete limitation",
                );
            }
        } else if statement.anchors.is_empty() {
            return invalid_recipe_output("knowledge-card statement has no proof anchor");
        } else if statement.limitation.is_some() {
            return invalid_recipe_output("only uncertainty may have a limitation");
        }
        if is_interpretation {
            if statement.premises.is_empty()
                || statement
                    .premises
                    .iter()
                    .any(|premise| !statement_ids.contains(premise))
            {
                return invalid_recipe_output(
                    "knowledge-card interpretation has an unknown premise",
                );
            }
        } else if !statement.premises.is_empty() {
            return invalid_recipe_output("only interpretation may have premises");
        }
        for anchor in statement.anchors {
            if anchor.reference.capture_id != capture_id {
                return Err(coded_error(
                    AgentErrorCode::InvalidRecipeCitation,
                    "knowledge-card proof anchor is outside the inference corpus",
                ));
            }
            if !inputs
                .iter()
                .any(|input| input.reference == anchor.reference)
            {
                return Err(coded_error(
                    AgentErrorCode::InvalidRecipeCitation,
                    "knowledge-card proof anchor is not a selected artifact Reference",
                ));
            }
        }
    }
    Ok(())
}

impl ClaimKind {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::ImportantIdea => "important-idea",
            Self::VisibleText => "visible-text",
            Self::VisualObservation => "visual-observation",
            Self::Uncertainty => "uncertainty",
        }
    }
}

fn build_derivative(
    publication: &StagedPublication<'_>,
    response: ProviderResponse,
    provider_binary: &Path,
    output_path: &Path,
    context: ArtifactMetadata,
    context_reference: Reference,
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
        order: None,
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
            .map(|input| input.reference.clone())
            .collect(),
        context: Some(context),
        context_reference: Some(context_reference),
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
        &derivative_published_event(derivative),
    );
    if let Err(error) = append {
        fs::rename(&final_dir, publication.staging)
            .context("rolling back Derivative publication")?;
        unlock_job(&ledger_lock)?;
        unlock_job(&job_lock)?;
        return Err(error);
    }
    unlock_job(&ledger_lock)?;
    if let Err(error) = rebuild_search_index() {
        record_index_degradation(&error);
    }
    current_job.state = "succeeded".to_string();
    current_job.updated_at = now_secs();
    current_job.derive_id = Some(publication.derive_id.to_string());
    current_job.worker_pid = None;
    write_job(&current_job)?;
    append_job_event(&publication.job.id, "succeeded")?;
    unlock_job(&job_lock)?;
    Ok(true)
}

pub(super) fn reconcile_published(job: &Job) -> Result<Option<String>> {
    let JobOperation::Derive { capture_id, .. } = &job.operation else {
        return Ok(None);
    };
    let Some(derivative) = list(capture_id)?
        .into_iter()
        .find(|derivative| derivative.job_id == job.id)
    else {
        return Ok(None);
    };
    let ledger_lock = lock_capture_ledger(capture_id)?;
    let ledger_path = captures_dir()?.join(capture_id).join("ledger.jsonl");
    let append = (|| -> Result<()> {
        let events: Vec<LedgerEvent> = read_json_lines(&ledger_path)?;
        let published = events.iter().any(|event| {
            event.event == "derivative_published"
                && event.job_id == job.id
                && event
                    .details
                    .as_ref()
                    .and_then(|details| details.get("derive_id"))
                    .and_then(Value::as_str)
                    == Some(derivative.derive_id.as_str())
        });
        if !published {
            append_json_line(&ledger_path, &derivative_published_event(&derivative))?;
        }
        Ok(())
    })();
    unlock_job(&ledger_lock)?;
    append?;
    Ok(Some(derivative.derive_id))
}

fn derivative_published_event(derivative: &Derivative) -> LedgerEvent {
    LedgerEvent {
        event: "derivative_published".to_string(),
        at: derivative.created_at,
        job_id: derivative.job_id.clone(),
        details: Some(json!({
            "derive_id": derivative.derive_id,
            "recipe": derivative.recipe,
            "provider": derivative.provider,
            "inputs": derivative.inputs,
            "context_reference": derivative.context_reference,
            "reference": derivative.reference,
        })),
    }
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
    Ok(list(capture_id)?.into_iter().find_map(|derivative| {
        if derivative.artifact.artifact_id == artifact_id {
            Some(derivative.artifact)
        } else {
            derivative
                .context
                .filter(|artifact| artifact.artifact_id == artifact_id)
        }
    }))
}

pub(super) fn search_artifacts(capture_id: &str) -> Result<Vec<ArtifactMetadata>> {
    Ok(list(capture_id)?
        .into_iter()
        .map(|derivative| derivative.artifact)
        .filter(|artifact| super::is_text_mime(&artifact.mime))
        .collect())
}

fn manifest(capture_id: &str) -> Result<Manifest> {
    read_json(&captures_dir()?.join(capture_id).join("manifest.json"))
}

fn select_inputs(
    capture_id: &str,
    manifest: &Manifest,
    target: &RecipeTarget,
) -> Result<InputSelection> {
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
            let verified = artifacts
                .into_iter()
                .map(|artifact| verified_input(capture_id, manifest, artifact))
                .collect::<Result<Vec<_>>>()?;
            let (selected, excluded_candidates) = verified.into_iter().fold(
                (Vec::new(), Vec::new()),
                |(mut selected, mut excluded), (reference, artifact)| {
                    if let Some((role, selection_reason)) = automatic_selection(&artifact) {
                        selected.push(SelectedInput {
                            reference,
                            artifact,
                            role: role.to_string(),
                            selection_reason: selection_reason.to_string(),
                        });
                    } else {
                        excluded.push(ExcludedCandidate {
                            reference,
                            reason: exclusion_reason(&artifact).to_string(),
                        });
                    }
                    (selected, excluded)
                },
            );
            Ok(InputSelection {
                selected,
                excluded_candidates,
            })
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
                Ok(SelectedInput {
                    reference: verified,
                    role: input_role(&artifact).to_string(),
                    selection_reason: "explicit-selection".to_string(),
                    artifact,
                })
            })
            .collect::<Result<Vec<_>>>()
            .map(|selected| InputSelection {
                selected,
                excluded_candidates: Vec::new(),
            }),
    }
}

fn automatic_selection(artifact: &ArtifactMetadata) -> Option<(&'static str, &'static str)> {
    if super::is_text_mime(&artifact.mime) {
        Some((input_role(artifact), text_selection_reason(artifact)))
    } else if artifact.mime.starts_with("image/") {
        Some(("visual-frame", "coverage"))
    } else {
        None
    }
}

fn input_role(artifact: &ArtifactMetadata) -> &'static str {
    if artifact.artifact_id.contains("caption") {
        "caption"
    } else if artifact.artifact_id.contains("transcription") {
        "transcription"
    } else if artifact.mime.starts_with("image/") {
        "visual-frame"
    } else {
        "visible-text"
    }
}

fn text_selection_reason(artifact: &ArtifactMetadata) -> &'static str {
    if artifact.artifact_id.contains("ocr") {
        "new-ocr-text"
    } else {
        "source-context"
    }
}

fn exclusion_reason(artifact: &ArtifactMetadata) -> &'static str {
    if artifact.mime.starts_with("video/") {
        "raw-video-unsupported"
    } else if artifact.mime.starts_with("audio/") {
        "raw-audio-unsupported"
    } else if artifact.mime.starts_with("application/") {
        "raw-document-unsupported"
    } else {
        "not-admissible-for-whole-capture"
    }
}

fn context_artifact(
    publication: &StagedPublication<'_>,
    context_path: &Path,
) -> Result<ArtifactMetadata> {
    Ok(ArtifactMetadata {
        artifact_id: format!("{}-context", publication.derive_id),
        path: format!("derivatives/{}/context.json", publication.derive_id),
        mime: "application/json".to_string(),
        sha256: sha256_file(context_path)?,
        size_bytes: file_size(context_path)?,
        locator: None,
        created_at: now_secs(),
        provider: None,
        proof_artifact_id: None,
        order: None,
    })
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

pub(super) fn artifact_path(capture_dir: &Path, artifact: &ArtifactMetadata) -> Result<PathBuf> {
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
