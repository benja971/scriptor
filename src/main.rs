//! Orchestration de `scriptor` : parse les arguments, charge la config,
//! détecte la Source, vérifie les binaires requis, calcule le dossier de
//! Sortie, puis relance ce même binaire en mode Worker détaché (cf.
//! `worker.rs`) avant de rendre la main.

mod artifacts;
mod audio;
mod binary;
mod cli;
mod config;
mod download;
mod frames;
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
/// calcule le dossier de Sortie (pour une Source locale), puis relance ce
/// même binaire en mode Worker détaché.
fn launch(args: &Args) -> Result<()> {
    let config = Config::load()?;
    let source = detect_source(&args.source);

    ensure_required_binaries_present(&source)?;

    let output = match &source {
        Source::Local(path) => Some(
            output::output_dir_for_local(std::path::Path::new(path))
                .context("failed to resolve output directory")?,
        ),
        Source::Remote(_) => None,
    };

    let log_path = create_log_file_path().context("failed to create log file")?;
    let threads =
        u32::try_from(config.threads).context("invalid `threads` value in configuration")?;

    spawn_worker(
        args,
        &config,
        output.as_deref(),
        &log_path,
        &config.model_path(),
        threads,
    )
}

/// Vérifie la présence des binaires requis par la Source donnée : `ffmpeg`,
/// `ffprobe` et `whisper-cli` toujours, `yt-dlp` seulement pour une Source
/// distante. Échoue immédiatement (avant tout détachement) si l'un d'eux est
/// absent.
fn ensure_required_binaries_present(source: &Source) -> Result<()> {
    binary::ensure_present("ffmpeg")?;
    binary::ensure_present("ffprobe")?;
    binary::ensure_present("whisper-cli")?;
    if matches!(source, Source::Remote(_)) {
        binary::ensure_present("yt-dlp").context("required for a remote Source")?;
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
    let exe = std::env::current_exe().context("could not determine current executable path")?;

    let stdout_file = File::create(log_path)
        .with_context(|| format!("creating log file {}", log_path.display()))?;
    let stderr_file = stdout_file
        .try_clone()
        .context("duplicating log file descriptor")?;

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
        .arg(threads.to_string())
        .arg("--frame-interval-secs")
        .arg(config.frame_interval_secs.to_string())
        .arg("--frame-scene-threshold")
        .arg(config.frame_scene_threshold.to_string())
        .arg("--frame-dedup-window-secs")
        .arg(config.frame_dedup_window_secs.to_string());

    if args.keep_source_video || config.keep_source_video {
        command.arg("--keep-source-video");
    }
    if args.keep_muted_video || config.keep_muted_video {
        command.arg("--keep-muted-video");
    }
    if args.keep_audio || config.keep_audio {
        command.arg("--keep-audio");
    }

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
        .context("could not launch detached Worker")?;

    println!("Worker lancé, log : {}", log_path.display());

    Ok(())
}

/// Chemin du fichier de log du Worker :
/// `dirs::cache_dir()/scriptor/logs/<id-unique>.log`. Le répertoire parent
/// est créé si besoin.
fn create_log_file_path() -> Result<PathBuf> {
    let cache_dir = dirs::cache_dir().context("could not determine user cache directory")?;
    let logs_dir = cache_dir.join("scriptor").join("logs");
    fs::create_dir_all(&logs_dir)
        .with_context(|| format!("creating logs directory {}", logs_dir.display()))?;

    let filename = format!("{}.log", unique_id::unique_id());
    Ok(logs_dir.join(filename))
}
