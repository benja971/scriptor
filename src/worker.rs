//! Logique du Worker détaché : exécute le Pipeline complet (téléchargement
//! si Source distante → extraction audio → transcription → écriture de la
//! Sortie → nettoyage des temporaires), logue via `tracing`, puis notifie le
//! succès ou l'échec via `notify.rs`.
//!
//! Le process CLI initial relance lui-même le binaire avec `--worker` et
//! stdout/stderr déjà redirigés vers le fichier de log par le `Command` de
//! lancement (cf. `main.rs`) : `tracing_subscriber` écrit donc dans ce
//! fichier simplement en écrivant sur stdout, sans avoir besoin de rouvrir
//! le fichier lui-même.

use std::fs;
use std::io;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};

use crate::cli::{ResolvedSource, WorkerParams};
use crate::unique_id::unique_id;
use crate::{audio, download, notify, output, transcribe};

/// Point d'entrée du mode Worker : initialise le logging fichier, exécute le
/// Pipeline, puis notifie le résultat sur le bureau.
///
/// # Errors
///
/// Retourne une erreur si le logging ne peut pas être initialisé, ou si le
/// Pipeline échoue. La notification (succès ou échec) est toujours best-effort :
/// un échec de notification est logué mais n'écrase jamais le résultat du
/// Pipeline lui-même (la Sortie a déjà été produite avec succès, ou l'erreur
/// d'origine du Pipeline doit rester celle remontée).
pub fn run(params: &WorkerParams) -> Result<()> {
    init_logging()?;

    tracing::info!(source = params.source.display(), "démarrage du Worker");

    match run_pipeline(params) {
        Ok(output_path) => {
            tracing::info!(output = %output_path.display(), "Pipeline terminé avec succès");
            if let Err(notify_err) = notify::notify_success(&output_path) {
                tracing::error!(error = %notify_err, "échec de la notification de succès");
            }
            Ok(())
        }
        Err(err) => {
            tracing::error!(error = format!("{err:#}"), "échec du Pipeline");
            if let Err(notify_err) = notify::notify_failure(params.source.display(), &params.log) {
                tracing::error!(error = %notify_err, "échec de la notification d'échec");
            }
            Err(err)
        }
    }
}

/// Initialise `tracing_subscriber` pour écrire sur stdout (déjà redirigé
/// vers le fichier de log du Worker par le process initial).
fn init_logging() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(io::stdout)
        .with_ansi(false)
        .try_init()
        .map_err(|err| anyhow!("initialisation du logging du Worker : {err}"))
}

/// Exécute le Pipeline complet et retourne le chemin de la Sortie produite.
fn run_pipeline(params: &WorkerParams) -> Result<PathBuf> {
    let tmp_dir = create_tmp_dir()?;
    let _cleanup = TmpDirGuard(tmp_dir.clone());

    let (media_path, output_path) = match &params.source {
        ResolvedSource::Local { path, output } => (PathBuf::from(path), output.clone()),
        ResolvedSource::Remote { url, output_dir } => {
            tracing::info!(url, "téléchargement de la Source distante");
            let downloaded = download::download(url, &tmp_dir)
                .context("échec du téléchargement de la Source")?;
            let output_path = output::output_path_for_remote(output_dir, &downloaded.title)
                .context("échec de la résolution du chemin de Sortie")?;
            (downloaded.path, output_path)
        }
    };

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("création du répertoire de sortie {}", parent.display()))?;
    }

    let audio_wav = tmp_dir.join("audio.wav");
    tracing::info!(input = %media_path.display(), "extraction audio");
    audio::extract_audio(&media_path, &audio_wav).context("échec de l'extraction audio")?;

    let basename = output::basename_for_transcription(&output_path);
    tracing::info!(output = %output_path.display(), "transcription");
    transcribe::transcribe(
        &params.model_path,
        &audio_wav,
        &params.language,
        params.threads,
        &basename,
    )
    .context("échec de la transcription")?;

    Ok(output_path)
}

/// Crée et retourne un dossier temporaire dédié à cette exécution du
/// Pipeline : `dirs::cache_dir()/scriptor/tmp/<id-unique>/`.
fn create_tmp_dir() -> Result<PathBuf> {
    let cache_dir =
        dirs::cache_dir().context("impossible de déterminer le répertoire de cache utilisateur")?;
    let tmp_dir = cache_dir.join("scriptor").join("tmp").join(unique_id());
    fs::create_dir_all(&tmp_dir)
        .with_context(|| format!("création du dossier temporaire {}", tmp_dir.display()))?;
    Ok(tmp_dir)
}

/// Supprime le dossier temporaire du Pipeline à la fin de son scope, que le
/// Pipeline ait réussi ou échoué. Un échec de nettoyage est logué mais ne
/// fait pas échouer le Worker (le `Drop` ne doit pas paniquer).
struct TmpDirGuard(PathBuf);

impl Drop for TmpDirGuard {
    fn drop(&mut self) {
        if let Err(err) = fs::remove_dir_all(&self.0) {
            tracing::warn!(
                path = %self.0.display(),
                error = %err,
                "échec du nettoyage du dossier temporaire"
            );
        }
    }
}
