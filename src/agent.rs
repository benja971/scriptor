use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, BoostQuery, Occur, PhraseQuery, QueryParser, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, STORED, STRING, Schema, TEXT, Term, TextFieldIndexing, TextOptions,
    Value as TantivyValue,
};
use tantivy::tokenizer::{LowerCaser, SimpleTokenizer, TextAnalyzer};
use tantivy::{Index, TantivyDocument, doc};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};
use url::Url;

use crate::config::Config;
use crate::resource::ResourceBudget;
use crate::unique_id::unique_id;
use crate::{audio, frames, transcribe};

mod admission;
mod derive;
mod error;
mod publication;
mod social_capture;
mod web_capture;

use error::CodedError;
pub use error::{AgentErrorCode, coded_error};
use publication::{Acquisition, AcquisitionResult, PreparedCapture, Publication};

const POLICY_NAME: &str = "safe-local@1";
const DEFAULT_PAGE_LIMIT: usize = 20;
const MAX_PAGE_LIMIT: usize = 100;
const SEARCH_MAX_CANDIDATES: usize = 500;
const SEARCH_INDEX_VERSION: u8 = 3;
const KNOWLEDGE_INDEX_VERSION: u8 = 1;
const KNOWLEDGE_ANALYZER_VERSION: &str = "knowledge_v1";
const KNOWLEDGE_PHRASE_BOOST: f32 = 2.0;
const DEFAULT_READ_LENGTH: usize = 8 * 1024;
const MAX_READ_LENGTH: usize = 1024 * 1024;
pub const MANIFEST_FORMAT_VERSION: u8 = 1;

#[derive(Parser)]
#[command(name = "scriptor")]
struct AgentCli {
    #[command(subcommand)]
    command: AgentCommand,
}

#[derive(Subcommand)]
enum AgentCommand {
    Capture(CaptureCommand),
    Knowledge(KnowledgeCommand),
    Derive(DeriveCommand),
    Job(JobCommand),
    #[command(name = "capture-worker", hide = true)]
    CaptureWorker {
        #[arg(long)]
        job_id: String,
    },
}

#[derive(Args)]
struct KnowledgeCommand {
    #[command(subcommand)]
    command: KnowledgeSubcommand,
}

#[derive(Subcommand)]
enum KnowledgeSubcommand {
    Search {
        query: String,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long, default_value_t = DEFAULT_PAGE_LIMIT)]
        limit: usize,
    },
    Index(KnowledgeIndexCommand),
}

#[derive(Args)]
struct KnowledgeIndexCommand {
    #[command(subcommand)]
    command: KnowledgeIndexSubcommand,
}

#[derive(Subcommand)]
enum KnowledgeIndexSubcommand {
    Rebuild,
}

#[derive(Args)]
struct DeriveCommand {
    capture_id: String,
    #[arg(long, value_enum)]
    recipe: RecipeKind,
    #[arg(long)]
    provider: String,
    #[arg(long)]
    policy: Option<String>,
    #[arg(long, conflicts_with = "reference")]
    whole_capture: bool,
    #[arg(long)]
    reference: Vec<String>,
    #[arg(long)]
    parameters: Option<String>,
    #[arg(long)]
    retry_of: Option<String>,
}

#[derive(Args)]
struct CaptureCommand {
    source: Option<PathBuf>,
    #[arg(long, global = true)]
    policy: Option<String>,
    #[arg(long, value_enum)]
    renderer: Option<WebRenderer>,
    #[arg(long)]
    retry_of: Option<String>,
    #[command(subcommand)]
    command: Option<CaptureSubcommand>,
}

#[derive(Subcommand)]
enum CaptureSubcommand {
    Continue {
        capture_id: String,
        #[arg(long = "discovery")]
        discovery_ids: Vec<String>,
    },
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
#[serde(deny_unknown_fields)]
struct PolicySnapshot {
    duplicate_mode: String,
    limits: Limits,
    allows_remote_calls: bool,
    allowed_providers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    allowed_recipes: Vec<RecipeKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyDocument {
    id: String,
    version: u8,
    snapshot: PolicySnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    #[serde(rename = "job_id")]
    id: String,
    state: String,
    source: String,
    #[serde(default)]
    operation: JobOperation,
    policy: Policy,
    created_at: u64,
    updated_at: u64,
    worker_pid: Option<u32>,
    capture_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    derive_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retry_of: Option<String>,
    #[serde(default)]
    child_capture_ids: Vec<String>,
    #[serde(default)]
    checkpoint: Option<ResolutionCheckpoint>,
    error: Option<StructuredError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum JobOperation {
    Capture {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        renderer: Option<WebRenderer>,
    },
    Continue {
        parent_capture_id: String,
        discovery_ids: Vec<String>,
    },
    Derive {
        capture_id: String,
        recipe: Recipe,
        provider: String,
        parameters: Value,
    },
}

impl Default for JobOperation {
    fn default() -> Self {
        Self::Capture { renderer: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
enum WebRenderer {
    Lightpanda,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
enum RecipeKind {
    KnowledgeCard,
    StructuredSummary,
    ProvenClaims,
    Checklist,
    MarkdownNote,
    SourcedAnswer,
}

impl RecipeKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::KnowledgeCard => "knowledge-card",
            Self::StructuredSummary => "structured-summary",
            Self::ProvenClaims => "proven-claims",
            Self::Checklist => "checklist",
            Self::MarkdownNote => "markdown-note",
            Self::SourcedAnswer => "sourced-answer",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Recipe {
    kind: RecipeKind,
    target: RecipeTarget,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RecipeTarget {
    Capture { capture_id: String },
    References { references: Vec<Reference> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResolutionCheckpoint {
    queue: Vec<QueuedDiscovery>,
    attempted_sources: u8,
    downloaded_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QueuedDiscovery {
    discovery_id: String,
    source: String,
    depth: u8,
    order: u32,
    #[serde(default)]
    order_path: Vec<u32>,
    ancestors: Vec<String>,
    #[serde(default)]
    kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StructuredError {
    code: String,
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capability: Option<String>,
}

#[derive(Serialize)]
struct CreatedJob<'a> {
    job: &'a Job,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(
        default = "legacy_manifest_format_version",
        deserialize_with = "deserialize_manifest_format_version"
    )]
    format_version: u8,
    capture_id: String,
    #[serde(default = "legacy_capture_version")]
    capture_version: u64,
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
    #[serde(default)]
    discoveries: Vec<Discovery>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    remote_provenance: Option<RemoteProvenance>,
}

const fn legacy_manifest_format_version() -> u8 {
    MANIFEST_FORMAT_VERSION
}

const fn legacy_capture_version() -> u64 {
    1
}

fn deserialize_manifest_format_version<'de, D>(deserializer: D) -> std::result::Result<u8, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let version = u8::deserialize(deserializer)?;
    if version != MANIFEST_FORMAT_VERSION {
        return Err(serde::de::Error::custom(format!(
            "unsupported Capture manifest format version {version}"
        )));
    }
    Ok(version)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RemoteProvenance {
    requested_url: String,
    final_url: String,
    mime: String,
    sha256: String,
    size_bytes: u64,
    redirect_chain: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    canonical_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    post_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    published_at: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    media: Vec<RemoteMediaProvenance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RemoteMediaProvenance {
    order: u32,
    requested_url: String,
    final_url: String,
    mime: String,
    sha256: String,
    size_bytes: u64,
    redirect_chain: Vec<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    order: Option<u32>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum Locator {
    File,
    Url {
        value: String,
    },
    MediaTimestamp {
        timestamps_secs: Vec<f64>,
    },
    PdfPages {
        first_page: u32,
        last_page: u32,
    },
    ImageRegions {
        regions: Vec<ImageRegion>,
    },
    FrameRegions {
        timestamps_secs: Vec<f64>,
        regions: Vec<ImageRegion>,
    },
    CssSelector {
        value: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    derivatives: Vec<derive::Derivative>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LedgerEvent {
    event: String,
    at: u64,
    job_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
struct Reference {
    capture_id: String,
    artifact_id: String,
    sha256: String,
    locator: Option<Locator>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Discovery {
    #[serde(default, rename = "discovery_id")]
    id: String,
    source: String,
    parent: Reference,
    locator: Locator,
    order: u32,
    status: String,
    reason: String,
    #[serde(default)]
    kind: Option<String>,
}

fn discovery_id(parent: &Reference, source: &str, locator: &Locator, order: u32) -> Result<String> {
    let identity = serde_json::to_vec(&(parent, source, locator, order))
        .context("serializing Discovery identity")?;
    Ok(sha256_bytes(&identity))
}

#[derive(Deserialize)]
struct RenderedDiscovery {
    url: String,
    parent_locator: Locator,
    locator: Locator,
    order: u32,
    status: String,
    reason: String,
    #[serde(default)]
    kind: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
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

#[derive(Serialize)]
struct SearchPage {
    captures: Vec<CaptureSummary>,
    next_cursor: Option<String>,
    truncated: bool,
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

#[derive(Debug, Clone, Deserialize, Serialize)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    order: Option<u32>,
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

#[derive(Deserialize, Serialize)]
struct SearchCursor {
    version: u8,
    query: String,
    index_version: u8,
    captures: Vec<CaptureSummary>,
    position: usize,
    truncated: bool,
}

struct SearchFields {
    capture_id: Field,
    source: Field,
    source_sha256: Field,
    published_at: Field,
    artifact_id: Field,
    artifact_sha256: Field,
    artifact_locator: Field,
    text: Field,
}

struct KnowledgeFields {
    text: Field,
    statement_text: Field,
    statement_kind: Field,
    reference: Field,
    statement_id: Field,
    source_locator: Field,
    remote_published_at: Field,
    capture_published_at: Field,
}

#[derive(Clone, Deserialize, Serialize)]
struct KnowledgeReference {
    capture_id: String,
    artifact_id: String,
    sha256: String,
    locator: Option<Locator>,
}

#[derive(Clone, Deserialize, Serialize)]
struct KnowledgeResult {
    text: String,
    kind: String,
    reference: KnowledgeReference,
    statement_id: String,
    source: KnowledgeSourceSummary,
    #[serde(skip)]
    remote_published_at: Option<String>,
    #[serde(skip)]
    capture_published_at: u64,
}

#[derive(Clone, Deserialize, Serialize)]
struct KnowledgeSourceSummary {
    locator: String,
}

#[derive(Serialize)]
struct KnowledgePage {
    results: Vec<KnowledgeResult>,
    next_cursor: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct KnowledgeCursor {
    version: u8,
    query: String,
    snapshot: String,
    position: usize,
    results: Vec<KnowledgeResult>,
}

#[derive(Deserialize, Serialize)]
struct KnowledgeIndexMetadata {
    version: u8,
    analyzer: String,
    snapshot: String,
}

#[derive(Deserialize)]
struct KnowledgeDerivativeManifest {
    recipe: KnowledgeRecipe,
    artifact: KnowledgeArtifact,
    reference: KnowledgeReference,
    created_at: u64,
}

#[derive(Deserialize, Serialize)]
struct KnowledgeRecipe {
    kind: String,
    target: Value,
}

#[derive(Deserialize)]
struct KnowledgeArtifact {
    path: String,
}

#[derive(Deserialize)]
struct KnowledgeCard {
    format_version: u8,
    recipe: String,
    knowledge_core: KnowledgeCore,
}

#[derive(Deserialize)]
struct KnowledgeCore {
    statements: Vec<KnowledgeStatement>,
}

#[derive(Deserialize)]
struct KnowledgeStatement {
    id: String,
    kind: String,
    text: String,
}

struct KnowledgeCardRecord {
    derivative: KnowledgeDerivativeManifest,
    statements: Vec<KnowledgeStatement>,
    supersession_key: String,
    source_locator: String,
    remote_published_at: Option<String>,
    capture_published_at: u64,
}

#[derive(Serialize)]
struct RebuiltSearchIndex {
    captures: usize,
}

#[derive(Serialize)]
struct RebuiltKnowledgeIndex {
    cards: usize,
}

struct ReadRequest {
    capture_id: String,
    artifact_id: String,
    expected_sha256: Option<String>,
}

pub fn run(arguments: Vec<OsString>) -> Result<()> {
    let cli = match AgentCli::try_parse_from(arguments) {
        Ok(cli) => cli,
        Err(error) => return print_agent_error("invalid_command", error.to_string()),
    };
    let result = match cli.command {
        AgentCommand::Capture(command) => run_capture_command(command),
        AgentCommand::Knowledge(command) => run_knowledge_command(command),
        AgentCommand::Derive(command) => run_derive_command(command),
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

fn run_knowledge_command(command: KnowledgeCommand) -> Result<()> {
    match command.command {
        KnowledgeSubcommand::Search {
            query,
            cursor,
            limit,
        } => search_knowledge(&query, cursor.as_deref(), limit),
        KnowledgeSubcommand::Index(KnowledgeIndexCommand {
            command: KnowledgeIndexSubcommand::Rebuild,
        }) => print_json(&RebuiltKnowledgeIndex {
            cards: rebuild_knowledge_index()?,
        }),
    }
}

fn run_derive_command(command: DeriveCommand) -> Result<()> {
    let policy_name = command
        .policy
        .as_deref()
        .context("derive requires --policy")?;
    let parameters = command
        .parameters
        .as_deref()
        .map_or_else(
            || Ok(Value::Object(serde_json::Map::new())),
            serde_json::from_str,
        )
        .context("parsing Derive parameters as JSON")?;
    derive::validate_parameters(&parameters)?;
    let references = command
        .reference
        .iter()
        .map(|reference| {
            serde_json::from_str(reference).context("parsing Derive Reference as JSON")
        })
        .collect::<Result<Vec<Reference>>>()?;
    if !command.whole_capture && references.is_empty() {
        return Err(coded_error(
            AgentErrorCode::InvalidDeriveTarget,
            "derive requires --whole-capture or --reference",
        ));
    }
    let target = if command.whole_capture {
        RecipeTarget::Capture {
            capture_id: command.capture_id.clone(),
        }
    } else {
        RecipeTarget::References { references }
    };
    let policy = policy_for(policy_name)?;
    derive::admit(
        &command.capture_id,
        command.recipe,
        &command.provider,
        &target,
        &policy,
    )?;
    let recipe = Recipe {
        kind: command.recipe,
        target,
    };
    if let Some(retry_of) = command.retry_of.as_deref() {
        derive::admit_retry(
            retry_of,
            &command.capture_id,
            &recipe,
            &command.provider,
            &parameters,
        )?;
    }
    create_job(
        command.capture_id.clone(),
        policy,
        JobOperation::Derive {
            capture_id: command.capture_id,
            recipe,
            provider: command.provider,
            parameters,
        },
        command.retry_of,
    )
}

fn print_agent_error(code: &str, message: String) -> Result<()> {
    print_json(&AgentError {
        error: StructuredError {
            code: code.to_string(),
            message,
            capability: None,
        },
    })
}

fn agent_error(error: &anyhow::Error) -> StructuredError {
    let message = format!("{error:#}");
    if let Some(code) = error.chain().find_map(|cause| {
        cause
            .downcast_ref::<CodedError>()
            .map(|coded| coded.code.as_str())
    }) {
        return StructuredError {
            code: code.to_string(),
            message,
            capability: None,
        };
    }
    let code = if message.contains("pagination cursor") {
        "invalid_cursor"
    } else if message.contains("requires --policy") {
        "policy_required"
    } else if message.contains("Policy") || message.contains("policy JSON") {
        "invalid_policy"
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
        capability: None,
    }
}

fn run_capture_command(command: CaptureCommand) -> Result<()> {
    match command.command {
        Some(CaptureSubcommand::Continue {
            capture_id,
            discovery_ids,
        }) => continue_capture(&capture_id, &discovery_ids, command.policy.as_deref()),
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
            command.renderer,
            command.retry_of,
        ),
    }
}

fn continue_capture(
    capture_id: &str,
    discovery_ids: &[String],
    policy_name: Option<&str>,
) -> Result<()> {
    validate_id(capture_id, "capture")?;
    let policy = policy_for(policy_name.context("capture continue requires --policy")?)?;
    let parent: Manifest = read_json(&captures_dir()?.join(capture_id).join("manifest.json"))?;
    for discovery_id in discovery_ids {
        if !parent
            .discoveries
            .iter()
            .any(|discovery| discovery.id == *discovery_id)
        {
            bail!("unknown Discovery `{discovery_id}` for Capture `{capture_id}`");
        }
    }
    create_job(
        capture_id.to_string(),
        policy,
        JobOperation::Continue {
            parent_capture_id: capture_id.to_string(),
            discovery_ids: discovery_ids.to_vec(),
        },
        None,
    )
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

fn create_capture_job(
    source: &Path,
    policy_name: &str,
    renderer: Option<WebRenderer>,
    retry_of: Option<String>,
) -> Result<()> {
    let policy = policy_for(policy_name)?;
    let source = admission::admit(source, &policy)?;
    if renderer.is_some() && !matches!(source, admission::Source::Web(_)) {
        bail!("--renderer is only supported for a Web Source");
    }
    let source = source.into_job_source();
    if let Some(job_id) = retry_of.as_deref() {
        admit_capture_retry(job_id, &source, &policy, renderer)?;
    }
    create_job(source, policy, JobOperation::Capture { renderer }, retry_of)
}

fn admit_capture_retry(
    job_id: &str,
    source: &str,
    policy: &Policy,
    renderer: Option<WebRenderer>,
) -> Result<()> {
    let previous = reconcile_interrupted(job_id).context("reading retried Capture Job")?;
    let JobOperation::Capture {
        renderer: previous_renderer,
    } = previous.operation
    else {
        return Err(coded_error(
            AgentErrorCode::InvalidRetry,
            "retry target is not a Capture Job",
        ));
    };
    if previous.state != "interrupted"
        || previous.source != source
        || previous.policy.sha256 != policy.sha256
        || previous_renderer != renderer
    {
        return Err(coded_error(
            AgentErrorCode::InvalidRetry,
            "retry target does not match an interrupted Capture request",
        ));
    }
    Ok(())
}

fn create_job(
    source: String,
    policy: Policy,
    operation: JobOperation,
    retry_of: Option<String>,
) -> Result<()> {
    let now = now_secs();
    let job = Job {
        id: format!("job-{}", unique_id()),
        state: "queued".to_string(),
        source,
        operation,
        policy,
        created_at: now,
        updated_at: now,
        worker_pid: None,
        capture_id: None,
        derive_id: None,
        retry_of,
        child_capture_ids: Vec::new(),
        checkpoint: None,
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
        .process_group(0)
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

    if let JobOperation::Continue {
        parent_capture_id, ..
    } = &job.operation
    {
        return match resolve_discoveries(&job, parent_capture_id) {
            Ok(()) => Ok(()),
            Err(error) => fail_job(job_id, &error),
        };
    }

    if matches!(job.operation, JobOperation::Derive { .. }) {
        return match derive::publish(&job) {
            Ok(()) => Ok(()),
            Err(error) => fail_job(job_id, &error),
        };
    }

    match publish_capture(&job) {
        Ok(Publication::Published {
            capture_id,
            partial,
            ..
        }) => complete_job(job_id, capture_id, partial),
        Ok(Publication::Cancelled) => Ok(()),
        Err(error) => fail_job(job_id, &error),
    }
}

#[allow(clippy::too_many_lines)]
fn resolve_discoveries(job: &Job, parent_capture_id: &str) -> Result<()> {
    let parent: Manifest = read_json(
        &captures_dir()?
            .join(parent_capture_id)
            .join("manifest.json"),
    )?;
    if parent.policy.id != job.policy.id || parent.policy.version != job.policy.version {
        bail!("capture continue Policy must match the parent Capture Policy");
    }
    let parent_source = normalized_web_url(&parent.source.locator)?;
    let mut checkpoint = job
        .checkpoint
        .clone()
        .unwrap_or_else(|| ResolutionCheckpoint {
            queue: parent
                .discoveries
                .iter()
                .filter(|discovery| {
                    (job.operation.discovery_ids().is_empty()
                        && discovery.status == "skipped_budget")
                        || job
                            .operation
                            .discovery_ids()
                            .iter()
                            .any(|id| id == &discovery.id)
                })
                .map(|discovery| QueuedDiscovery {
                    discovery_id: discovery.id.clone(),
                    source: discovery.source.clone(),
                    depth: 1,
                    order: discovery.order,
                    order_path: vec![discovery.order],
                    ancestors: vec![parent_source.clone()],
                    kind: discovery.kind.clone(),
                })
                .collect(),
            attempted_sources: 0,
            downloaded_bytes: 0,
        });
    sort_resolution_queue(&mut checkpoint.queue);
    let mut partial = false;
    let mut child_capture_ids = job.child_capture_ids.clone();
    while let Some(discovery) = checkpoint.queue.first().cloned() {
        if read_job(&job.id)?.state == "cancelled" {
            return Ok(());
        }
        if discovery.depth > job.policy.snapshot.limits.depth_limit {
            append_discovery_event(
                parent_capture_id,
                &job.id,
                &discovery,
                "skipped_policy",
                Some(json!({"reason": "depth_limit"})),
            )?;
            partial = true;
            persist_resolution_progress(&job.id, &mut checkpoint, &discovery, None)?;
            continue;
        }
        if checkpoint.attempted_sources >= job.policy.snapshot.limits.source_limit {
            append_discovery_event(
                parent_capture_id,
                &job.id,
                &discovery,
                "skipped_budget",
                None,
            )?;
            partial = true;
            persist_resolution_progress(&job.id, &mut checkpoint, &discovery, None)?;
            continue;
        }
        checkpoint.attempted_sources = checkpoint.attempted_sources.saturating_add(1);
        let normalized_source = normalized_web_url(&discovery.source)?;
        if discovery
            .ancestors
            .iter()
            .any(|ancestor| ancestor == &normalized_source)
        {
            append_discovery_event(parent_capture_id, &job.id, &discovery, "cycle", None)?;
            partial = true;
            persist_resolution_progress(&job.id, &mut checkpoint, &discovery, None)?;
            continue;
        }
        let staging = job_staging_dir(&job.id)?;
        let deadline = UNIX_EPOCH
            .checked_add(Duration::from_secs(
                job.created_at
                    .saturating_add(job.policy.snapshot.limits.duration_limit_secs),
            ))
            .context("calculating discovery resolution deadline")?;
        if discovery.kind.as_deref() == Some("web") {
            let publication = match publish_web_discovery(job, &discovery, &mut checkpoint.queue)? {
                WebDiscoveryPublication::Published(publication) => publication,
                WebDiscoveryPublication::Cycle => {
                    append_discovery_event(parent_capture_id, &job.id, &discovery, "cycle", None)?;
                    partial = true;
                    persist_resolution_progress(&job.id, &mut checkpoint, &discovery, None)?;
                    continue;
                }
            };
            match publication {
                Publication::Published {
                    capture_id,
                    partial: child_partial,
                    reused,
                } => {
                    append_discovery_event(
                        parent_capture_id,
                        &job.id,
                        &discovery,
                        if reused {
                            "reused"
                        } else if child_partial {
                            "failed"
                        } else {
                            "captured"
                        },
                        Some(
                            json!({"capture_id": capture_id, "reference": reference_for_capture(&capture_id)?, "final_url": discovery.source}),
                        ),
                    )?;
                    child_capture_ids.push(capture_id);
                    persist_resolution_progress(
                        &job.id,
                        &mut checkpoint,
                        &discovery,
                        child_capture_ids.last().map(String::as_str),
                    )?;
                    partial |= child_partial;
                }
                Publication::Cancelled => return Ok(()),
            }
            sort_resolution_queue(&mut checkpoint.queue);
            write_checkpoint(&job.id, &checkpoint)?;
            continue;
        }
        let download_limit = job
            .policy
            .snapshot
            .limits
            .download_byte_limit
            .saturating_sub(checkpoint.downloaded_bytes);
        if download_limit == 0 {
            append_discovery_event(
                parent_capture_id,
                &job.id,
                &discovery,
                "skipped_budget",
                None,
            )?;
            partial = true;
            persist_resolution_progress(&job.id, &mut checkpoint, &discovery, None)?;
            continue;
        }
        match crate::web::acquire_binary(
            &discovery.source,
            &staging,
            deadline,
            job.policy.snapshot.limits.disk_byte_limit,
            download_limit,
            || Ok(read_job(&job.id)?.state == "cancelled"),
        ) {
            Ok(acquired) => {
                checkpoint.downloaded_bytes = checkpoint
                    .downloaded_bytes
                    .saturating_add(acquired.size_bytes);
                let normalized_final_url = normalized_web_url(&acquired.final_url)?;
                if discovery
                    .ancestors
                    .iter()
                    .any(|ancestor| ancestor == &normalized_final_url)
                {
                    append_discovery_event(
                        parent_capture_id,
                        &job.id,
                        &discovery,
                        "cycle",
                        Some(
                            json!({"requested_url": discovery.source, "final_url": acquired.final_url, "redirect_chain": acquired.redirect_chain}),
                        ),
                    )?;
                    cleanup_binary_staging(&staging)?;
                    partial = true;
                    persist_resolution_progress(&job.id, &mut checkpoint, &discovery, None)?;
                    continue;
                }
                if is_html_discovery(&acquired.path, &acquired.mime)? {
                    let publication = publish_web_discovery(
                        job,
                        &QueuedDiscovery {
                            source: acquired.final_url.clone(),
                            kind: Some("web".to_string()),
                            ..discovery.clone()
                        },
                        &mut checkpoint.queue,
                    )?;
                    cleanup_binary_staging(&staging)?;
                    match publication {
                        WebDiscoveryPublication::Cycle => {
                            append_discovery_event(
                                parent_capture_id,
                                &job.id,
                                &discovery,
                                "cycle",
                                Some(
                                    json!({"requested_url": discovery.source, "final_url": acquired.final_url}),
                                ),
                            )?;
                            partial = true;
                            persist_resolution_progress(
                                &job.id,
                                &mut checkpoint,
                                &discovery,
                                None,
                            )?;
                        }
                        WebDiscoveryPublication::Published(Publication::Published {
                            capture_id,
                            partial: child_partial,
                            reused,
                        }) => {
                            append_discovery_event(
                                parent_capture_id,
                                &job.id,
                                &discovery,
                                if reused {
                                    "reused"
                                } else if child_partial {
                                    "failed"
                                } else {
                                    "captured"
                                },
                                Some(
                                    json!({"capture_id": capture_id, "reference": reference_for_capture(&capture_id)?, "requested_url": discovery.source, "final_url": acquired.final_url, "mime": acquired.mime, "sha256": acquired.sha256, "size_bytes": acquired.size_bytes, "redirect_chain": acquired.redirect_chain}),
                                ),
                            )?;
                            child_capture_ids.push(capture_id);
                            persist_resolution_progress(
                                &job.id,
                                &mut checkpoint,
                                &discovery,
                                child_capture_ids.last().map(String::as_str),
                            )?;
                            partial |= child_partial;
                        }
                        WebDiscoveryPublication::Published(Publication::Cancelled) => return Ok(()),
                    }
                    sort_resolution_queue(&mut checkpoint.queue);
                    write_checkpoint(&job.id, &checkpoint)?;
                    continue;
                }
                let Some(extension) = binary_extension(&acquired.path, &acquired.mime)? else {
                    append_discovery_event(
                        parent_capture_id,
                        &job.id,
                        &discovery,
                        "unsupported",
                        Some(
                            json!({"requested_url": discovery.source, "final_url": acquired.final_url, "mime": acquired.mime, "sha256": acquired.sha256, "size_bytes": acquired.size_bytes, "redirect_chain": acquired.redirect_chain}),
                        ),
                    )?;
                    cleanup_binary_staging(&staging)?;
                    partial = true;
                    persist_resolution_progress(&job.id, &mut checkpoint, &discovery, None)?;
                    continue;
                };
                let typed_path = acquired.path.with_extension(extension);
                fs::rename(&acquired.path, &typed_path)
                    .context("typing acquired binary staging file")?;
                match publication::publish(
                    job,
                    &RemoteBinaryAcquisition {
                        job,
                        source: &typed_path,
                        source_url: &acquired.final_url,
                        sha256: &acquired.sha256,
                        provenance: RemoteProvenance {
                            requested_url: discovery.source.clone(),
                            final_url: acquired.final_url.clone(),
                            mime: acquired.mime.clone(),
                            sha256: acquired.sha256.clone(),
                            size_bytes: acquired.size_bytes,
                            redirect_chain: acquired.redirect_chain.clone(),
                            canonical_url: None,
                            platform: None,
                            post_id: None,
                            author: None,
                            published_at: None,
                            media: Vec::new(),
                        },
                    },
                )? {
                    Publication::Published {
                        capture_id,
                        partial: child_partial,
                        reused,
                    } => {
                        append_discovery_event(
                            parent_capture_id,
                            &job.id,
                            &discovery,
                            if reused {
                                "reused"
                            } else if child_partial {
                                "failed"
                            } else {
                                "captured"
                            },
                            Some(
                                json!({"capture_id": capture_id, "reference": reference_for_capture(&capture_id)?, "requested_url": discovery.source, "final_url": acquired.final_url, "mime": acquired.mime, "sha256": acquired.sha256, "size_bytes": acquired.size_bytes, "redirect_chain": acquired.redirect_chain}),
                            ),
                        )?;
                        child_capture_ids.push(capture_id);
                        persist_resolution_progress(
                            &job.id,
                            &mut checkpoint,
                            &discovery,
                            child_capture_ids.last().map(String::as_str),
                        )?;
                        partial |= child_partial;
                    }
                    Publication::Cancelled => return Ok(()),
                }
                cleanup_binary_staging(&staging)?;
                write_checkpoint(&job.id, &checkpoint)?;
            }
            Err(error) => {
                append_discovery_event(
                    parent_capture_id,
                    &job.id,
                    &discovery,
                    "failed",
                    Some(json!({"message": format!("{error:#}")})),
                )?;
                if read_job(&job.id)?.state == "cancelled" {
                    return Ok(());
                }
                partial = true;
                persist_resolution_progress(&job.id, &mut checkpoint, &discovery, None)?;
            }
        }
    }
    complete_continuation_job(&job.id, child_capture_ids, partial)
}

enum WebDiscoveryPublication {
    Published(Publication),
    Cycle,
}

fn publish_web_discovery(
    job: &Job,
    discovery: &QueuedDiscovery,
    queue: &mut Vec<QueuedDiscovery>,
) -> Result<WebDiscoveryPublication> {
    let publication = match publication::publish(
        job,
        &web_capture::WebAcquisition {
            job,
            url: &discovery.source,
            renderer: None,
            forbidden_final_urls: &discovery.ancestors,
        },
    ) {
        Ok(publication) => publication,
        Err(error) if format!("{error:#}").contains(web_capture::FINAL_URL_CYCLE) => {
            return Ok(WebDiscoveryPublication::Cycle);
        }
        Err(error) => return Err(error),
    };
    if let Publication::Published { capture_id, .. } = &publication {
        let child: Manifest = read_json(&captures_dir()?.join(capture_id).join("manifest.json"))?;
        let mut ancestors = discovery.ancestors.clone();
        ancestors.push(normalized_web_url(&child.source.locator)?);
        let mut parent_order_path = discovery.order_path.clone();
        if parent_order_path.is_empty() {
            parent_order_path.push(discovery.order);
        }
        queue.extend(child.discoveries.into_iter().filter_map(|child| {
            (child.status == "inventoried" || child.status == "skipped_budget").then_some(
                QueuedDiscovery {
                    discovery_id: child.id,
                    source: child.source,
                    depth: discovery.depth.saturating_add(1),
                    order: child.order,
                    order_path: {
                        let mut order_path = parent_order_path.clone();
                        order_path.push(child.order);
                        order_path
                    },
                    ancestors: ancestors.clone(),
                    kind: child.kind,
                },
            )
        }));
    }
    Ok(WebDiscoveryPublication::Published(publication))
}

fn sort_resolution_queue(queue: &mut [QueuedDiscovery]) {
    queue.sort_by(|left, right| {
        left.depth.cmp(&right.depth).then_with(|| {
            if left.order_path.is_empty() && right.order_path.is_empty() {
                left.order.cmp(&right.order)
            } else {
                left.order_path.cmp(&right.order_path)
            }
        })
    });
}

fn normalized_mime(mime: &str) -> String {
    mime.split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn cleanup_binary_staging(staging: &Path) -> Result<()> {
    let path = staging.join("binary-acquisition");
    fs::remove_dir_all(&path).with_context(|| format!("cleaning binary staging {}", path.display()))
}

fn normalized_web_url(value: &str) -> Result<String> {
    let mut url = Url::parse(value).with_context(|| format!("parsing Web URL `{value}`"))?;
    url.set_fragment(None);
    Ok(url.to_string())
}

fn reference_for_capture(capture_id: &str) -> Result<Reference> {
    let manifest: Manifest = read_json(&captures_dir()?.join(capture_id).join("manifest.json"))?;
    Ok(reference_for(&manifest))
}

fn binary_magic(path: &Path) -> Result<Option<&'static str>> {
    let mut file = File::open(path)
        .with_context(|| format!("opening binary MIME probe {}", path.display()))?;
    let mut bytes = [0_u8; 16];
    let size = file.read(&mut bytes).context("reading binary MIME probe")?;
    let bytes = bytes.get(..size).context("slicing binary MIME probe")?;
    Ok(if bytes.starts_with(b"%PDF-") {
        Some("application/pdf")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.get(..4) == Some(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if bytes.get(..4) == Some(b"II*\0") || bytes.get(..4) == Some(b"MM\0*") {
        Some("image/tiff")
    } else if bytes.get(..3) == Some(b"ID3") || has_mp3_frame_header(bytes) {
        Some("audio/mpeg")
    } else if bytes.get(..4) == Some(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        Some("audio/wav")
    } else if bytes.get(..4) == Some(b"OggS") {
        Some("audio/ogg")
    } else if bytes.get(4..8) == Some(b"ftyp") {
        Some("video/mp4")
    } else if bytes.get(..4) == Some(&[0x1a, 0x45, 0xdf, 0xa3]) {
        Some("video/webm")
    } else {
        None
    })
}

const fn has_mp3_frame_header(bytes: &[u8]) -> bool {
    let Some((&first, remaining)) = bytes.split_first() else {
        return false;
    };
    let Some((&second, remaining)) = remaining.split_first() else {
        return false;
    };
    let Some(&third) = remaining.first() else {
        return false;
    };
    first == 0xff
        && second & 0xe0 == 0xe0
        && second & 0x18 != 0x08
        && second & 0x06 != 0
        && third & 0xf0 != 0
        && third & 0xf0 != 0xf0
        && third & 0x0c != 0x0c
}

fn is_html_discovery(path: &Path, mime: &str) -> Result<bool> {
    Ok(normalized_mime(mime) == "text/html" && binary_magic(path)?.is_none())
}

fn binary_extension(path: &Path, mime: &str) -> Result<Option<&'static str>> {
    let declared = normalized_mime(mime);
    let magic = binary_magic(path)?;
    if magic != Some(declared.as_str()) {
        return Ok(None);
    }
    Ok(match declared.as_str() {
        "application/pdf" => Some("pdf"),
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/tiff" => Some("tiff"),
        "audio/mpeg" => Some("mp3"),
        "audio/mp4" => Some("m4a"),
        "audio/ogg" => Some("ogg"),
        "audio/wav" => Some("wav"),
        "video/mp4" => Some("mp4"),
        "video/webm" => Some("webm"),
        _ => None,
    })
}

impl JobOperation {
    fn discovery_ids(&self) -> &[String] {
        match self {
            Self::Continue { discovery_ids, .. } => discovery_ids,
            Self::Capture { .. } | Self::Derive { .. } => &[],
        }
    }
}

fn job_staging_dir(job_id: &str) -> Result<PathBuf> {
    let path = repository_dir()?.join("job-staging").join(job_id);
    fs::create_dir_all(&path)
        .with_context(|| format!("creating Job staging directory {}", path.display()))?;
    Ok(path)
}

fn write_checkpoint(job_id: &str, checkpoint: &ResolutionCheckpoint) -> Result<()> {
    let lock = lock_job(job_id)?;
    let mut job = read_job(job_id)?;
    job.checkpoint = Some(checkpoint.clone());
    write_job(&job)?;
    unlock_job(&lock)
}

fn persist_resolution_progress(
    job_id: &str,
    checkpoint: &mut ResolutionCheckpoint,
    completed: &QueuedDiscovery,
    child_capture_id: Option<&str>,
) -> Result<()> {
    let index = checkpoint
        .queue
        .iter()
        .position(|queued| queued.discovery_id == completed.discovery_id)
        .context("finding completed Discovery in continuation checkpoint")?;
    checkpoint.queue.remove(index);
    let lock = lock_job(job_id)?;
    let mut job = read_job(job_id)?;
    job.checkpoint = Some(checkpoint.clone());
    if let Some(capture_id) = child_capture_id
        && !job.child_capture_ids.iter().any(|id| id == capture_id)
    {
        job.child_capture_ids.push(capture_id.to_string());
    }
    write_job(&job)?;
    unlock_job(&lock)
}

fn append_discovery_event(
    capture_id: &str,
    job_id: &str,
    discovery: &QueuedDiscovery,
    status: &str,
    details: Option<Value>,
) -> Result<()> {
    let lock = lock_capture_ledger(capture_id)?;
    let ledger_path = captures_dir()?.join(capture_id).join("ledger.jsonl");
    let events: Vec<LedgerEvent> = read_json_lines(&ledger_path)?;
    if events.iter().any(|event| {
        event.event == "discovery_resolved"
            && event.job_id == job_id
            && event
                .details
                .as_ref()
                .and_then(|details| details.get("discovery_id"))
                .and_then(Value::as_str)
                == Some(discovery.discovery_id.as_str())
    }) {
        return FileExt::unlock(&lock).context("unlocking Capture ledger");
    }
    let mut payload = serde_json::Map::new();
    payload.insert("discovery_id".to_string(), json!(discovery.discovery_id));
    payload.insert("status".to_string(), json!(status));
    if let Some(Value::Object(details)) = details {
        payload.extend(details);
    }
    append_json_line(
        &ledger_path,
        &LedgerEvent {
            event: "discovery_resolved".to_string(),
            at: now_secs(),
            job_id: job_id.to_string(),
            details: Some(Value::Object(payload)),
        },
    )?;
    FileExt::unlock(&lock).context("unlocking Capture ledger")
}

fn publish_capture(job: &Job) -> Result<Publication> {
    match admission::admit_job(job)? {
        admission::Source::Local(source) => publication::publish(
            job,
            &LocalAcquisition {
                job,
                source: &source,
            },
        ),
        admission::Source::Web(url) => selected_renderer(job).map_or_else(
            || {
                social_capture::classify_url(&url).map_or_else(
                    || {
                        publication::publish(
                            job,
                            &web_capture::WebAcquisition {
                                job,
                                url: &url,
                                renderer: None,
                                forbidden_final_urls: &[],
                            },
                        )
                    },
                    |platform| {
                        publication::publish(
                            job,
                            &social_capture::SocialAcquisition {
                                job,
                                url: &url,
                                platform,
                            },
                        )
                    },
                )
            },
            |renderer| {
                publication::publish(
                    job,
                    &web_capture::WebAcquisition {
                        job,
                        url: &url,
                        renderer: Some(renderer),
                        forbidden_final_urls: &[],
                    },
                )
            },
        ),
    }
}

const fn selected_renderer(job: &Job) -> Option<WebRenderer> {
    match job.operation {
        JobOperation::Capture { renderer } => renderer,
        JobOperation::Continue { .. } | JobOperation::Derive { .. } => None,
    }
}

struct LocalAcquisition<'a> {
    job: &'a Job,
    source: &'a Path,
}

struct RemoteBinaryAcquisition<'a> {
    job: &'a Job,
    source: &'a Path,
    source_url: &'a str,
    sha256: &'a str,
    provenance: RemoteProvenance,
}

impl Acquisition for RemoteBinaryAcquisition<'_> {
    fn source_hash(&self, _staging: &Path) -> Result<AcquisitionResult<String>> {
        Ok(AcquisitionResult::Ready(self.sha256.to_string()))
    }

    fn acquire(
        &self,
        capture: &publication::StagedCapture,
        source_hash: &str,
    ) -> Result<AcquisitionResult<PreparedCapture>> {
        let mut source_job = self.job.clone();
        source_job.source = self.source.to_string_lossy().into_owned();
        LocalAcquisition {
            job: &source_job,
            source: self.source,
        }
        .acquire(capture, source_hash)
        .map(|result| match result {
            AcquisitionResult::Ready(prepared) => AcquisitionResult::Ready(
                prepared
                    .with_remote_provenance(self.source_url.to_string(), self.provenance.clone()),
            ),
            AcquisitionResult::Cancelled => AcquisitionResult::Cancelled,
        })
    }
}

impl LocalAcquisition<'_> {
    fn cancelled<T>(
        budget: &ResourceBudget<'_>,
        result: Result<T>,
    ) -> Result<AcquisitionResult<T>> {
        match result {
            Ok(value) => Ok(AcquisitionResult::Ready(value)),
            Err(_error) if budget.is_cancelled()? => Ok(AcquisitionResult::Cancelled),
            Err(error) => Err(error),
        }
    }
}

impl Acquisition for LocalAcquisition<'_> {
    fn source_hash(&self, staging: &Path) -> Result<AcquisitionResult<String>> {
        if 1 > self.job.policy.snapshot.limits.source_limit {
            bail!("local Source exceeds safe-local@1 acquisition budget");
        }
        let metadata = fs::metadata(self.source)
            .with_context(|| format!("reading local Source metadata {}", self.source.display()))?;
        if metadata.len() > self.job.policy.snapshot.limits.disk_byte_limit {
            bail!("local Source exceeds safe-local@1 disk budget");
        }
        let is_cancelled = || Ok(read_job(&self.job.id)?.state == "cancelled");
        let budget = ResourceBudget::new(
            staging,
            self.job.created_at,
            self.job.policy.snapshot.limits.duration_limit_secs,
            self.job.policy.snapshot.limits.disk_byte_limit,
        )
        .with_cancellation(&is_cancelled);
        Self::cancelled(&budget, sha256_file_limited(self.source, &budget))
    }

    fn acquire(
        &self,
        capture: &publication::StagedCapture,
        source_hash: &str,
    ) -> Result<AcquisitionResult<PreparedCapture>> {
        fs::create_dir_all(capture.staging().join("proofs")).with_context(|| {
            format!(
                "creating Capture staging directory {}",
                capture.staging().display()
            )
        })?;
        let is_cancelled = || Ok(read_job(&self.job.id)?.state == "cancelled");
        let budget = ResourceBudget::new(
            capture.staging(),
            self.job.created_at,
            self.job.policy.snapshot.limits.duration_limit_secs,
            self.job.policy.snapshot.limits.disk_byte_limit,
        )
        .with_cancellation(&is_cancelled);
        let proof_path = capture.staging().join("proofs/source");
        let copy = copy_file_limited(self.source, &proof_path, &budget)
            .with_context(|| format!("copying local Source {}", self.source.display()));
        match Self::cancelled(&budget, copy)? {
            AcquisitionResult::Ready(()) => {}
            AcquisitionResult::Cancelled => return Ok(AcquisitionResult::Cancelled),
        }
        let proof_created_at = now_secs();
        let proof_size = fs::metadata(&proof_path)
            .context("reading copied Proof metadata")?
            .len();
        let proof_hash = match Self::cancelled(&budget, sha256_file_limited(&proof_path, &budget))?
        {
            AcquisitionResult::Ready(hash) => hash,
            AcquisitionResult::Cancelled => return Ok(AcquisitionResult::Cancelled),
        };
        let mut manifest = Manifest {
            format_version: MANIFEST_FORMAT_VERSION,
            capture_id: capture.capture_id().to_string(),
            capture_version: 1,
            source: SourceIdentity {
                locator: self.job.source.clone(),
                sha256: source_hash.to_string(),
            },
            policy: self.job.policy.clone(),
            published_at: now_secs(),
            proof: Proof {
                artifact_id: "proof-source".to_string(),
                path: "proofs/source".to_string(),
                mime: mime_for_source(self.source).to_string(),
                sha256: proof_hash,
                size_bytes: proof_size,
                locator: Locator::File,
                created_at: proof_created_at,
                order: None,
            },
            extractions: Vec::new(),
            capabilities: Vec::new(),
            artifacts: Vec::new(),
            discoveries: Vec::new(),
            remote_provenance: None,
        };
        if is_media_source(self.source) {
            capture_local_media(
                self.job,
                &proof_path,
                capture.staging(),
                &mut manifest,
                &budget,
                None,
            );
        } else if is_document_source(self.source) {
            capture_local_document(
                self.job,
                &proof_path,
                capture.staging(),
                &mut manifest,
                &budget,
            );
        }
        if budget.is_cancelled()? {
            return Ok(AcquisitionResult::Cancelled);
        }
        let partial = manifest
            .capabilities
            .iter()
            .any(|capability| capability.state != "succeeded");
        Ok(AcquisitionResult::Ready(PreparedCapture::new(
            manifest, partial,
        )))
    }
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
    budget: &ResourceBudget<'_>,
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
                capability: None,
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
    budget: &ResourceBudget<'_>,
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
    budget: &ResourceBudget<'_>,
) -> std::result::Result<(), DocumentFailure> {
    const CAPABILITY: &str = "image-ocr";
    let (extraction, provider) = ocr_image(
        job,
        staging,
        OcrTarget {
            image_path: proof_path,
            relative_path: "extractions/ocr.txt",
            artifact_id: "extraction-image-ocr",
            proof_artifact_id: "proof-source",
        },
        budget,
    )
    .map_err(|error| {
        let error = *error;
        document_failure(CAPABILITY, error.provider, error.code, error.error)
    })?;
    manifest.extractions.push(extraction);
    manifest.capabilities.push(Capability {
        name: CAPABILITY.to_string(),
        state: "succeeded".to_string(),
        provider,
        error: None,
    });
    Ok(())
}

struct OcrFailure {
    provider: Provider,
    code: &'static str,
    error: anyhow::Error,
}

#[derive(Clone, Copy)]
struct OcrTarget<'a> {
    image_path: &'a Path,
    relative_path: &'a str,
    artifact_id: &'a str,
    proof_artifact_id: &'a str,
}

const fn ocr_extraction_failure(provider: Provider, error: anyhow::Error) -> OcrFailure {
    OcrFailure {
        provider,
        code: "extraction_failed",
        error,
    }
}

fn ocr_image(
    job: &Job,
    staging: &Path,
    target: OcrTarget<'_>,
    budget: &ResourceBudget<'_>,
) -> std::result::Result<(Extraction, Provider), Box<OcrFailure>> {
    if !policy_allows(&job.policy, "tesseract") {
        return Err(Box::new(OcrFailure {
            provider: unresolved_provider("tesseract"),
            code: "provider_not_allowed",
            error: denied_provider_error("tesseract"),
        }));
    }
    let provider =
        document_provider("tesseract", json!({ "format": "tsv" }), budget).map_err(|error| {
            Box::new(ocr_extraction_failure(
                unresolved_provider("tesseract"),
                error,
            ))
        })?;
    let mut command = Command::new("tesseract");
    command.args([target.image_path, Path::new("stdout"), Path::new("tsv")]);
    let output = budget
        .output(&mut command)
        .map_err(|error| Box::new(ocr_extraction_failure(provider.clone(), error)))?;
    provider_succeeded("tesseract", &output)
        .map_err(|error| Box::new(ocr_extraction_failure(provider.clone(), error)))?;
    let (text, regions) = parse_tesseract_tsv(&output.stdout)
        .map_err(|error| Box::new(ocr_extraction_failure(provider.clone(), error)))?;
    let output_path = staging.join(target.relative_path);
    let output_dir = output_path
        .parent()
        .context("resolving OCR Extraction directory")
        .map_err(|error| Box::new(ocr_extraction_failure(provider.clone(), error)))?;
    fs::create_dir_all(output_dir)
        .with_context(|| format!("creating OCR Extraction directory {}", output_dir.display()))
        .map_err(|error| Box::new(ocr_extraction_failure(provider.clone(), error)))?;
    budget
        .check_disk_capacity(
            u64::try_from(text.len())
                .context("converting OCR output size")
                .map_err(|error| Box::new(ocr_extraction_failure(provider.clone(), error)))?,
        )
        .map_err(|error| Box::new(ocr_extraction_failure(provider.clone(), error)))?;
    fs::write(&output_path, text)
        .with_context(|| format!("writing OCR Extraction {}", output_path.display()))
        .map_err(|error| ocr_extraction_failure(provider.clone(), error))?;
    let mut extraction = document_extraction(
        target.artifact_id,
        target.relative_path,
        &output_path,
        Locator::ImageRegions { regions },
        None,
        provider.clone(),
        budget,
    )
    .map_err(|error| Box::new(ocr_extraction_failure(provider.clone(), error)))?;
    extraction.proof_artifact_id = target.proof_artifact_id.to_string();
    Ok((extraction, provider))
}

pub fn enrich_image_ocr(
    job: &Job,
    staging: &Path,
    manifest: &mut Manifest,
    budget: &ResourceBudget<'_>,
) -> Result<()> {
    let already_extracted = |artifact_id: &str| {
        manifest
            .extractions
            .iter()
            .any(|extraction| extraction.artifact_id == format!("{artifact_id}-ocr"))
    };
    let mut images = manifest
        .artifacts
        .iter()
        .filter(|artifact| {
            artifact.mime.starts_with("image/") && !already_extracted(&artifact.artifact_id)
        })
        .map(|artifact| {
            (
                artifact.artifact_id.clone(),
                artifact.path.clone(),
                artifact.artifact_id.clone(),
            )
        })
        .collect::<Vec<_>>();
    images.extend(
        manifest
            .extractions
            .iter()
            .filter(|artifact| {
                artifact.mime.starts_with("image/") && !already_extracted(&artifact.artifact_id)
            })
            .map(|artifact| {
                (
                    artifact.artifact_id.clone(),
                    artifact.path.clone(),
                    artifact.artifact_id.clone(),
                )
            }),
    );
    for (source_artifact_id, source_path, proof_artifact_id) in images {
        if budget.is_cancelled()? {
            return Ok(());
        }
        let capability = format!("image-ocr-{source_artifact_id}");
        let artifact_id = format!("{source_artifact_id}-ocr");
        let relative_path = format!("extractions/ocr/{source_artifact_id}.txt");
        match ocr_image(
            job,
            staging,
            OcrTarget {
                image_path: &staging.join(source_path),
                relative_path: &relative_path,
                artifact_id: &artifact_id,
                proof_artifact_id: &proof_artifact_id,
            },
            budget,
        ) {
            Ok((extraction, provider)) => {
                manifest.extractions.push(extraction);
                manifest.capabilities.push(Capability {
                    name: capability,
                    state: "succeeded".to_string(),
                    provider,
                    error: None,
                });
            }
            Err(error) => {
                let error = *error;
                manifest.capabilities.push(Capability {
                    name: capability,
                    state: if error.code == "provider_not_allowed" {
                        "not_attempted".to_string()
                    } else {
                        "failed".to_string()
                    },
                    provider: error.provider,
                    error: Some(StructuredError {
                        code: error.code.to_string(),
                        message: format!("{:#}", error.error),
                        capability: None,
                    }),
                });
            }
        }
    }
    Ok(())
}

fn document_provider(
    name: &str,
    parameters: Value,
    budget: &ResourceBudget<'_>,
) -> Result<Provider> {
    provider(name, parameters, &[], budget)
}

fn pdf_page_count(proof_path: &Path, budget: &ResourceBudget<'_>) -> Result<u32> {
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
    budget: &ResourceBudget<'_>,
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
    budget: &ResourceBudget<'_>,
    media_order: Option<u32>,
) {
    let config = match Config::load().context("loading configuration for local media Capture") {
        Ok(config) => config,
        Err(error) => {
            record_media_setup_failure(manifest, &error);
            return;
        }
    };
    let work_dir = media_order.map_or_else(
        || staging.join("work"),
        |order| staging.join(format!("media-work-{order}")),
    );
    if let Err(error) = fs::create_dir_all(&work_dir)
        .with_context(|| format!("creating media work directory {}", work_dir.display()))
    {
        record_media_setup_failure(manifest, &error);
        return;
    }
    let target = MediaExtractionTarget {
        staging,
        work_dir: &work_dir,
        order: media_order,
    };
    let audio_allowed = policy_allows(&job.policy, "ffmpeg");
    let transcription_allowed = audio_allowed && policy_allows(&job.policy, "whisper-cli");
    if audio_allowed {
        if let Err(error) = capture_transcription(
            proof_path,
            manifest,
            &config,
            budget,
            transcription_allowed,
            &target,
        ) {
            cleanup_media_extraction(staging, "transcription", manifest, media_order);
            record_capability_failure(
                manifest,
                &media_capability_name(media_order, "transcription"),
                &error,
            );
        }
    } else {
        let error = denied_provider_error("ffmpeg");
        manifest.capabilities.push(blocked_capability(
            &media_capability_name(media_order, "audio-extraction"),
            unresolved_provider("ffmpeg"),
            &error,
        ));
        manifest.capabilities.push(blocked_capability(
            &media_capability_name(media_order, "transcription"),
            unresolved_provider("ffmpeg"),
            &error,
        ));
    }
    if !enforce_media_limits(
        budget,
        staging,
        manifest,
        &media_capability_name(media_order, "transcription"),
        media_order,
    ) {
        cleanup_work_dir(&work_dir, manifest);
        return;
    }
    if let Some(provider) = first_denied_provider(&job.policy, &["ffmpeg", "ffprobe"]) {
        let error = denied_provider_error(provider);
        manifest.capabilities.push(blocked_capability(
            &media_capability_name(media_order, "frames"),
            unresolved_provider(provider),
            &error,
        ));
    } else if let Err(error) = capture_frames(
        proof_path,
        staging,
        manifest,
        &work_dir,
        &config,
        budget,
        media_order,
    ) {
        cleanup_media_extraction(staging, "frames", manifest, media_order);
        record_capability_failure(
            manifest,
            &media_capability_name(media_order, "frames"),
            &error,
        );
    }
    enforce_media_limits(
        budget,
        staging,
        manifest,
        &media_capability_name(media_order, "frames"),
        media_order,
    );
    if let Err(error) = enrich_image_ocr(job, staging, manifest, budget) {
        record_image_ocr_cancellation_failure(manifest, &error);
    }
    cleanup_work_dir(&work_dir, manifest);
}

fn record_image_ocr_cancellation_failure(manifest: &mut Manifest, error: &anyhow::Error) {
    manifest.capabilities.push(failed_capability(
        "image-ocr-cancellation-check",
        unresolved_provider("resource-budget"),
        error,
    ));
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

fn cleanup_media_extraction(
    staging: &Path,
    capability: &str,
    manifest: &mut Manifest,
    media_order: Option<u32>,
) {
    let path = media_extraction_path(staging, capability, media_order);
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

fn media_capability_name(media_order: Option<u32>, capability: &str) -> String {
    media_order.map_or_else(
        || capability.to_string(),
        |order| format!("media-{order}-{capability}"),
    )
}

fn media_extraction_path(staging: &Path, capability: &str, media_order: Option<u32>) -> PathBuf {
    let directory = media_order.map_or_else(
        || staging.join("extractions"),
        |order| staging.join("extractions").join(format!("media-{order}")),
    );
    if capability == "frames" {
        directory.join("frames")
    } else {
        directory.join("transcription.txt")
    }
}

fn media_extraction_relative(capability: &str, media_order: Option<u32>) -> String {
    media_order.map_or_else(
        || format!("extractions/{capability}.txt"),
        |order| format!("extractions/media-{order}/{capability}.txt"),
    )
}

fn media_artifact_id(media_order: Option<u32>, kind: &str) -> String {
    media_order.map_or_else(
        || format!("extraction-{kind}"),
        |order| format!("extraction-media-{order}-{kind}"),
    )
}

struct MediaExtractionTarget<'a> {
    staging: &'a Path,
    work_dir: &'a Path,
    order: Option<u32>,
}

fn capture_transcription(
    proof_path: &Path,
    manifest: &mut Manifest,
    config: &Config,
    budget: &ResourceBudget<'_>,
    transcription_allowed: bool,
    target: &MediaExtractionTarget<'_>,
) -> Result<()> {
    let audio_path = target.work_dir.join("audio.wav");
    let audio_parameters = json!({
        "format": "pcm_s16le",
        "sample_rate_hz": 16_000,
        "channels": 1,
    });
    let audio_provider = match provider("ffmpeg", audio_parameters, &[], budget) {
        Ok(provider) => provider,
        Err(error) => {
            manifest.capabilities.push(failed_capability(
                &media_capability_name(target.order, "audio-extraction"),
                unresolved_provider_for(&error, "ffmpeg"),
                &error,
            ));
            manifest.capabilities.push(blocked_capability(
                &media_capability_name(target.order, "transcription"),
                unresolved_provider("whisper-cli"),
                &error,
            ));
            return Ok(());
        }
    };
    match audio::extract_audio_limited(proof_path, &audio_path, budget) {
        Ok(()) => {
            manifest.capabilities.push(Capability {
                name: media_capability_name(target.order, "audio-extraction"),
                state: "succeeded".to_string(),
                provider: audio_provider.clone(),
                error: None,
            });
            if transcription_allowed {
                capture_transcription_text(
                    manifest,
                    &audio_path,
                    config,
                    budget,
                    &audio_provider,
                    target,
                )?;
            } else {
                let error = denied_provider_error("whisper-cli");
                manifest.capabilities.push(blocked_capability(
                    &media_capability_name(target.order, "transcription"),
                    unresolved_provider("whisper-cli"),
                    &error,
                ));
            }
        }
        Err(error) => {
            manifest.capabilities.push(failed_capability(
                &media_capability_name(target.order, "audio-extraction"),
                audio_provider,
                &error,
            ));
            manifest.capabilities.push(blocked_capability(
                &media_capability_name(target.order, "transcription"),
                unresolved_provider("whisper-cli"),
                &error,
            ));
        }
    }
    Ok(())
}

fn capture_transcription_text(
    manifest: &mut Manifest,
    audio_path: &Path,
    config: &Config,
    budget: &ResourceBudget<'_>,
    audio_provider: &Provider,
    target: &MediaExtractionTarget<'_>,
) -> Result<()> {
    let transcription_provider = match transcription_parameters(config, audio_provider, budget)
        .and_then(|parameters| provider("whisper-cli", parameters, &[], budget))
    {
        Ok(provider) => provider,
        Err(error) => {
            manifest.capabilities.push(failed_capability(
                &media_capability_name(target.order, "transcription"),
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
        &target.work_dir.join("transcription"),
        budget,
    ) {
        Ok(transcription_path) => {
            let destination = media_extraction_path(target.staging, "transcription", target.order);
            copy_extraction(&transcription_path, &destination, budget)?;
            let mut extraction = extraction(
                &media_artifact_id(target.order, "transcription"),
                &media_extraction_relative("transcription", target.order),
                "text/plain",
                &destination,
                None,
                transcription_provider.clone(),
                budget,
            )?;
            if let Some(order) = target.order {
                extraction.proof_artifact_id = format!("media-{order}");
            }
            manifest.extractions.push(extraction);
            manifest.capabilities.push(Capability {
                name: media_capability_name(target.order, "transcription"),
                state: "succeeded".to_string(),
                provider: transcription_provider,
                error: None,
            });
        }
        Err(error) => manifest.capabilities.push(failed_capability(
            &media_capability_name(target.order, "transcription"),
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
    budget: &ResourceBudget<'_>,
    media_order: Option<u32>,
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
                &media_capability_name(media_order, "frames"),
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
            let destination = media_extraction_path(staging, "frames", media_order);
            if frame_count > 0 {
                let frame_ocr_provider = frame_ocr_provider(manifest, budget, media_order);
                let frame_ocr_succeeded = capture_frame_extractions(
                    &frames_dir,
                    &destination,
                    manifest,
                    &frames_provider,
                    frame_ocr_provider.as_ref(),
                    budget,
                    media_order,
                )?;
                if let Some(provider) = frame_ocr_provider.filter(|_| frame_ocr_succeeded) {
                    manifest.capabilities.push(Capability {
                        name: media_capability_name(media_order, "frame-ocr"),
                        state: "succeeded".to_string(),
                        provider,
                        error: None,
                    });
                }
            }
            manifest.capabilities.push(Capability {
                name: media_capability_name(media_order, "frames"),
                state: "succeeded".to_string(),
                provider: frames_provider,
                error: None,
            });
        }
        Err(error) => {
            manifest.capabilities.push(failed_capability(
                &media_capability_name(media_order, "frames"),
                frames_provider,
                &error,
            ));
        }
    }
    Ok(())
}

fn capture_frame_extractions(
    frames_dir: &Path,
    destination: &Path,
    manifest: &mut Manifest,
    frames_provider: &Provider,
    frame_ocr_provider: Option<&Provider>,
    budget: &ResourceBudget<'_>,
    media_order: Option<u32>,
) -> Result<bool> {
    budget.check()?;
    fs::create_dir_all(destination).with_context(|| {
        format!(
            "creating Frames Extraction directory {}",
            destination.display()
        )
    })?;
    let mut frames = fs::read_dir(frames_dir)
        .with_context(|| format!("reading Frames {}", frames_dir.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()
        .context("reading Frame paths")?;
    frames.sort();
    let mut frame_ocr_succeeded = true;
    for (index, frame) in frames.iter().enumerate() {
        budget.check()?;
        let filename = frame.file_name().context("reading Frame filename")?;
        let filename = filename.to_string_lossy();
        let timestamp = frame_timestamp(&filename)?;
        let extraction_path = destination.join(filename.as_ref());
        copy_extraction(frame, &extraction_path, budget)?;
        let artifact_id = media_order.map_or_else(
            || format!("extraction-frame-{index:04}"),
            |order| format!("extraction-media-{order}-frame-{index:04}"),
        );
        let path = media_order.map_or_else(
            || format!("extractions/frames/{filename}"),
            |order| format!("extractions/media-{order}/frames/{filename}"),
        );
        let mut extraction = extraction(
            &artifact_id,
            &path,
            "image/jpeg",
            &extraction_path,
            Some(Locator::MediaTimestamp {
                timestamps_secs: vec![timestamp],
            }),
            frames_provider.clone(),
            budget,
        )?;
        if let Some(order) = media_order {
            extraction.proof_artifact_id = format!("media-{order}");
        }
        manifest.extractions.push(extraction);
        if let Some(provider) = frame_ocr_provider {
            frame_ocr_succeeded &= capture_frame_ocr(
                &extraction_path,
                destination,
                index,
                timestamp,
                media_order,
                provider,
                manifest,
                budget,
            );
        }
    }
    Ok(frame_ocr_succeeded)
}

fn frame_ocr_provider(
    manifest: &mut Manifest,
    budget: &ResourceBudget<'_>,
    media_order: Option<u32>,
) -> Option<Provider> {
    if !policy_allows(&manifest.policy, "tesseract") {
        let error = denied_provider_error("tesseract");
        manifest.capabilities.push(blocked_capability(
            &media_capability_name(media_order, "frame-ocr"),
            unresolved_provider("tesseract"),
            &error,
        ));
        return None;
    }
    match provider("tesseract", json!({ "format": "tsv" }), &[], budget) {
        Ok(provider) => Some(provider),
        Err(error) => {
            manifest.capabilities.push(failed_capability(
                &media_capability_name(media_order, "frame-ocr"),
                unresolved_provider("tesseract"),
                &error,
            ));
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn capture_frame_ocr(
    frame_path: &Path,
    destination: &Path,
    frame_index: usize,
    timestamp: f64,
    media_order: Option<u32>,
    provider: &Provider,
    manifest: &mut Manifest,
    budget: &ResourceBudget<'_>,
) -> bool {
    let result = (|| -> Result<()> {
        let mut command = Command::new("tesseract");
        command.args([frame_path, Path::new("stdout"), Path::new("tsv")]);
        let output = budget.output(&mut command)?;
        provider_succeeded("tesseract", &output)?;
        let (text, regions) = parse_tesseract_tsv(&output.stdout)?;
        let output_path = destination.join(format!("frame-{frame_index:04}.ocr.txt"));
        budget.check_disk_capacity(
            u64::try_from(text.len()).context("converting OCR output size")?,
        )?;
        fs::write(&output_path, text)
            .with_context(|| format!("writing Frame OCR Extraction {}", output_path.display()))?;
        let artifact_id = media_order.map_or_else(
            || format!("extraction-frame-{frame_index:04}-ocr"),
            |order| format!("extraction-media-{order}-frame-{frame_index:04}-ocr"),
        );
        let path = media_order.map_or_else(
            || format!("extractions/frames/frame-{frame_index:04}.ocr.txt"),
            |order| format!("extractions/media-{order}/frames/frame-{frame_index:04}.ocr.txt"),
        );
        let mut extraction = extraction(
            &artifact_id,
            &path,
            "text/plain; charset=utf-8",
            &output_path,
            Some(Locator::FrameRegions {
                timestamps_secs: vec![timestamp],
                regions,
            }),
            provider.clone(),
            budget,
        )?;
        if let Some(order) = media_order {
            extraction.proof_artifact_id = format!("media-{order}");
        }
        manifest.extractions.push(extraction);
        Ok(())
    })();
    match result {
        Ok(()) => true,
        Err(error) => {
            manifest.capabilities.push(failed_capability(
                &media_capability_name(media_order, "frame-ocr"),
                provider.clone(),
                &error,
            ));
            false
        }
    }
}

fn failed_capability(name: &str, provider: Provider, error: &anyhow::Error) -> Capability {
    Capability {
        name: name.to_string(),
        state: "failed".to_string(),
        provider,
        error: Some(StructuredError {
            code: "capability_failed".to_string(),
            message: format!("{error:#}"),
            capability: None,
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
            capability: None,
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
            capability: None,
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
    budget: &ResourceBudget<'_>,
    staging: &Path,
    manifest: &mut Manifest,
    capability: &str,
    media_order: Option<u32>,
) -> bool {
    let result = budget.check();
    if let Err(error) = result {
        manifest.extractions.retain(|extraction| {
            let affected = match capability.rsplit('-').next() {
                Some("transcription") => {
                    extraction.artifact_id == media_artifact_id(media_order, "transcription")
                }
                Some("frames") => media_order.map_or_else(
                    || extraction.artifact_id.starts_with("extraction-frame-"),
                    |order| {
                        extraction
                            .artifact_id
                            .starts_with(&format!("extraction-media-{order}-frame-"))
                    },
                ),
                _ => false,
            };
            !affected
        });
        let base_capability = capability.rsplit('-').next().unwrap_or(capability);
        cleanup_media_extraction(staging, base_capability, manifest, media_order);
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

fn copy_extraction(source: &Path, destination: &Path, budget: &ResourceBudget<'_>) -> Result<()> {
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
    budget: &ResourceBudget<'_>,
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
    budget: &ResourceBudget<'_>,
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
    budget: &ResourceBudget<'_>,
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
            capability: match &job.operation {
                JobOperation::Derive { recipe, .. } => Some(recipe.kind.as_str().to_string()),
                _ => None,
            },
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

fn complete_continuation_job(
    job_id: &str,
    child_capture_ids: Vec<String>,
    partial: bool,
) -> Result<()> {
    let lock = lock_job(job_id)?;
    let mut job = read_job(job_id)?;
    if job.state != "cancelled" {
        job.state = if partial {
            "partial".to_string()
        } else {
            "succeeded".to_string()
        };
        job.updated_at = now_secs();
        job.worker_pid = None;
        job.child_capture_ids = child_capture_ids;
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
    let typed_code = error.chain().find_map(|cause| {
        cause
            .downcast_ref::<CodedError>()
            .map(|coded| coded.code.as_str().to_string())
    });
    let (code, capability) = match &job.operation {
        JobOperation::Derive { recipe, .. } => (
            typed_code.unwrap_or_else(|| "derive_provider_failed".to_string()),
            Some(recipe.kind.as_str().to_string()),
        ),
        _ => (error_code(error), None),
    };
    job.error = Some(StructuredError {
        code,
        message: format!("{error:#}"),
        capability,
    });
    write_job(&job)?;
    append_job_event(&job.id, "failed")?;
    unlock_job(&lock)
}

fn error_code(error: &anyhow::Error) -> String {
    error
        .chain()
        .find_map(|cause| {
            cause
                .downcast_ref::<CodedError>()
                .map(|error| error.code.as_str())
        })
        .unwrap_or("capture_failed")
        .to_string()
}

fn print_job(job_id: &str) -> Result<()> {
    let job = reconcile_interrupted(job_id)?;
    print_json(&job)
}

fn wait_for_job(job_id: &str, timeout_secs: u64) -> Result<()> {
    let deadline = SystemTime::now()
        .checked_add(Duration::from_secs(timeout_secs))
        .context("calculating Job wait deadline")?;
    loop {
        let job = reconcile_interrupted(job_id)?;
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
    let derivatives = derive::list(capture_id)?;
    print_json(&InspectedCapture {
        manifest,
        ledger,
        derivatives,
    })
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
    if let Some(cursor) = cursor {
        return print_json(&search_page(decode_search_cursor(cursor)?, query, limit)?);
    }
    if let Some(error) = read_index_degradation()? {
        return print_json(&AgentError { error });
    }
    let index = match Index::open_in_dir(search_index_path()?) {
        Ok(index) => index,
        Err(error) => {
            return print_json(&AgentError {
                error: StructuredError {
                    code: "index_unavailable".to_string(),
                    message: format!("search Index is unavailable: {error}"),
                    capability: None,
                },
            });
        }
    };
    let schema = index.schema();
    let fields = SearchFields {
        capture_id: schema
            .get_field("capture_id")
            .context("reading capture_id Search field")?,
        source: schema
            .get_field("source")
            .context("reading source Search field")?,
        source_sha256: schema
            .get_field("source_sha256")
            .context("reading source_sha256 Search field")?,
        published_at: schema
            .get_field("published_at")
            .context("reading published_at Search field")?,
        artifact_id: schema
            .get_field("artifact_id")
            .context("reading artifact_id Search field")?,
        artifact_sha256: schema
            .get_field("artifact_sha256")
            .context("reading artifact_sha256 Search field")?,
        artifact_locator: schema
            .get_field("artifact_locator")
            .context("reading artifact_locator Search field")?,
        text: schema
            .get_field("text")
            .context("reading text Search field")?,
    };
    let mut parser = QueryParser::for_index(&index, vec![fields.source, fields.text]);
    parser.set_conjunction_by_default();
    let parsed = parser
        .parse_query(query)
        .map_err(|error| anyhow::anyhow!(error))
        .context("parsing Search query")?;
    let reader = index
        .reader()
        .map_err(|error| anyhow::anyhow!(error))
        .context("opening Search reader")?;
    let searcher = reader.searcher();
    let hits = searcher
        .search(
            &parsed,
            &TopDocs::with_limit(SEARCH_MAX_CANDIDATES.saturating_add(1)).order_by_score(),
        )
        .map_err(|error| anyhow::anyhow!(error))
        .context("searching Captures")?;
    let mut matches = hits
        .into_iter()
        .map(|(score, address)| {
            let document: TantivyDocument = searcher
                .doc(address)
                .map_err(|error| anyhow::anyhow!(error))
                .context("reading Search result")?;
            Ok((score, search_summary_for(&document, &fields)?))
        })
        .collect::<Result<Vec<_>>>()?;
    matches.sort_unstable_by(|(left_score, left), (right_score, right)| {
        right_score
            .total_cmp(left_score)
            .then_with(|| left.capture_id.cmp(&right.capture_id))
            .then_with(|| left.reference.artifact_id.cmp(&right.reference.artifact_id))
    });
    let truncated = matches.len() > SEARCH_MAX_CANDIDATES;
    matches.truncate(SEARCH_MAX_CANDIDATES);
    let captures = matches
        .into_iter()
        .map(|(_, capture)| capture)
        .collect::<Vec<_>>();
    print_json(&search_page(
        SearchCursor {
            version: 1,
            query: query.to_string(),
            index_version: SEARCH_INDEX_VERSION,
            captures,
            position: 0,
            truncated,
        },
        query,
        limit,
    )?)
}

fn search_page(mut cursor: SearchCursor, query: &str, limit: usize) -> Result<SearchPage> {
    if cursor.version != 1
        || cursor.index_version != SEARCH_INDEX_VERSION
        || cursor.query != query
        || cursor.position > cursor.captures.len()
    {
        bail!("pagination cursor does not match this operation or snapshot");
    }
    let end = cursor
        .position
        .saturating_add(limit)
        .min(cursor.captures.len());
    let captures = cursor
        .captures
        .get(cursor.position..end)
        .context("reading bounded Search page")?
        .to_vec();
    let next_cursor = if end < cursor.captures.len() {
        cursor.position = end;
        Some(encode_search_cursor(&cursor)?)
    } else {
        None
    };
    Ok(SearchPage {
        captures,
        next_cursor,
        truncated: cursor.truncated,
    })
}

#[allow(clippy::too_many_lines)]
fn search_knowledge(query: &str, cursor: Option<&str>, limit: usize) -> Result<()> {
    validate_page_limit(limit)?;
    let tokens = knowledge_tokens(query);
    if tokens.is_empty() {
        return print_json(&AgentError {
            error: StructuredError {
                code: "invalid_request".to_string(),
                message: "knowledge search query must contain at least one token".to_string(),
                capability: None,
            },
        });
    }
    let normalized_query = tokens.join(" ");
    if let Some(cursor) = cursor {
        let cursor = decode_knowledge_cursor(cursor)?;
        let metadata = read_knowledge_metadata()?;
        if cursor.version != 1
            || cursor.query != normalized_query
            || cursor.snapshot != metadata.snapshot
            || cursor.position > cursor.results.len()
        {
            bail!("pagination cursor does not match this operation or snapshot");
        }
        return print_json(&knowledge_page(cursor, limit)?);
    }
    if let Some(error) = read_knowledge_degradation()? {
        return print_json(&AgentError { error });
    }
    let metadata = match read_knowledge_metadata() {
        Ok(metadata) => metadata,
        Err(error) => {
            return print_json(&AgentError {
                error: StructuredError {
                    code: "index_unavailable".to_string(),
                    message: format!("knowledge Index is unavailable: {error:#}"),
                    capability: None,
                },
            });
        }
    };
    if metadata.version != KNOWLEDGE_INDEX_VERSION
        || metadata.analyzer != KNOWLEDGE_ANALYZER_VERSION
    {
        return print_json(&AgentError {
            error: StructuredError {
                code: "index_degraded".to_string(),
                message: "knowledge Index has an unsupported format or analyzer".to_string(),
                capability: None,
            },
        });
    }
    let index = match Index::open_in_dir(knowledge_index_path()?) {
        Ok(index) => index,
        Err(error) => {
            return print_json(&AgentError {
                error: StructuredError {
                    code: "index_unavailable".to_string(),
                    message: format!("knowledge Index is unavailable: {error}"),
                    capability: None,
                },
            });
        }
    };
    register_knowledge_analyzer(&index);
    let fields = knowledge_fields(&index.schema())?;
    let query = knowledge_query(&fields, &tokens);
    let reader = index
        .reader()
        .map_err(|error| anyhow::anyhow!(error))
        .context("opening knowledge Index reader")?;
    let searcher = reader.searcher();
    let limit = usize::try_from(searcher.num_docs()).context("counting knowledge statements")?;
    let hits = searcher
        .search(&query, &TopDocs::with_limit(limit).order_by_score())
        .map_err(|error| anyhow::anyhow!(error))
        .context("searching knowledge statements")?;
    let mut results = hits
        .into_iter()
        .map(|(score, address)| {
            let document: TantivyDocument = searcher
                .doc(address)
                .map_err(|error| anyhow::anyhow!(error))
                .context("reading knowledge Search result")?;
            Ok((score, knowledge_result_for(&document, &fields)?))
        })
        .collect::<Result<Vec<_>>>()?;
    results.sort_unstable_by(|(left_score, left), (right_score, right)| {
        right_score
            .total_cmp(left_score)
            .then_with(|| knowledge_result_date(right).cmp(&knowledge_result_date(left)))
            .then_with(|| left.reference.artifact_id.cmp(&right.reference.artifact_id))
            .then_with(|| left.statement_id.cmp(&right.statement_id))
    });
    print_json(&knowledge_page(
        KnowledgeCursor {
            version: 1,
            query: normalized_query,
            snapshot: metadata.snapshot,
            position: 0,
            results: results.into_iter().map(|(_, result)| result).collect(),
        },
        limit,
    )?)
}

fn knowledge_page(mut cursor: KnowledgeCursor, limit: usize) -> Result<KnowledgePage> {
    let end = cursor
        .position
        .saturating_add(limit)
        .min(cursor.results.len());
    let results = cursor
        .results
        .get(cursor.position..end)
        .context("reading bounded knowledge Search page")?
        .to_vec();
    let next_cursor = if end < cursor.results.len() {
        cursor.position = end;
        Some(encode_knowledge_cursor(&cursor)?)
    } else {
        None
    };
    Ok(KnowledgePage {
        results,
        next_cursor,
    })
}

fn knowledge_result_date(result: &KnowledgeResult) -> (Option<&str>, u64) {
    (
        result.remote_published_at.as_deref(),
        result.capture_published_at,
    )
}

fn knowledge_query(fields: &KnowledgeFields, tokens: &[String]) -> BooleanQuery {
    let mut clauses = tokens
        .iter()
        .map(|token| {
            let query: Box<dyn tantivy::query::Query> = Box::new(TermQuery::new(
                Term::from_field_text(fields.text, token),
                IndexRecordOption::WithFreqsAndPositions,
            ));
            (Occur::Must, query)
        })
        .collect::<Vec<_>>();
    if tokens.len() > 1 {
        let phrase = PhraseQuery::new(
            tokens
                .iter()
                .map(|token| Term::from_field_text(fields.text, token))
                .collect(),
        );
        clauses.push((
            Occur::Should,
            Box::new(BoostQuery::new(Box::new(phrase), KNOWLEDGE_PHRASE_BOOST)),
        ));
    }
    BooleanQuery::new(clauses)
}

fn knowledge_tokens(value: &str) -> Vec<String> {
    let normalized = value
        .nfd()
        .filter(|character| !is_combining_mark(*character));
    let mut tokens = Vec::new();
    let mut token = String::new();
    for character in normalized.flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            token.push(character);
        } else if !token.is_empty() {
            tokens.push(std::mem::take(&mut token));
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
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
    let artifact = artifact_for(&manifest, &request.artifact_id)
        .or(derive::artifact_for(
            &request.capture_id,
            &request.artifact_id,
        )?)
        .context("unknown artifact identifier")?;
    if !is_text_mime(&artifact.mime) {
        bail!("binary artifacts cannot be read on stdout");
    }
    let path = derive::artifact_path(&directory, &artifact)?;
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
    let start = after
        .as_ref()
        .map(|cursor| {
            manifests
                .iter()
                .position(|manifest| manifest.capture_id == cursor.capture_id)
                .map(|position| position.saturating_add(1))
                .context("pagination cursor does not match this snapshot")
        })
        .transpose()?
        .unwrap_or(0);
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
    let manifests = list_manifests()?;
    let count = manifests.len();
    write_search_index(&manifests)?;
    clear_index_degradation()?;
    unlock_search_index(&lock)?;
    Ok(count)
}

fn rebuild_knowledge_index() -> Result<usize> {
    let lock = lock_knowledge_index()?;
    let result = (|| {
        let records = knowledge_card_records()?;
        let count = records.len();
        write_knowledge_index(&records)?;
        clear_knowledge_degradation()?;
        Ok(count)
    })();
    unlock_knowledge_index(&lock)?;
    result
}

fn knowledge_index_path() -> Result<PathBuf> {
    Ok(repository_dir()?.join("knowledge-index"))
}

fn knowledge_metadata_path() -> Result<PathBuf> {
    Ok(repository_dir()?.join("knowledge-index.json"))
}

fn knowledge_status_path() -> Result<PathBuf> {
    Ok(repository_dir()?.join("knowledge-index-status.json"))
}

fn lock_knowledge_index() -> Result<File> {
    let directory = repository_dir()?;
    fs::create_dir_all(&directory)
        .with_context(|| format!("creating knowledge Index directory {}", directory.display()))?;
    let path = directory.join("knowledge-index.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening knowledge Index lock {}", path.display()))?;
    file.lock_exclusive().context("locking knowledge Index")?;
    Ok(file)
}

fn unlock_knowledge_index(file: &File) -> Result<()> {
    FileExt::unlock(file).context("unlocking knowledge Index")
}

fn knowledge_card_records() -> Result<Vec<KnowledgeCardRecord>> {
    let mut records = Vec::new();
    for manifest in list_manifests()? {
        let derivatives_dir = captures_dir()?
            .join(&manifest.capture_id)
            .join("derivatives");
        if !derivatives_dir.try_exists().with_context(|| {
            format!(
                "checking Derivative directory {}",
                derivatives_dir.display()
            )
        })? {
            continue;
        }
        for entry in fs::read_dir(&derivatives_dir)
            .with_context(|| format!("listing Derivatives in {}", derivatives_dir.display()))?
        {
            let entry = entry.context("reading Derivative directory entry")?;
            if !entry
                .file_type()
                .context("reading Derivative entry type")?
                .is_dir()
                || entry.file_name().to_string_lossy().starts_with('.')
            {
                continue;
            }
            let derivative: KnowledgeDerivativeManifest =
                read_json(&entry.path().join("manifest.json"))?;
            if derivative.recipe.kind != "knowledge-card" {
                continue;
            }
            let artifact_path = knowledge_artifact_path(
                &captures_dir()?.join(&manifest.capture_id),
                &derivative.artifact.path,
            )?;
            let card: KnowledgeCard = read_json(&artifact_path)?;
            if card.format_version != 1 || card.recipe != "knowledge-card" {
                bail!("knowledge Index has an unsupported knowledge-card format");
            }
            let supersession_key = knowledge_supersession_key(&manifest, &derivative.recipe)?;
            records.push(KnowledgeCardRecord {
                derivative,
                statements: card.knowledge_core.statements,
                supersession_key,
                source_locator: manifest.source.locator.clone(),
                remote_published_at: manifest
                    .remote_provenance
                    .as_ref()
                    .and_then(|remote| remote.published_at.clone()),
                capture_published_at: manifest.published_at,
            });
        }
    }
    Ok(active_knowledge_cards(records))
}

fn knowledge_artifact_path(capture_dir: &Path, artifact_path: &str) -> Result<PathBuf> {
    let relative = Path::new(artifact_path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("invalid artifact path in knowledge Derivative manifest");
    }
    Ok(capture_dir.join(relative))
}

fn active_knowledge_cards(mut records: Vec<KnowledgeCardRecord>) -> Vec<KnowledgeCardRecord> {
    records.sort_unstable_by(|left, right| {
        left.supersession_key
            .cmp(&right.supersession_key)
            .then_with(|| left.derivative.created_at.cmp(&right.derivative.created_at))
            .then_with(|| {
                left.derivative
                    .reference
                    .artifact_id
                    .cmp(&right.derivative.reference.artifact_id)
            })
    });
    let mut active = HashMap::new();
    for record in records {
        active.insert(record.supersession_key.clone(), record);
    }
    active.into_values().collect()
}

fn knowledge_supersession_key(manifest: &Manifest, recipe: &KnowledgeRecipe) -> Result<String> {
    let source_identity = manifest
        .remote_provenance
        .as_ref()
        .and_then(|remote| {
            remote
                .platform
                .as_ref()
                .zip(remote.post_id.as_ref())
                .map(|(platform, post_id)| format!("social:{platform}:{post_id}"))
        })
        .unwrap_or_else(|| format!("source:{}", manifest.source.sha256));
    let mut proof_hashes = vec![manifest.proof.sha256.clone()];
    proof_hashes.extend(manifest.artifacts.iter().map(|proof| proof.sha256.clone()));
    proof_hashes.extend(
        manifest
            .extractions
            .iter()
            .map(|extraction| extraction.sha256.clone()),
    );
    proof_hashes.sort_unstable();
    let observed_state = sha256_bytes(
        &serde_json::to_vec(&proof_hashes).context("serializing knowledge observed state")?,
    );
    let recipe = serde_json::to_string(recipe).context("serializing knowledge Recipe")?;
    Ok(format!("{recipe}|{source_identity}|{observed_state}"))
}

fn write_knowledge_index(records: &[KnowledgeCardRecord]) -> Result<()> {
    let path = knowledge_index_path()?;
    let temporary = path.with_file_name(format!(".knowledge-index-{}", unique_id()));
    fs::create_dir_all(&temporary)
        .with_context(|| format!("creating knowledge Index {}", temporary.display()))?;
    let result = (|| {
        let mut schema_builder = Schema::builder();
        let text_options = TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(KNOWLEDGE_ANALYZER_VERSION)
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        );
        let text = schema_builder.add_text_field("text", text_options);
        let statement_text = schema_builder.add_text_field("statement_text", STORED);
        let statement_kind = schema_builder.add_text_field("statement_kind", STRING | STORED);
        let reference = schema_builder.add_text_field("reference", STORED);
        let statement_id = schema_builder.add_text_field("statement_id", STRING | STORED);
        let source_locator = schema_builder.add_text_field("source_locator", STORED);
        let remote_published_at = schema_builder.add_text_field("remote_published_at", STORED);
        let capture_published_at = schema_builder.add_u64_field("capture_published_at", STORED);
        let index = Index::create_in_dir(&temporary, schema_builder.build())
            .map_err(|error| anyhow::anyhow!(error))
            .context("creating knowledge Index")?;
        register_knowledge_analyzer(&index);
        let mut writer = index
            .writer(50_000_000)
            .map_err(|error| anyhow::anyhow!(error))
            .context("creating knowledge Index writer")?;
        for record in records {
            let remote_published_at_value =
                record.remote_published_at.as_deref().unwrap_or_default();
            let reference_json = serde_json::to_string(&record.derivative.reference)
                .context("serializing knowledge Card Reference")?;
            for statement in &record.statements {
                let text_tokens = knowledge_tokens(&statement.text);
                if text_tokens.is_empty() {
                    continue;
                }
                writer
                    .add_document(doc!(
                        text => text_tokens.join(" "),
                        statement_text => statement.text.clone(),
                        statement_kind => statement.kind.clone(),
                        reference => reference_json.clone(),
                        statement_id => statement.id.clone(),
                        source_locator => record.source_locator.clone(),
                        remote_published_at => remote_published_at_value,
                        capture_published_at => record.capture_published_at,
                    ))
                    .map_err(|error| anyhow::anyhow!(error))
                    .context("indexing knowledge statement")?;
            }
        }
        writer
            .commit()
            .map_err(|error| anyhow::anyhow!(error))
            .context("publishing knowledge Index")?;
        let metadata = KnowledgeIndexMetadata {
            version: KNOWLEDGE_INDEX_VERSION,
            analyzer: KNOWLEDGE_ANALYZER_VERSION.to_string(),
            snapshot: unique_id(),
        };
        if path
            .try_exists()
            .context("checking previous knowledge Index")?
        {
            let previous =
                path.with_file_name(format!(".knowledge-index-previous-{}", unique_id()));
            fs::rename(&path, &previous)
                .with_context(|| format!("staging previous knowledge Index {}", path.display()))?;
            fs::rename(&temporary, &path)
                .with_context(|| format!("publishing knowledge Index {}", path.display()))?;
            fs::remove_dir_all(&previous).with_context(|| {
                format!("removing previous knowledge Index {}", previous.display())
            })?;
        } else {
            fs::rename(&temporary, &path)
                .with_context(|| format!("publishing knowledge Index {}", path.display()))?;
        }
        write_json(&knowledge_metadata_path()?, &metadata)
    })();
    if result.is_err() {
        drop(fs::remove_dir_all(&temporary));
    }
    result
}

fn register_knowledge_analyzer(index: &Index) {
    index.tokenizers().register(
        KNOWLEDGE_ANALYZER_VERSION,
        TextAnalyzer::builder(SimpleTokenizer::default())
            .filter(LowerCaser)
            .build(),
    );
}

fn knowledge_fields(schema: &Schema) -> Result<KnowledgeFields> {
    Ok(KnowledgeFields {
        text: schema
            .get_field("text")
            .context("reading knowledge text field")?,
        statement_text: schema
            .get_field("statement_text")
            .context("reading knowledge statement_text field")?,
        statement_kind: schema
            .get_field("statement_kind")
            .context("reading knowledge statement_kind field")?,
        reference: schema
            .get_field("reference")
            .context("reading knowledge reference field")?,
        statement_id: schema
            .get_field("statement_id")
            .context("reading knowledge statement_id field")?,
        source_locator: schema
            .get_field("source_locator")
            .context("reading knowledge source_locator field")?,
        remote_published_at: schema
            .get_field("remote_published_at")
            .context("reading knowledge remote_published_at field")?,
        capture_published_at: schema
            .get_field("capture_published_at")
            .context("reading knowledge capture_published_at field")?,
    })
}

fn knowledge_result_for(
    document: &TantivyDocument,
    fields: &KnowledgeFields,
) -> Result<KnowledgeResult> {
    let remote_published_at =
        stored_search_string(document, fields.remote_published_at, "remote_published_at")?;
    Ok(KnowledgeResult {
        text: stored_search_string(document, fields.statement_text, "statement_text")?,
        kind: stored_search_string(document, fields.statement_kind, "statement_kind")?,
        reference: serde_json::from_str(&stored_search_string(
            document,
            fields.reference,
            "reference",
        )?)
        .context("parsing knowledge Card Reference")?,
        statement_id: stored_search_string(document, fields.statement_id, "statement_id")?,
        source: KnowledgeSourceSummary {
            locator: stored_search_string(document, fields.source_locator, "source_locator")?,
        },
        remote_published_at: (!remote_published_at.is_empty()).then_some(remote_published_at),
        capture_published_at: document
            .get_first(fields.capture_published_at)
            .and_then(|value| TantivyValue::as_u64(&value))
            .context("reading stored knowledge capture_published_at")?,
    })
}

fn read_knowledge_metadata() -> Result<KnowledgeIndexMetadata> {
    read_json(&knowledge_metadata_path()?).context("reading knowledge Index metadata")
}

#[allow(dead_code)]
fn record_knowledge_degradation(error: &anyhow::Error) {
    let result = (|| -> Result<()> {
        let directory = repository_dir()?;
        fs::create_dir_all(&directory).context("creating knowledge Index status directory")?;
        write_json(
            &knowledge_status_path()?,
            &AgentError {
                error: StructuredError {
                    code: "index_degraded".to_string(),
                    message: format!("{error:#}"),
                    capability: None,
                },
            },
        )
    })();
    drop(result);
}

fn clear_knowledge_degradation() -> Result<()> {
    let path = knowledge_status_path()?;
    if path
        .try_exists()
        .context("checking knowledge Index status")?
    {
        fs::remove_file(&path)
            .with_context(|| format!("clearing knowledge Index status {}", path.display()))?;
    }
    Ok(())
}

fn read_knowledge_degradation() -> Result<Option<StructuredError>> {
    let path = knowledge_status_path()?;
    if !path
        .try_exists()
        .context("checking knowledge Index status")?
    {
        return Ok(None);
    }
    let status: AgentError = read_json(&path)?;
    Ok(Some(status.error))
}

fn search_index_path() -> Result<PathBuf> {
    Ok(repository_dir()?.join("index"))
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

fn write_search_index(manifests: &[Manifest]) -> Result<()> {
    let path = search_index_path()?;
    let temporary = path.with_file_name(format!(".index-{}", unique_id()));
    fs::create_dir_all(&temporary)
        .with_context(|| format!("creating Search Index {}", temporary.display()))?;
    let mut schema_builder = Schema::builder();
    let capture_id = schema_builder.add_text_field("capture_id", STRING | STORED);
    let source = schema_builder.add_text_field("source", TEXT | STORED);
    let source_sha256 = schema_builder.add_text_field("source_sha256", STRING | STORED);
    let published_at = schema_builder.add_text_field("published_at", STRING | STORED);
    let artifact_id = schema_builder.add_text_field("artifact_id", STRING | STORED);
    let artifact_sha256 = schema_builder.add_text_field("artifact_sha256", STRING | STORED);
    let artifact_locator = schema_builder.add_text_field("artifact_locator", TEXT | STORED);
    let text = schema_builder.add_text_field("text", TEXT);
    let index = Index::create_in_dir(&temporary, schema_builder.build())
        .map_err(|error| anyhow::anyhow!(error))
        .context("creating Search Index")?;
    let mut writer = index
        .writer(50_000_000)
        .map_err(|error| anyhow::anyhow!(error))
        .context("creating Search writer")?;
    for manifest in manifests {
        let capture_dir = captures_dir()?.join(&manifest.capture_id);
        for artifact in search_artifacts_for(manifest)? {
            let locator = serde_json::to_string(&artifact.locator)
                .context("serializing Search artifact Locator")?;
            let text_content = search_text_for(&capture_dir, &artifact)?;
            writer
                .add_document(doc!(
                    capture_id => manifest.capture_id.clone(),
                    source => manifest.source.locator.clone(),
                    source_sha256 => manifest.source.sha256.clone(),
                    published_at => manifest.published_at.to_string(),
                    artifact_id => artifact.artifact_id.clone(),
                    artifact_sha256 => artifact.sha256.clone(),
                    artifact_locator => locator,
                    text => text_content,
                ))
                .map_err(|error| anyhow::anyhow!(error))
                .context("indexing Capture artifact")?;
        }
    }
    writer
        .commit()
        .map_err(|error| anyhow::anyhow!(error))
        .context("publishing Search Index")?;
    if path
        .try_exists()
        .context("checking previous Search Index")?
    {
        fs::remove_dir_all(&path)
            .with_context(|| format!("removing Search Index {}", path.display()))?;
    }
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
                    capability: None,
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

fn search_artifacts_for(manifest: &Manifest) -> Result<Vec<ArtifactMetadata>> {
    let mut artifacts = vec![artifact_from_proof(&manifest.proof)];
    artifacts.extend(manifest.artifacts.iter().map(artifact_from_proof));
    artifacts.extend(manifest.extractions.iter().map(artifact_from_extraction));
    artifacts.extend(derive::search_artifacts(&manifest.capture_id)?);
    Ok(artifacts
        .into_iter()
        .filter(|artifact| is_text_mime(&artifact.mime))
        .collect())
}

fn search_text_for(capture_dir: &Path, artifact: &ArtifactMetadata) -> Result<String> {
    let path = derive::artifact_path(capture_dir, artifact)?;
    let file =
        File::open(&path).with_context(|| format!("opening text artifact {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(u64::try_from(MAX_READ_LENGTH).context("converting Index text limit")?)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading text artifact {}", path.display()))?;
    Ok(String::from_utf8(bytes).unwrap_or_default())
}

fn search_summary_for(document: &TantivyDocument, fields: &SearchFields) -> Result<CaptureSummary> {
    let capture_id = stored_search_string(document, fields.capture_id, "capture_id")?;
    let source = SourceIdentity {
        locator: stored_search_string(document, fields.source, "source")?,
        sha256: stored_search_string(document, fields.source_sha256, "source_sha256")?,
    };
    let published_at = stored_search_string(document, fields.published_at, "published_at")?
        .parse()
        .context("parsing Search publication date")?;
    let locator = serde_json::from_str(&stored_search_string(
        document,
        fields.artifact_locator,
        "artifact_locator",
    )?)
    .context("parsing Search artifact Locator")?;
    Ok(CaptureSummary {
        capture_id: capture_id.clone(),
        source,
        published_at,
        reference: Reference {
            capture_id,
            artifact_id: stored_search_string(document, fields.artifact_id, "artifact_id")?,
            sha256: stored_search_string(document, fields.artifact_sha256, "artifact_sha256")?,
            locator,
        },
    })
}

fn stored_search_string(document: &TantivyDocument, field: Field, name: &str) -> Result<String> {
    document
        .get_first(field)
        .and_then(|value| TantivyValue::as_str(&value))
        .map(str::to_string)
        .with_context(|| format!("reading stored Search field `{name}`"))
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
        order: proof.order,
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
        order: None,
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
    let id = unique_id();
    write_json(&cursor_path(&id)?, cursor)?;
    Ok(format!("v1-{id}"))
}

fn decode_cursor(cursor: &str) -> Result<Cursor> {
    let id = cursor
        .strip_prefix("v1-")
        .context("invalid pagination cursor")?;
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        bail!("invalid pagination cursor");
    }
    read_json(&cursor_path(id)?).context("invalid pagination cursor")
}

fn encode_search_cursor(cursor: &SearchCursor) -> Result<String> {
    let id = unique_id();
    write_json(&cursor_path(&id)?, cursor)?;
    Ok(format!("v2-{id}"))
}

fn decode_search_cursor(cursor: &str) -> Result<SearchCursor> {
    let id = cursor
        .strip_prefix("v2-")
        .context("invalid pagination cursor")?;
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        bail!("invalid pagination cursor");
    }
    read_json(&cursor_path(id)?).context("invalid pagination cursor")
}

fn encode_knowledge_cursor(cursor: &KnowledgeCursor) -> Result<String> {
    let id = unique_id();
    write_json(&cursor_path(&id)?, cursor)?;
    Ok(format!("v3-{id}"))
}

fn decode_knowledge_cursor(cursor: &str) -> Result<KnowledgeCursor> {
    let id = cursor
        .strip_prefix("v3-")
        .context("invalid pagination cursor")?;
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        bail!("invalid pagination cursor");
    }
    read_json(&cursor_path(id)?).context("invalid pagination cursor")
}

fn cursor_path(id: &str) -> Result<PathBuf> {
    let directory = repository_dir()?.join("cursors");
    fs::create_dir_all(&directory)
        .with_context(|| format!("creating cursor directory {}", directory.display()))?;
    Ok(directory.join(format!("{id}.json")))
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

fn reconcile_interrupted(job_id: &str) -> Result<Job> {
    let lock = lock_job(job_id)?;
    let mut job = read_job(job_id)?;
    let has_no_live_worker = job
        .worker_pid
        .is_none_or(|pid| !Path::new("/proc").join(pid.to_string()).exists());
    if matches!(job.state.as_str(), "queued" | "running") && has_no_live_worker {
        let published_derive = derive::reconcile_published(&job)?;
        job.state = if published_derive.is_some() {
            "succeeded".to_string()
        } else {
            "interrupted".to_string()
        };
        job.updated_at = now_secs();
        job.worker_pid = None;
        job.derive_id = published_derive;
        write_job(&job)?;
        append_job_event(&job.id, &job.state)?;
    }
    unlock_job(&lock)?;
    Ok(job)
}

fn policy_for(value: &str) -> Result<Policy> {
    if value != POLICY_NAME && value != "safe-web@1" {
        let document: PolicyDocument =
            serde_json::from_str(value).context("parsing Policy JSON")?;
        return normalize_policy(document.id, document.version, document.snapshot);
    }
    let limits = Limits {
        depth_limit: 2,
        source_limit: 50,
        download_byte_limit: 2 * 1024 * 1024 * 1024,
        disk_byte_limit: 10 * 1024 * 1024 * 1024,
        duration_limit_secs: 30 * 60,
        concurrency_limit: 2,
    };
    let mut allowed_providers = vec![
        "ffmpeg".to_string(),
        "ffprobe".to_string(),
        "whisper-cli".to_string(),
        "pdftotext".to_string(),
        "pdfinfo".to_string(),
        "tesseract".to_string(),
    ];
    let allowed_recipes = if value == POLICY_NAME {
        allowed_providers.push("scriptor-local-derive".to_string());
        vec![
            RecipeKind::KnowledgeCard,
            RecipeKind::StructuredSummary,
            RecipeKind::ProvenClaims,
            RecipeKind::Checklist,
            RecipeKind::MarkdownNote,
            RecipeKind::SourcedAnswer,
        ]
    } else {
        allowed_providers.extend([
            "page-renderer".to_string(),
            "lightpanda".to_string(),
            "instagram-provider".to_string(),
            "linkedin-provider".to_string(),
            "scriptor-binary-acquirer".to_string(),
            "yt-dlp".to_string(),
        ]);
        Vec::new()
    };
    let snapshot = PolicySnapshot {
        duplicate_mode: "reuse".to_string(),
        limits,
        allows_remote_calls: value == "safe-web@1",
        allowed_providers,
        allowed_recipes,
    };
    normalize_policy(
        if value == POLICY_NAME {
            "safe-local".to_string()
        } else {
            "safe-web".to_string()
        },
        1,
        snapshot,
    )
}

fn normalize_policy(id: String, version: u8, snapshot: PolicySnapshot) -> Result<Policy> {
    const LOCAL_PROVIDERS: &[&str] = &[
        "ffmpeg",
        "ffprobe",
        "whisper-cli",
        "pdftotext",
        "pdfinfo",
        "tesseract",
        "scriptor-local-derive",
    ];
    const WEB_PROVIDERS: &[&str] = &[
        "ffmpeg",
        "ffprobe",
        "whisper-cli",
        "pdftotext",
        "pdfinfo",
        "tesseract",
        "page-renderer",
        "lightpanda",
        "instagram-provider",
        "linkedin-provider",
        "scriptor-binary-acquirer",
        "yt-dlp",
    ];
    let valid_identity = matches!((id.as_str(), version), ("safe-local" | "safe-web", 1));
    if !valid_identity {
        bail!("invalid Policy identity {id}@{version}");
    }
    if !matches!(
        snapshot.duplicate_mode.as_str(),
        "reuse" | "create" | "fail"
    ) {
        bail!("invalid Policy duplicate_mode");
    }
    let limits = &snapshot.limits;
    if limits.depth_limit == 0
        || limits.source_limit == 0
        || limits.download_byte_limit == 0
        || limits.disk_byte_limit == 0
        || limits.duration_limit_secs == 0
        || limits.concurrency_limit == 0
    {
        bail!("invalid Policy limits");
    }
    let (allows_remote_calls, allowed_providers) = if id == "safe-local" {
        (false, LOCAL_PROVIDERS)
    } else {
        (true, WEB_PROVIDERS)
    };
    if snapshot.allows_remote_calls != allows_remote_calls
        || snapshot
            .allowed_providers
            .iter()
            .any(|provider| !allowed_providers.contains(&provider.as_str()))
        || (id == "safe-web" && !snapshot.allowed_recipes.is_empty())
        || snapshot.allowed_recipes.iter().any(|recipe| {
            !matches!(
                recipe,
                RecipeKind::KnowledgeCard
                    | RecipeKind::StructuredSummary
                    | RecipeKind::ProvenClaims
                    | RecipeKind::Checklist
                    | RecipeKind::MarkdownNote
                    | RecipeKind::SourcedAnswer
            )
        })
    {
        bail!("inconsistent Policy allowlists");
    }
    let canonical_snapshot =
        serde_json::to_value(&snapshot).context("normalizing Policy snapshot")?;
    let snapshot_bytes =
        serde_json::to_vec(&canonical_snapshot).context("serializing Policy snapshot")?;
    Ok(Policy {
        id,
        version,
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
            details: None,
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

fn next_capture_version(source_hash: &str) -> Result<u64> {
    list_manifests()?
        .into_iter()
        .filter(|manifest| manifest.source.sha256 == source_hash)
        .map(|manifest| manifest.capture_version)
        .max()
        .map_or(Ok(1), |version| {
            version
                .checked_add(1)
                .context("incrementing Capture version")
        })
}

fn lock_capture_key(source_hash: &str) -> Result<File> {
    let directory = repository_dir()?.join("capture-keys");
    fs::create_dir_all(&directory)
        .with_context(|| format!("creating Capture key directory {}", directory.display()))?;
    let path = directory.join(source_hash);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening Capture key lock {}", path.display()))?;
    file.lock_exclusive()
        .with_context(|| format!("locking Capture key {}", path.display()))?;
    Ok(file)
}

fn lock_capture_ledger(capture_id: &str) -> Result<File> {
    let directory = repository_dir()?.join("capture-ledgers");
    fs::create_dir_all(&directory)
        .with_context(|| format!("creating Capture ledger directory {}", directory.display()))?;
    let path = directory.join(capture_id);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening Capture ledger lock {}", path.display()))?;
    file.lock_exclusive()
        .with_context(|| format!("locking Capture ledger {}", path.display()))?;
    Ok(file)
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

fn sha256_file_limited(path: &Path, budget: &ResourceBudget<'_>) -> Result<String> {
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

fn copy_file_limited(source: &Path, destination: &Path, budget: &ResourceBudget<'_>) -> Result<()> {
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

#[cfg(test)]
mod continuation_tests {
    use super::{
        QueuedDiscovery, binary_extension, is_html_discovery, sort_resolution_queue, unique_id,
    };
    use std::fs;

    #[test]
    fn rejects_a_declared_mime_that_conflicts_with_magic_bytes() {
        let path = std::env::temp_dir().join(format!("scriptor-mime-lie-{}", unique_id()));
        assert!(fs::write(&path, b"%PDF-1.4").is_ok());
        assert_eq!(binary_extension(&path, "image/png").ok(), Some(None));
        assert_eq!(
            binary_extension(&path, "application/pdf").ok(),
            Some(Some("pdf"))
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_media_declared_only_by_content_type() {
        let path = std::env::temp_dir().join(format!("scriptor-media-lie-{}", unique_id()));
        assert!(fs::write(&path, b"not a media container").is_ok());
        assert_eq!(binary_extension(&path, "video/mp4").ok(), Some(None));
        assert_eq!(binary_extension(&path, "audio/mpeg").ok(), Some(None));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_binary_content_declared_as_html() {
        let path = std::env::temp_dir().join(format!("scriptor-html-lie-{}", unique_id()));
        assert!(fs::write(&path, b"%PDF-1.4").is_ok());
        assert_eq!(is_html_discovery(&path, "text/html").ok(), Some(false));
        assert!(fs::write(&path, b"<!doctype html><title>page</title>").is_ok());
        assert_eq!(is_html_discovery(&path, "text/html").ok(), Some(true));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn validates_the_full_mp3_frame_sync() {
        let path = std::env::temp_dir().join(format!("scriptor-mp3-magic-{}", unique_id()));
        assert!(fs::write(&path, [0xe0, 0, 0]).is_ok());
        assert_eq!(binary_extension(&path, "audio/mpeg").ok(), Some(None));
        assert!(fs::write(&path, [0xff, 0xfb, 0x90, 0x64]).is_ok());
        assert_eq!(
            binary_extension(&path, "audio/mpeg").ok(),
            Some(Some("mp3"))
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn orders_nested_discoveries_breadth_first() {
        let mut queue = vec![
            QueuedDiscovery {
                discovery_id: "deep".to_string(),
                source: "https://example.test/deep".to_string(),
                depth: 2,
                order: 0,
                order_path: vec![0, 0],
                ancestors: Vec::new(),
                kind: None,
            },
            QueuedDiscovery {
                discovery_id: "second".to_string(),
                source: "https://example.test/second".to_string(),
                depth: 1,
                order: 1,
                order_path: vec![1],
                ancestors: Vec::new(),
                kind: None,
            },
            QueuedDiscovery {
                discovery_id: "first".to_string(),
                source: "https://example.test/first".to_string(),
                depth: 1,
                order: 0,
                order_path: vec![0],
                ancestors: Vec::new(),
                kind: None,
            },
        ];
        sort_resolution_queue(&mut queue);
        assert_eq!(
            queue
                .iter()
                .map(|item| item.discovery_id.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second", "deep"]
        );
    }

    #[test]
    fn source_budget_leaves_every_discovery_after_the_fiftieth_unattempted() {
        let discoveries = (0..51).collect::<Vec<_>>();
        let attempted = discoveries.iter().take(50).count();
        let skipped = discoveries.iter().skip(50).count();
        assert_eq!(attempted, 50);
        assert_eq!(skipped, 1);
    }
}
