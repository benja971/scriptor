//! Conservation optionnelle d'artefacts intermédiaires du Pipeline pour une
//! Source distante : la vidéo brute téléchargée, une version muette, et/ou
//! l'audio d'origine (qualité native, pas le WAV dégradé produit pour la
//! transcription). Réservé à la Source distante : pour une Source locale, le
//! fichier original existe déjà sur le disque de l'utilisateur, à côté du
//! dossier de Sortie.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::binary::ensure_present_in;

/// Copie `media_path` (vidéo brute téléchargée) vers `dest_dir/source.<ext>`,
/// sans transcodage : c'est un simple duplicata du fichier tel que
/// téléchargé.
///
/// # Errors
///
/// Retourne une erreur si `media_path` n'a pas d'extension exploitable, ou si
/// la copie échoue.
pub fn keep_source_video(media_path: &Path, dest_dir: &Path) -> Result<()> {
    let dest = dest_dir
        .join("source")
        .with_extension(extension_of(media_path)?);
    fs::copy(media_path, &dest)
        .with_context(|| format!("copying source video to {}", dest.display()))?;
    Ok(())
}

/// Écrit dans `dest_dir/video-muted.<ext>` la piste vidéo de `media_path`
/// sans sa piste audio, sans réencodage (`-an -c:v copy`).
///
/// # Errors
///
/// Retourne une erreur si `media_path` n'a pas d'extension exploitable, si
/// `ffmpeg` est absent du `PATH`, si le process ne peut pas être lancé, ou
/// s'il termine avec un code de sortie non nul.
pub fn keep_muted_video(media_path: &Path, dest_dir: &Path) -> Result<()> {
    let path_env = env::var_os("PATH").unwrap_or_default();
    keep_muted_video_with_path(media_path, dest_dir, &path_env)
}

fn keep_muted_video_with_path(media_path: &Path, dest_dir: &Path, path_env: &OsStr) -> Result<()> {
    ensure_present_in("ffmpeg", path_env)?;
    let dest = dest_dir
        .join("video-muted")
        .with_extension(extension_of(media_path)?);

    run_ffmpeg_copy(media_path, &["-an", "-c:v", "copy"], &dest, path_env)
}

/// Écrit dans `dest_dir/audio.mka` l'audio d'origine de `media_path`, sans
/// réencodage (`-vn -c:a copy`). Le conteneur Matroska (`.mka`) est utilisé
/// systématiquement plutôt qu'une extension déduite du codec audio : il
/// accepte n'importe quel codec en copie directe, évitant une table de
/// correspondance codec→extension à maintenir pour chaque plateforme source
/// (Instagram, `TikTok`, `YouTube`... n'utilisent pas toujours le même codec).
///
/// # Errors
///
/// Retourne une erreur si `ffmpeg` est absent du `PATH`, si le process ne
/// peut pas être lancé, ou s'il termine avec un code de sortie non nul.
pub fn keep_original_audio(media_path: &Path, dest_dir: &Path) -> Result<()> {
    let path_env = env::var_os("PATH").unwrap_or_default();
    keep_original_audio_with_path(media_path, dest_dir, &path_env)
}

fn keep_original_audio_with_path(
    media_path: &Path,
    dest_dir: &Path,
    path_env: &OsStr,
) -> Result<()> {
    ensure_present_in("ffmpeg", path_env)?;
    let dest = dest_dir.join("audio.mka");

    run_ffmpeg_copy(media_path, &["-vn", "-c:a", "copy"], &dest, path_env)
}

/// Lance `ffmpeg -y -i <input> <stream_args> <dest>`, sans réencodage
/// (`-c:... copy` dans `stream_args`).
fn run_ffmpeg_copy(
    input: &Path,
    stream_args: &[&str],
    dest: &Path,
    path_env: &OsStr,
) -> Result<()> {
    let output = Command::new("ffmpeg")
        .env("PATH", path_env)
        .arg("-y")
        .arg("-i")
        .arg(input)
        .args(stream_args)
        .arg(dest)
        .output()
        .context("failed to launch `ffmpeg`")?;

    if !output.status.success() {
        bail!(
            "`ffmpeg` failed (exit code {:?})\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    Ok(())
}

/// Extension de fichier de `path` (sans le point), ou une erreur si `path`
/// n'en a pas.
fn extension_of(path: &Path) -> Result<&str> {
    path.extension()
        .and_then(OsStr::to_str)
        .with_context(|| format!("could not determine file extension of {}", path.display()))
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

    use super::{keep_muted_video_with_path, keep_original_audio_with_path, keep_source_video};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir(label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "scriptor-test-artifacts-{label}-{}-{n}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("creating test temporary directory");
        dir
    }

    fn write_fake_ffmpeg(bin_dir: &Path, script: &str) {
        let path = bin_dir.join("ffmpeg");
        fs::write(&path, script).expect("writing fake ffmpeg");
        let mut perms = fs::metadata(&path)
            .expect("reading fake ffmpeg metadata")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod on fake ffmpeg");
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
    fn keep_source_video_copies_file_with_original_extension() {
        let work_dir = unique_temp_dir("copy-ok");
        let media_path = work_dir.join("downloaded.mp4");
        fs::write(&media_path, b"fake video bytes").expect("writing fake media file");
        let dest_dir = work_dir.join("out");
        fs::create_dir_all(&dest_dir).expect("creating dest dir");

        keep_source_video(&media_path, &dest_dir).expect("copy should succeed");

        let dest = dest_dir.join("source.mp4");
        assert!(dest.is_file());
        assert_eq!(fs::read(&dest).expect("reading copy"), b"fake video bytes");

        let _ = fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn keep_source_video_fails_without_extension() {
        let work_dir = unique_temp_dir("copy-noext");
        let media_path = work_dir.join("downloaded");
        fs::write(&media_path, b"fake video bytes").expect("writing fake media file");
        let dest_dir = work_dir.join("out");
        fs::create_dir_all(&dest_dir).expect("creating dest dir");

        let err = keep_source_video(&media_path, &dest_dir)
            .expect_err("should fail without a file extension");

        assert!(format!("{err:#}").contains("extension"));

        let _ = fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn keep_muted_video_writes_file_with_original_extension() {
        let bin_dir = unique_temp_dir("bin-muted-ok");
        write_fake_ffmpeg(&bin_dir, FAKE_FFMPEG_SUCCESS);
        let work_dir = unique_temp_dir("work-muted-ok");
        let media_path = work_dir.join("downloaded.mp4");
        fs::write(&media_path, b"fake video bytes").expect("writing fake media file");
        let dest_dir = work_dir.join("out");
        fs::create_dir_all(&dest_dir).expect("creating dest dir");

        keep_muted_video_with_path(&media_path, &dest_dir, bin_dir.as_os_str())
            .expect("mocked extraction should succeed");

        assert!(dest_dir.join("video-muted.mp4").is_file());

        let _ = fs::remove_dir_all(&bin_dir);
        let _ = fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn keep_muted_video_failure_surfaces_stderr() {
        let bin_dir = unique_temp_dir("bin-muted-fail");
        write_fake_ffmpeg(&bin_dir, FAKE_FFMPEG_FAILURE);
        let work_dir = unique_temp_dir("work-muted-fail");
        let media_path = work_dir.join("downloaded.mp4");
        fs::write(&media_path, b"fake video bytes").expect("writing fake media file");
        let dest_dir = work_dir.join("out");
        fs::create_dir_all(&dest_dir).expect("creating dest dir");

        let err = keep_muted_video_with_path(&media_path, &dest_dir, bin_dir.as_os_str())
            .expect_err("mocked extraction should fail");

        assert!(format!("{err:#}").contains("boom: fake ffmpeg failure"));

        let _ = fs::remove_dir_all(&bin_dir);
        let _ = fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn keep_original_audio_writes_mka_file() {
        let bin_dir = unique_temp_dir("bin-audio-ok");
        write_fake_ffmpeg(&bin_dir, FAKE_FFMPEG_SUCCESS);
        let work_dir = unique_temp_dir("work-audio-ok");
        let media_path = work_dir.join("downloaded.mp4");
        fs::write(&media_path, b"fake video bytes").expect("writing fake media file");
        let dest_dir = work_dir.join("out");
        fs::create_dir_all(&dest_dir).expect("creating dest dir");

        keep_original_audio_with_path(&media_path, &dest_dir, bin_dir.as_os_str())
            .expect("mocked extraction should succeed");

        assert!(dest_dir.join("audio.mka").is_file());

        let _ = fs::remove_dir_all(&bin_dir);
        let _ = fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn keep_original_audio_failure_surfaces_stderr() {
        let bin_dir = unique_temp_dir("bin-audio-fail");
        write_fake_ffmpeg(&bin_dir, FAKE_FFMPEG_FAILURE);
        let work_dir = unique_temp_dir("work-audio-fail");
        let media_path = work_dir.join("downloaded.mp4");
        fs::write(&media_path, b"fake video bytes").expect("writing fake media file");
        let dest_dir = work_dir.join("out");
        fs::create_dir_all(&dest_dir).expect("creating dest dir");

        let err = keep_original_audio_with_path(&media_path, &dest_dir, bin_dir.as_os_str())
            .expect_err("mocked extraction should fail");

        assert!(format!("{err:#}").contains("boom: fake ffmpeg failure"));

        let _ = fs::remove_dir_all(&bin_dir);
        let _ = fs::remove_dir_all(&work_dir);
    }
}
