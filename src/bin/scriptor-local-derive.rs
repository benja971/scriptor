use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Parser)]
struct Cli {
    #[arg(long)]
    request: PathBuf,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Deserialize)]
struct Request {
    context: RequestContext,
    recipe: Recipe,
    inputs: Vec<Input>,
}

#[derive(Deserialize)]
struct RequestContext {
    path: PathBuf,
}

#[derive(Deserialize)]
struct Recipe {
    kind: String,
}

#[derive(Deserialize)]
struct Input {
    reference: Value,
    path: PathBuf,
    mime: String,
}

#[derive(Deserialize)]
struct InferenceContext {
    excluded_candidates: Vec<ExcludedCandidate>,
}

#[derive(Deserialize)]
struct ExcludedCandidate {
    reference: Value,
    reason: String,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let request: Request = read_json(&cli.request).context("reading Derive request")?;
    let context: InferenceContext =
        read_json(&request.context.path).context("reading bounded inference context")?;
    if request.recipe.kind != "knowledge-card" {
        return write_output(
            &cli.output,
            &json!({
                "format_version": 1,
                "recipe": request.recipe.kind,
                "claims": [],
            }),
        );
    }
    let mut coverage = Vec::new();
    let mut statements = Vec::new();

    for (index, input) in request.inputs.iter().enumerate() {
        let id = index.checked_add(1).context("numbering local statements")?;
        let text =
            if is_text_mime(&input.mime) {
                Some(fs::read_to_string(&input.path).with_context(|| {
                    format!("reading selected text input {}", input.path.display())
                })?)
            } else {
                None
            };
        if let Some(text) = text.as_deref().and_then(first_nonempty_line) {
            coverage.push(json!({
                "reference": input.reference,
                "state": "examined",
                "reason": "local-text-extraction",
            }));
            statements.push(json!({
                "id": format!("statement-{id}"),
                "kind": "attributed-declaration",
                "text": format!("La Source indique : {text}"),
                "anchors": [{"reference": input.reference}],
            }));
            continue;
        }
        coverage.push(json!({
            "reference": input.reference,
            "state": "unusable",
            "reason": "no-local-text-extraction",
        }));
        statements.push(json!({
            "id": format!("uncertainty-{id}"),
            "kind": "uncertainty",
            "text": "L'artefact sélectionné n'a pas de texte exploitable par le Provider local.",
            "anchors": [{"reference": input.reference}],
            "limitation": "Le Provider local ne réalise ni analyse visuelle ni inférence de contenu binaire.",
        }));
    }
    coverage.extend(context.excluded_candidates.into_iter().map(|candidate| {
        json!({
            "reference": candidate.reference,
            "state": "excluded",
            "reason": candidate.reason,
        })
    }));
    let output = json!({
        "format_version": 1,
        "recipe": "knowledge-card",
        "knowledge_core": {
            "coverage": coverage,
            "statements": statements,
        },
    });
    write_output(&cli.output, &output)
}

fn write_output(path: &PathBuf, output: &Value) -> Result<()> {
    fs::write(
        path,
        serde_json::to_vec(&output).context("serializing knowledge-card output")?,
    )
    .with_context(|| format!("writing knowledge-card output {}", path.display()))?;
    println!(
        "{{\"mime\":\"application/json\",\"effective_parameters\":{{\"style\":\"extractive-local\"}}}}"
    );
    Ok(())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &PathBuf) -> Result<T> {
    serde_json::from_slice(&fs::read(path).with_context(|| format!("reading {}", path.display()))?)
        .with_context(|| format!("parsing JSON {}", path.display()))
}

fn is_text_mime(mime: &str) -> bool {
    mime.starts_with("text/") || matches!(mime, "application/json" | "application/xml")
}

fn first_nonempty_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(500).collect())
}
