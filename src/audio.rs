use std::env;
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::binary::binary_exists_in;

/// Extrait la piste audio de `input` vers `output_wav`, au format PCM16 mono
/// 16 kHz, via `ffmpeg`.
///
/// # Errors
///
/// Retourne une erreur si le binaire `ffmpeg` est absent du `PATH`, si le
/// process ne peut pas être lancé, ou si `ffmpeg` termine avec un code de
/// sortie non nul (le message d'erreur inclut alors stdout/stderr du
/// process).
#[allow(dead_code)]
pub fn extract_audio(input: &Path, output_wav: &Path) -> Result<()> {
    let path_env = env::var_os("PATH").unwrap_or_default();
    extract_audio_with_path(input, output_wav, &path_env)
}

fn extract_audio_with_path(input: &Path, output_wav: &Path, path_env: &OsStr) -> Result<()> {
    if !binary_exists_in("ffmpeg", path_env) {
        bail!("binaire `ffmpeg` introuvable dans le PATH");
    }

    let output = Command::new("ffmpeg")
        .env("PATH", path_env)
        .arg("-y")
        .arg("-i")
        .arg(input)
        .args(["-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le"])
        .arg(output_wav)
        .output()
        .context("échec du lancement de `ffmpeg`")?;

    if !output.status.success() {
        bail!(
            "`ffmpeg` a échoué (code {:?})\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    Ok(())
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

    use super::extract_audio_with_path;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir(label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "scriptor-test-audio-{label}-{}-{n}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("création du répertoire temporaire de test");
        dir
    }

    fn write_fake_ffmpeg(bin_dir: &Path, script: &str) {
        let path = bin_dir.join("ffmpeg");
        fs::write(&path, script).expect("écriture du faux ffmpeg");
        let mut perms = fs::metadata(&path)
            .expect("lecture des métadonnées du faux ffmpeg")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod du faux ffmpeg");
    }

    const FAKE_FFMPEG_SUCCESS: &str = r#"#!/bin/sh
set -eu
last=""
for arg in "$@"; do
  last="$arg"
done
: > "$last"
"#;

    const FAKE_FFMPEG_FAILURE: &str = r#"#!/bin/sh
echo "boom: fake ffmpeg failure" >&2
exit 1
"#;

    #[test]
    fn extract_audio_success_writes_wav() {
        let bin_dir = unique_temp_dir("bin-ok");
        write_fake_ffmpeg(&bin_dir, FAKE_FFMPEG_SUCCESS);
        let work_dir = unique_temp_dir("work-ok");
        let input = work_dir.join("input.mp4");
        fs::write(&input, "fake video bytes").expect("écriture du faux fichier d'entrée");
        let output_wav = work_dir.join("output.wav");

        extract_audio_with_path(&input, &output_wav, bin_dir.as_os_str())
            .expect("l'extraction mockée doit réussir");

        assert!(output_wav.is_file());

        let _ = fs::remove_dir_all(&bin_dir);
        let _ = fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn extract_audio_failure_surfaces_stderr() {
        let bin_dir = unique_temp_dir("bin-fail");
        write_fake_ffmpeg(&bin_dir, FAKE_FFMPEG_FAILURE);
        let work_dir = unique_temp_dir("work-fail");
        let input = work_dir.join("input.mp4");
        fs::write(&input, "fake video bytes").expect("écriture du faux fichier d'entrée");
        let output_wav = work_dir.join("output.wav");

        let err = extract_audio_with_path(&input, &output_wav, bin_dir.as_os_str())
            .expect_err("l'extraction mockée doit échouer");

        assert!(format!("{err:#}").contains("boom: fake ffmpeg failure"));

        let _ = fs::remove_dir_all(&bin_dir);
        let _ = fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn extract_audio_missing_binary_fails_fast() {
        let empty_bin_dir = unique_temp_dir("bin-missing");
        let work_dir = unique_temp_dir("work-missing");
        let input = work_dir.join("input.mp4");
        fs::write(&input, "fake video bytes").expect("écriture du faux fichier d'entrée");
        let output_wav = work_dir.join("output.wav");

        let err = extract_audio_with_path(&input, &output_wav, empty_bin_dir.as_os_str())
            .expect_err("doit échouer si ffmpeg est absent du PATH");

        assert!(format!("{err:#}").contains("ffmpeg"));

        let _ = fs::remove_dir_all(&empty_bin_dir);
        let _ = fs::remove_dir_all(&work_dir);
    }
}
