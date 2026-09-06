//! Configuration de `scriptor` : chargement depuis `~/.config/scriptor/config.toml`,
//! valeurs par défaut si le fichier ou un champ est absent, création automatique du
//! fichier au premier chargement.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Configuration de `scriptor`, chargée depuis `config.toml` avec application de
/// défauts pour tout champ absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output_dir: default_output_dir(),
            model: default_model(),
            models_dir: default_models_dir(),
            language: default_language(),
            threads: default_threads(),
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
        .join("Transcriptions")
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

// Les lints anti-panic (`unwrap_used`, `expect_used`, `panic`) sont désactivés ici :
// ils protègent le code de production, pas les assertions de test qui doivent
// justement échouer bruyamment.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::path::PathBuf;

    use super::{
        Config, default_language, default_model, default_models_dir, default_output_dir,
        default_threads,
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
