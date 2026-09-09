//! Configuration de `scriptor` : chargement depuis `~/.config/scriptor/config.toml`,
//! valeurs par défaut si le fichier ou un champ est absent, création automatique du
//! fichier au premier chargement.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Configuration de `scriptor`, chargée depuis `config.toml` avec application de
/// défauts pour tout champ absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Dossier de sortie des transcriptions issues d'une Source distante.
    #[serde(default = "default_output_dir")]
    pub output_dir: PathBuf,
    /// Nom du Modèle whisper utilisé pour la transcription (ex. `small`).
    #[serde(default = "default_model")]
    pub model: String,
    /// Dossier dans lequel `scriptor` cherche les fichiers de Modèle.
    #[serde(default = "default_models_dir")]
    pub models_dir: PathBuf,
    /// Langue de transcription (`auto` pour la détection automatique).
    #[serde(default = "default_language")]
    pub language: String,
    /// Nombre de threads utilisés par la transcription.
    #[serde(default = "default_threads")]
    pub threads: usize,
    /// Intervalle (en secondes) entre deux Frames extraites à fréquence fixe.
    #[serde(default = "default_frame_interval_secs")]
    pub frame_interval_secs: u32,
    /// Seuil de détection ffmpeg (`0.0`-`1.0`) au-delà duquel un changement de
    /// scène déclenche l'extraction d'une Frame supplémentaire.
    #[serde(default = "default_frame_scene_threshold")]
    pub frame_scene_threshold: f64,
    /// Fenêtre (en secondes) en-deçà de laquelle une Frame de changement de
    /// scène trop proche d'une Frame à intervalle fixe est ignorée.
    #[serde(default = "default_frame_dedup_window_secs")]
    pub frame_dedup_window_secs: u32,
    /// Conserve, pour une Source distante, la vidéo telle que téléchargée
    /// (avec son) dans le dossier de Sortie.
    #[serde(default = "default_keep_source_video")]
    pub keep_source_video: bool,
    /// Conserve, pour une Source distante, une version de la vidéo sans
    /// piste audio dans le dossier de Sortie.
    #[serde(default = "default_keep_muted_video")]
    pub keep_muted_video: bool,
    /// Conserve, pour une Source distante, l'audio d'origine (qualité
    /// native, pas le WAV dégradé produit pour la transcription) dans le
    /// dossier de Sortie.
    #[serde(default = "default_keep_audio")]
    pub keep_audio: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output_dir: default_output_dir(),
            model: default_model(),
            models_dir: default_models_dir(),
            language: default_language(),
            threads: default_threads(),
            frame_interval_secs: default_frame_interval_secs(),
            frame_scene_threshold: default_frame_scene_threshold(),
            frame_dedup_window_secs: default_frame_dedup_window_secs(),
            keep_source_video: default_keep_source_video(),
            keep_muted_video: default_keep_muted_video(),
            keep_audio: default_keep_audio(),
        }
    }
}

impl Config {
    /// Charge la configuration depuis `~/.config/scriptor/config.toml`.
    ///
    /// Si le fichier est absent, il est créé avec les valeurs par défaut et son
    /// chemin est annoncé sur stdout. Si le fichier existe mais qu'un champ est
    /// absent, la valeur par défaut de ce champ est utilisée.
    ///
    /// # Errors
    ///
    /// Retourne une erreur si le répertoire de configuration ne peut pas être
    /// déterminé, si le fichier existant ne peut pas être lu ou parsé, ou si la
    /// création du fichier par défaut échoue.
    pub fn load() -> Result<Self> {
        let path = config_file_path()?;
        Self::load_from_path(&path)
    }

    /// Résout le chemin du fichier de Modèle : `models_dir/ggml-<model>.bin`.
    #[must_use]
    pub fn model_path(&self) -> PathBuf {
        self.models_dir.join(format!("ggml-{}.bin", self.model))
    }

    /// Implémentation testable de [`Config::load`], paramétrée par le chemin du
    /// fichier de configuration (évite de dépendre de `~/.config` réel en test).
    fn load_from_path(path: &Path) -> Result<Self> {
        if !path.exists() {
            let config = Self::default();
            config.write_to(path)?;
            println!("Fichier de configuration créé : {}", path.display());
            return Ok(config);
        }

        let content = fs::read_to_string(path)
            .with_context(|| format!("reading configuration file {}", path.display()))?;
        let config: Self = toml::from_str(&content)
            .with_context(|| format!("parsing configuration file {}", path.display()))?;
        Ok(config)
    }

    /// Écrit cette configuration au format TOML à l'emplacement donné, en créant
    /// les répertoires parents si besoin.
    fn write_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("creating configuration directory {}", parent.display())
            })?;
        }
        let serialized =
            toml::to_string_pretty(self).context("serializing configuration to TOML")?;
        fs::write(path, serialized)
            .with_context(|| format!("writing configuration file {}", path.display()))?;
        Ok(())
    }
}

/// Chemin du fichier `config.toml` : `<dirs::config_dir>/scriptor/config.toml`.
fn config_file_path() -> Result<PathBuf> {
    let config_dir =
        dirs::config_dir().context("could not determine user configuration directory")?;
    Ok(config_dir.join("scriptor").join("config.toml"))
}

fn default_output_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("scriptor")
}

fn default_model() -> String {
    "small".to_string()
}

fn default_models_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("scriptor")
        .join("models")
}

fn default_language() -> String {
    "auto".to_string()
}

fn default_threads() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
}

const fn default_frame_interval_secs() -> u32 {
    10
}

// Validé empiriquement sur une vraie vidéo Instagram (UI/dashboard, transitions
// "avant/après" par simple changement de couleur d'accent sur un layout par
// ailleurs identique) : à 0.4 (et même 0.3/0.2/0.15), la passe de détection de
// scène ne produisait STRICTEMENT AUCUNE Frame sur toute la vidéo (le score de
// différence globale de ffmpeg est trop insensible à un changement de teinte
// sur fond stable). À 0.05, les 5 transitions détectées correspondaient
// exactement aux vraies coupures du script (0.1 n'en capturait que 2 sur 5).
const fn default_frame_scene_threshold() -> f64 {
    0.05
}

const fn default_frame_dedup_window_secs() -> u32 {
    3
}

const fn default_keep_source_video() -> bool {
    false
}

const fn default_keep_muted_video() -> bool {
    false
}

const fn default_keep_audio() -> bool {
    false
}

// Les lints anti-panic (`unwrap_used`, `expect_used`, `panic`) sont désactivés ici :
// ils protègent le code de production, pas les assertions de test qui doivent
// justement échouer bruyamment.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::path::PathBuf;

    use super::{
        Config, default_frame_dedup_window_secs, default_frame_interval_secs,
        default_frame_scene_threshold, default_keep_audio, default_keep_muted_video,
        default_keep_source_video, default_language, default_model, default_models_dir,
        default_output_dir, default_threads,
    };

    #[test]
    fn defaults_applied_when_file_absent() {
        let temp = assert_fs::TempDir::new().expect("temporary directory");
        let path = temp.path().join("scriptor").join("config.toml");

        let config = Config::load_from_path(&path).expect("loading should succeed");

        assert_eq!(config.output_dir, default_output_dir());
        assert_eq!(config.model, default_model());
        assert_eq!(config.models_dir, default_models_dir());
        assert_eq!(config.language, default_language());
        assert_eq!(config.threads, default_threads());
        assert_eq!(config.frame_interval_secs, default_frame_interval_secs());
        assert!(
            (config.frame_scene_threshold - default_frame_scene_threshold()).abs() < f64::EPSILON
        );
        assert_eq!(
            config.frame_dedup_window_secs,
            default_frame_dedup_window_secs()
        );
        assert_eq!(config.keep_source_video, default_keep_source_video());
        assert_eq!(config.keep_muted_video, default_keep_muted_video());
        assert_eq!(config.keep_audio, default_keep_audio());
    }

    #[test]
    fn creates_file_on_first_load() {
        let temp = assert_fs::TempDir::new().expect("temporary directory");
        let path = temp.path().join("scriptor").join("config.toml");
        assert!(
            !path.exists(),
            "file should not exist before the first load"
        );

        let config = Config::load_from_path(&path).expect("loading should succeed");

        assert!(path.exists(), "file should be created on first load");
        let reloaded = Config::load_from_path(&path).expect("reload should succeed");
        assert_eq!(config, reloaded);
    }

    #[test]
    fn defaults_applied_when_fields_missing() {
        let temp = assert_fs::TempDir::new().expect("temporary directory");
        let path = temp.path().join("scriptor").join("config.toml");
        std::fs::create_dir_all(path.parent().expect("path has a parent"))
            .expect("creating parent directory");
        std::fs::write(&path, "model = \"medium\"\nthreads = 4\n")
            .expect("writing partial configuration file");

        let config = Config::load_from_path(&path).expect("loading should succeed");

        assert_eq!(config.model, "medium");
        assert_eq!(config.threads, 4);
        assert_eq!(config.output_dir, default_output_dir());
        assert_eq!(config.models_dir, default_models_dir());
        assert_eq!(config.language, default_language());
    }

    #[test]
    fn model_path_is_models_dir_joined_with_ggml_prefixed_name() {
        let config = Config {
            models_dir: PathBuf::from("/tmp/scriptor-models"),
            model: "small".to_string(),
            ..Config::default()
        };

        assert_eq!(
            config.model_path(),
            PathBuf::from("/tmp/scriptor-models/ggml-small.bin")
        );
    }
}
