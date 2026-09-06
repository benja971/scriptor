use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

/// Arguments du CLI `scriptor`.
///
/// Les champs autres que `source` forment le mode interne "Worker" : ils ne
/// sont pas destinés à un usage direct par l'utilisateur (masqués du
/// `--help`), mais utilisés par le process CLI initial pour relancer
/// lui-même le binaire en mode détaché, en lui transmettant tout ce qui a
/// déjà été résolu (Sortie, Modèle, config) avant le détachement.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    /// Chemin de fichier local ou URL (Instagram, `TikTok`, `YouTube`, lien direct...).
    pub source: String,

    /// Indique que ce process est le Worker détaché, relancé par le process
    /// CLI initial. Non destiné à un usage direct.
    #[arg(long, hide = true)]
    pub worker: bool,

    /// Chemin de Sortie déjà résolu (Source locale, gestion de collision
    /// déjà faite par le process initial).
    #[arg(long, hide = true)]
    pub output: Option<PathBuf>,

    /// Dossier de sortie configuré (Source distante : le nom final n'est
    /// connu qu'après téléchargement, donc résolu par le Worker lui-même).
    #[arg(long, hide = true)]
    pub output_dir: Option<PathBuf>,

    /// Chemin du fichier de log du Worker (stdout/stderr déjà redirigés vers
    /// ce fichier par le process initial ; transmis en plus pour que le
    /// Worker puisse le mentionner dans la notification d'échec).
    #[arg(long, hide = true)]
    pub log: Option<PathBuf>,

    /// Chemin du Modèle whisper résolu par le process initial.
    #[arg(long, hide = true)]
    pub model_path: Option<PathBuf>,

    /// Langue de transcription résolue par le process initial.
    #[arg(long, hide = true)]
    pub language: Option<String>,

    /// Nombre de threads résolu par le process initial.
    #[arg(long, hide = true)]
    pub threads: Option<u32>,
}

/// Distinction Source locale / Source distante, déterminée à partir du préfixe
/// `http://`/`https://` de l'argument fourni (cf. user story 18 de la spec).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Local(String),
    Remote(String),
}

/// Détecte le type de Source à partir de la chaîne fournie en argument.
pub fn detect_source(input: &str) -> Source {
    if input.starts_with("http://") || input.starts_with("https://") {
        Source::Remote(input.to_string())
    } else {
        Source::Local(input.to_string())
    }
}

/// Source déjà résolue par le process initial, portant par variante
/// exactement la Destination qui lui correspond : impossible de représenter
/// une Source locale avec un dossier de sortie, ou une Source distante avec
/// un chemin de Sortie déjà figé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedSource {
    Local { path: String, output: PathBuf },
    Remote { url: String, output_dir: PathBuf },
}

impl ResolvedSource {
    /// Représentation textuelle de la Source, pour les messages à l'utilisateur
    /// (notification d'échec notamment).
    pub fn display(&self) -> &str {
        match self {
            Self::Local { path, .. } => path,
            Self::Remote { url, .. } => url,
        }
    }
}

/// Tout ce dont le Worker a besoin pour exécuter le Pipeline, déjà validé :
/// contrairement à `Args`, aucun champ n'est optionnel ici.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerParams {
    pub source: ResolvedSource,
    pub log: PathBuf,
    pub model_path: PathBuf,
    pub language: String,
    pub threads: u32,
}

impl Args {
    /// Convertit les arguments bruts du mode Worker en paramètres validés.
    ///
    /// # Errors
    ///
    /// Retourne une erreur si un champ interne requis par le mode Worker est
    /// absent. Ne devrait jamais arriver via l'usage normal : le process
    /// initial fournit toujours l'ensemble complet (cf. `main.rs::spawn_worker`).
    pub fn into_worker_params(self) -> Result<WorkerParams> {
        let source = match detect_source(&self.source) {
            Source::Local(path) => ResolvedSource::Local {
                path,
                output: self
                    .output
                    .context("the Worker requires --output for a local Source")?,
            },
            Source::Remote(url) => ResolvedSource::Remote {
                url,
                output_dir: self
                    .output_dir
                    .context("the Worker requires --output-dir for a remote Source")?,
            },
        };
        Ok(WorkerParams {
            source,
            log: self.log.context("the Worker requires --log")?,
            model_path: self
                .model_path
                .context("the Worker requires --model-path")?,
            language: self.language.context("the Worker requires --language")?,
            threads: self.threads.context("the Worker requires --threads")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_http_url_as_remote() {
        let source = detect_source("http://example.com/video.mp4");
        assert_eq!(
            source,
            Source::Remote("http://example.com/video.mp4".to_string())
        );
    }

    #[test]
    fn detects_https_url_as_remote() {
        let source = detect_source("https://www.youtube.com/watch?v=abc123");
        assert_eq!(
            source,
            Source::Remote("https://www.youtube.com/watch?v=abc123".to_string())
        );
    }

    #[test]
    fn detects_relative_path_as_local() {
        let source = detect_source("videos/interview.mp4");
        assert_eq!(source, Source::Local("videos/interview.mp4".to_string()));
    }

    #[test]
    fn detects_absolute_path_as_local() {
        let source = detect_source("/home/user/videos/interview.mp4");
        assert_eq!(
            source,
            Source::Local("/home/user/videos/interview.mp4".to_string())
        );
    }

    #[test]
    fn empty_string_is_local() {
        let source = detect_source("");
        assert_eq!(source, Source::Local(String::new()));
    }

    #[test]
    fn schemeless_url_like_path_is_local() {
        let source = detect_source("example.com/video.mp4");
        assert_eq!(source, Source::Local("example.com/video.mp4".to_string()));
    }
}
