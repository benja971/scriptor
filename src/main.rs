//! Orchestration de `scriptor` : parse les arguments, charge la config,
//! détecte la Source, vérifie les binaires requis, calcule le chemin de
//! Sortie, puis relance ce même binaire en mode Worker détaché (cf.
//! `worker.rs`) avant de rendre la main.

mod audio;
mod binary;
mod cli;
mod config;
mod download;
mod notify;
mod output;
mod transcribe;
mod unique_id;
mod worker;

use std::fs::{self, File};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

use anyhow::{Context, Result};
use clap::Parser;

use cli::{Args, Source, detect_source};
use config::Config;

fn main() -> ExitCode {
    let args = Args::parse();

    let result = if args.worker {
        args.into_worker_params()
            .and_then(|params| worker::run(&params))
    } else {
        launch(&args)
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err:#}");
            ExitCode::FAILURE
        }
    }
}

/// Étapes réalisées par le process CLI initial, avant tout détachement :
/// charge la config, détecte la Source, vérifie les binaires requis,
/// calcule le chemin de Sortie (pour une Source locale), puis relance ce
/// même binaire en mode Worker détaché.
fn launch(args: &Args) -> Result<()> {
    let config = Config::load()?;
    let source = detect_source(&args.source);

    ensure_required_binaries_present(&source)?;

    let output = match &source {
        Source::Local(path) => Some(
            output::output_path_for_local(std::path::Path::new(path))
                .context("échec de la résolution du chemin de Sortie")?,
        ),
        Source::Remote(_) => None,
    };

    let log_path = create_log_file_path().context("échec de la création du fichier de log")?;
    let threads = u32::try_from(config.threads)
        .context("valeur de `threads` invalide dans la configuration")?;

    spawn_worker(
        args,
        &config,
        output.as_deref(),
        &log_path,
        &config.model_path(),
        threads,
    )
}

/// Vérifie la présence des binaires requis par la Source donnée : `ffmpeg`
/// et `whisper-cli` toujours, `yt-dlp` seulement pour une Source distante.
/// Échoue immédiatement (avant tout détachement) si l'un d'eux est absent.
fn ensure_required_binaries_present(source: &Source) -> Result<()> {
    binary::ensure_present("ffmpeg")?;
    binary::ensure_present("whisper-cli")?;
    if matches!(source, Source::Remote(_)) {
        binary::ensure_present("yt-dlp").context("nécessaire pour une Source distante")?;
    }
    Ok(())
}

/// Relance ce même binaire (`std::env::current_exe`) en mode Worker
/// détaché : stdin `/dev/null`, stdout/stderr redirigés vers `log_path`,
/// nouveau groupe de processus (`process_group(0)`), sans attendre sa fin.
fn spawn_worker(
    args: &Args,
    config: &Config,
    output: Option<&std::path::Path>,
    log_path: &std::path::Path,
    model_path: &std::path::Path,
    threads: u32,
) -> Result<()> {
    let exe = std::env::current_exe()
        .context("impossible de déterminer le chemin de l'exécutable courant")?;

    let stdout_file = File::create(log_path)
        .with_context(|| format!("création du fichier de log {}", log_path.display()))?;
    let stderr_file = stdout_file
        .try_clone()
        .context("duplication du descripteur du fichier de log")?;

    let mut command = Command::new(&exe);
    command
        .arg(&args.source)
        .arg("--worker")
        .arg("--log")
        .arg(log_path)
        .arg("--model-path")
        .arg(model_path)
        .arg("--language")
        .arg(&config.language)
        .arg("--threads")
        .arg(threads.to_string());

    if let Some(output) = output {
        command.arg("--output").arg(output);
    } else {
        command.arg("--output-dir").arg(&config.output_dir);
    }

    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .process_group(0);

    command
        .spawn()
        .context("impossible de lancer le Worker détaché")?;

    println!("Worker lancé, log : {}", log_path.display());

    Ok(())
}

/// Chemin du fichier de log du Worker :
/// `dirs::cache_dir()/scriptor/logs/<id-unique>.log`. Le répertoire parent
/// est créé si besoin.
fn create_log_file_path() -> Result<PathBuf> {
    let cache_dir =
        dirs::cache_dir().context("impossible de déterminer le répertoire de cache utilisateur")?;
    let logs_dir = cache_dir.join("scriptor").join("logs");
    fs::create_dir_all(&logs_dir)
        .with_context(|| format!("création du répertoire de logs {}", logs_dir.display()))?;

    let filename = format!("{}.log", unique_id::unique_id());
    Ok(logs_dir.join(filename))
}
