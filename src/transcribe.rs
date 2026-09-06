use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::binary::ensure_present_in;

/// Transcrit `audio_wav` avec `whisper-cli`, en utilisant le modèle situé à
/// `model_path`, la langue `language` et `threads` threads. Écrit le
/// résultat en `.txt` à côté de `output_basename` et retourne son chemin.
///
/// # Errors
///
/// Retourne une erreur si le binaire `whisper-cli` est absent du `PATH`, si
/// le process ne peut pas être lancé, ou si `whisper-cli` termine avec un
/// code de sortie non nul (le message d'erreur inclut alors stdout/stderr du
/// process).
pub fn transcribe(
    model_path: &Path,
    audio_wav: &Path,
    language: &str,
    threads: u32,
    output_basename: &Path,
) -> Result<PathBuf> {
    let path_env = env::var_os("PATH").unwrap_or_default();
    transcribe_with_path(
        model_path,
        audio_wav,
        language,
        threads,
        output_basename,
        &path_env,
    )
}

fn transcribe_with_path(
    model_path: &Path,
    audio_wav: &Path,
    language: &str,
    threads: u32,
    output_basename: &Path,
    path_env: &OsStr,
) -> Result<PathBuf> {
    ensure_present_in("whisper-cli", path_env)?;

    let output = Command::new("whisper-cli")
        .env("PATH", path_env)
        .arg("-m")
        .arg(model_path)
        .arg("-f")
        .arg(audio_wav)
        .arg("-t")
        .arg(threads.to_string())
        .arg("-l")
        .arg(language)
        .arg("-otxt")
        .arg("-of")
        .arg(output_basename)
        .args(["-np", "-nt"])
        .output()
        .context("failed to launch `whisper-cli`")?;

    if !output.status.success() {
        bail!(
            "`whisper-cli` failed (exit code {:?})\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    Ok(output_basename.with_extension("txt"))
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::panic_in_result_fn
    )]

    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::transcribe_with_path;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir(label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "scriptor-test-transcribe-{label}-{}-{n}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("creating test temporary directory");
        dir
    }

    fn write_fake_whisper_cli(bin_dir: &Path, script: &str) {
        let path = bin_dir.join("whisper-cli");
        fs::write(&path, script).expect("writing fake whisper-cli");
        let mut perms = fs::metadata(&path)
            .expect("reading fake whisper-cli metadata")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod on fake whisper-cli");
    }

    const FAKE_WHISPER_CLI_SUCCESS: &str = r#"#!/bin/sh
set -eu
of=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -of)
      of="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
printf 'fake transcription\n' > "${of}.txt"
"#;

    const FAKE_WHISPER_CLI_FAILURE: &str = r#"#!/bin/sh
echo "boom: fake whisper-cli failure" >&2
exit 1
"#;

    #[test]
    fn transcribe_success_writes_txt() {
        let bin_dir = unique_temp_dir("bin-ok");
        write_fake_whisper_cli(&bin_dir, FAKE_WHISPER_CLI_SUCCESS);
        let work_dir = unique_temp_dir("work-ok");
        let model_path = work_dir.join("ggml-small.bin");
        fs::write(&model_path, "fake model bytes").expect("writing fake model");
        let audio_wav = work_dir.join("audio.wav");
        fs::write(&audio_wav, "fake wav bytes").expect("writing fake wav");
        let output_basename = work_dir.join("output");

        let txt_path = transcribe_with_path(
            &model_path,
            &audio_wav,
            "fr",
            4,
            &output_basename,
            bin_dir.as_os_str(),
        )
        .expect("mocked transcription should succeed");

        assert_eq!(txt_path, output_basename.with_extension("txt"));
        assert!(txt_path.is_file());
        let content = fs::read_to_string(&txt_path).expect("reading produced .txt");
        assert_eq!(content, "fake transcription\n");

        let _ = fs::remove_dir_all(&bin_dir);
        let _ = fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn transcribe_failure_surfaces_stderr() {
        let bin_dir = unique_temp_dir("bin-fail");
        write_fake_whisper_cli(&bin_dir, FAKE_WHISPER_CLI_FAILURE);
        let work_dir = unique_temp_dir("work-fail");
        let model_path = work_dir.join("ggml-small.bin");
        fs::write(&model_path, "fake model bytes").expect("writing fake model");
        let audio_wav = work_dir.join("audio.wav");
        fs::write(&audio_wav, "fake wav bytes").expect("writing fake wav");
        let output_basename = work_dir.join("output");

        let err = transcribe_with_path(
            &model_path,
            &audio_wav,
            "fr",
            4,
            &output_basename,
            bin_dir.as_os_str(),
        )
        .expect_err("mocked transcription should fail");

        assert!(format!("{err:#}").contains("boom: fake whisper-cli failure"));

        let _ = fs::remove_dir_all(&bin_dir);
        let _ = fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn transcribe_missing_binary_fails_fast() {
        let empty_bin_dir = unique_temp_dir("bin-missing");
        let work_dir = unique_temp_dir("work-missing");
        let model_path = work_dir.join("ggml-small.bin");
        fs::write(&model_path, "fake model bytes").expect("writing fake model");
        let audio_wav = work_dir.join("audio.wav");
        fs::write(&audio_wav, "fake wav bytes").expect("writing fake wav");
        let output_basename = work_dir.join("output");

        let err = transcribe_with_path(
            &model_path,
            &audio_wav,
            "fr",
            4,
            &output_basename,
            empty_bin_dir.as_os_str(),
        )
        .expect_err("should fail if whisper-cli is absent from PATH");

        assert!(format!("{err:#}").contains("whisper-cli"));

        let _ = fs::remove_dir_all(&empty_bin_dir);
        let _ = fs::remove_dir_all(&work_dir);
    }
}
