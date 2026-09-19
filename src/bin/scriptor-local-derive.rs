use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
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

#[derive(Serialize)]
struct KnowledgeCard {
    format_version: u8,
    recipe: &'static str,
    knowledge_core: KnowledgeCore,
}

#[derive(Serialize)]
struct KnowledgeCore {
    statements: Vec<Value>,
    coverage: Vec<Value>,
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
        return write_empty_recipe_output(&cli.output, &request.recipe.kind);
    }
    let mut coverage = Vec::new();
    let mut statements = Vec::new();
    let has_caption = request.inputs.iter().any(is_caption);

    for (index, input) in request.inputs.iter().enumerate() {
        let id = index.checked_add(1).context("numbering local statements")?;
        if is_capture_metadata(input) {
            coverage.push(json!({
                "reference": input.reference,
                "state": "examined",
                "reason": "capture-provenance",
            }));
            continue;
        }
        if input.mime.starts_with("image/") {
            coverage.push(json!({
                "reference": input.reference,
                "state": "unusable",
                "reason": "no-local-visual-analysis",
            }));
            continue;
        }
        if has_caption && is_ocr(input) {
            coverage.push(json!({
                "reference": input.reference,
                "state": "unusable",
                "reason": "ocr-layout-not-reliable-for-extractive-card",
            }));
            continue;
        }
        let text =
            if is_text_mime(&input.mime) {
                Some(fs::read_to_string(&input.path).with_context(|| {
                    format!("reading selected text input {}", input.path.display())
                })?)
            } else {
                None
            };
        if is_caption(input)
            && publish_caption(
                &input.reference,
                text.as_deref(),
                id,
                &mut coverage,
                &mut statements,
            )?
        {
            continue;
        }
        if let Some(text) = text.as_deref().and_then(text_excerpt) {
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
    let output = KnowledgeCard {
        format_version: 1,
        recipe: "knowledge-card",
        knowledge_core: KnowledgeCore {
            statements,
            coverage,
        },
    };
    write_output(&cli.output, &output)
}

fn write_empty_recipe_output(path: &PathBuf, recipe: &str) -> Result<()> {
    write_output(
        path,
        &json!({
            "format_version": 1,
            "recipe": recipe,
            "claims": [],
        }),
    )
}

fn write_output(path: &PathBuf, output: &impl Serialize) -> Result<()> {
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

fn artifact_id(input: &Input) -> Option<&str> {
    input.reference.get("artifact_id").and_then(Value::as_str)
}

fn is_caption(input: &Input) -> bool {
    artifact_id(input).is_some_and(|id| id == "extraction-caption")
}

fn is_ocr(input: &Input) -> bool {
    artifact_id(input).is_some_and(|id| id.ends_with("-ocr"))
}

fn is_capture_metadata(input: &Input) -> bool {
    artifact_id(input).is_some_and(|id| id.starts_with("proof-instagram-"))
}

fn text_excerpt(text: &str) -> Option<String> {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        None
    } else {
        Some(normalized.chars().take(2_000).collect())
    }
}

fn publish_caption(
    reference: &Value,
    text: Option<&str>,
    id: usize,
    coverage: &mut Vec<Value>,
    statements: &mut Vec<Value>,
) -> Result<bool> {
    let excerpts = text.map(caption_excerpts).unwrap_or_default();
    if excerpts.is_empty() {
        return Ok(false);
    }
    coverage.push(json!({
        "reference": reference,
        "state": "examined",
        "reason": "local-text-extraction",
    }));
    for (part, text) in excerpts.into_iter().enumerate() {
        let part = part
            .checked_add(1)
            .context("numbering caption statements")?;
        statements.push(json!({
            "id": format!("statement-{id}-{part}"),
            "kind": "attributed-declaration",
            "text": format!("La Source indique : {text}"),
            "anchors": [{"reference": reference}],
        }));
    }
    Ok(true)
}

fn caption_excerpts(text: &str) -> Vec<String> {
    let mut excerpts = Vec::new();
    let mut paragraph = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() {
            push_paragraph(&mut excerpts, &mut paragraph);
        } else if let Some(item) = line.strip_prefix(['•', '-', '*']).map(str::trim) {
            push_paragraph(&mut excerpts, &mut paragraph);
            if let Some(item) = text_excerpt(item) {
                excerpts.push(item);
            }
        } else {
            paragraph.push(line);
        }
    }
    push_paragraph(&mut excerpts, &mut paragraph);
    excerpts
}

fn push_paragraph(excerpts: &mut Vec<String>, paragraph: &mut Vec<&str>) {
    if let Some(text) = text_excerpt(&paragraph.join(" ")) {
        excerpts.push(text);
    }
    paragraph.clear();
}

#[cfg(test)]
mod tests {
    use super::{KnowledgeCard, KnowledgeCore, caption_excerpts};
    use anyhow::{Context, Result};
    use serde_json::json;

    #[test]
    fn caption_excerpts_keep_paragraphs_and_bullets_atomic() {
        assert_eq!(
            caption_excerpts(
                "Titre\n\nContexte utile.\n\n• Première tactique\n• Deuxième tactique"
            ),
            [
                "Titre",
                "Contexte utile.",
                "Première tactique",
                "Deuxième tactique"
            ]
        );
    }

    #[test]
    fn knowledge_card_serializes_statements_before_coverage() -> Result<()> {
        let card = KnowledgeCard {
            format_version: 1,
            recipe: "knowledge-card",
            knowledge_core: KnowledgeCore {
                statements: vec![json!({"text":"utile"})],
                coverage: vec![json!({"reason":"détaillé"})],
            },
        };
        let output = serde_json::to_string(&card)?;
        let statements = output.find("statements").context("finding statements")?;
        let coverage = output.find("coverage").context("finding coverage")?;
        if statements < coverage {
            Ok(())
        } else {
            anyhow::bail!("statements must precede coverage")
        }
    }
}
