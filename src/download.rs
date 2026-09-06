use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::binary::binary_exists_in;

/// Résultat d'un téléchargement réussi : chemin du fichier téléchargé et
/// titre de la vidéo (utilisé plus tard pour nommer la Sortie).
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct DownloadedMedia {
    pub path: PathBuf,
    pub title: String,
}

/// Télécharge `url` avec `yt-dlp` dans `output_dir`, et retourne le chemin du
/// fichier téléchargé ainsi que son titre.
///
/// Le chemin final est récupéré via `--print-to-file after_move:filepath`
/// plutôt que par un parsing fragile de la sortie standard de `yt-dlp`
/// (susceptible de contenir des lignes de progression).
///
/// # Errors
///
/// Retourne une erreur si le binaire `yt-dlp` est absent du `PATH`, si
/// `output_dir` ne peut pas être créé, si le process ne peut pas être
/// lancé, ou si `yt-dlp` termine avec un code de sortie non nul (le message
/// d'erreur inclut alors stdout/stderr du process).
#[allow(dead_code)]
pub fn download(url: &str, output_dir: &Path) -> Result<DownloadedMedia> {
    let path_env = env::var_os("PATH").unwrap_or_default();
    download_with_path(url, output_dir, &path_env)
}

fn download_with_path(url: &str, output_dir: &Path, path_env: &OsStr) -> Result<DownloadedMedia> {
    if !binary_exists_in("yt-dlp", path_env) {
        bail!("binaire `yt-dlp` introuvable dans le PATH");
    }

    fs::create_dir_all(output_dir)
        .with_context(|| format!("impossible de créer le répertoire {}", output_dir.display()))?;

    let filepath_marker = output_dir.join(".yt-dlp-filepath");

    let output = Command::new("yt-dlp")
        .env("PATH", path_env)
        .arg(url)
        .arg("--paths")
        .arg(output_dir)
        .arg("--output")
        .arg("%(title)s.%(ext)s")
        .arg("--print-to-file")
        .arg("after_move:filepath")
        .arg(&filepath_marker)
        .output()
        .context("échec du lancement de `yt-dlp`")?;

    if !output.status.success() {
        bail!(
            "`yt-dlp` a échoué (code {:?})\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    let recorded_path = fs::read_to_string(&filepath_marker).with_context(|| {
        format!(
            "impossible de lire le chemin téléchargé depuis {}",
            filepath_marker.display()
        )
    })?;
    let _ = fs::remove_file(&filepath_marker);

    let path = PathBuf::from(recorded_path.trim());
    let title = path
        .file_stem()
        .and_then(OsStr::to_str)
        .with_context(|| format!("impossible d'extraire le titre depuis {}", path.display()))?
        .to_owned();

    Ok(DownloadedMedia { path, title })
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

    use super::download_with_path;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir(label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "scriptor-test-download-{label}-{}-{n}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("création du répertoire temporaire de test");
        dir
    }

    fn write_fake_yt_dlp(bin_dir: &Path, script: &str) {
        let path = bin_dir.join("yt-dlp");
        fs::write(&path, script).expect("écriture du faux yt-dlp");
        let mut perms = fs::metadata(&path)
            .expect("lecture des métadonnées du faux yt-dlp")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod du faux yt-dlp");
    }

    const FAKE_YT_DLP_SUCCESS: &str = r#"#!/bin/sh
set -eu
output_dir=""
marker=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --paths)
      output_dir="$2"
      shift 2
      ;;
    --print-to-file)
      marker="$3"
      shift 3
      ;;
    *)
      shift
      ;;
  esac
done
video_path="$output_dir/Fake Title.mp4"
: > "$video_path"
printf '%s' "$video_path" > "$marker"
"#;

    const FAKE_YT_DLP_FAILURE: &str = r#"#!/bin/sh
echo "boom: fake yt-dlp failure" >&2
exit 1
"#;

    #[test]
    fn download_success_returns_path_and_title() {
        let bin_dir = unique_temp_dir("bin-ok");
        write_fake_yt_dlp(&bin_dir, FAKE_YT_DLP_SUCCESS);
        let output_dir = unique_temp_dir("out-ok");

        let media = download_with_path(
            "https://example.com/video",
            &output_dir,
            bin_dir.as_os_str(),
        )
        .expect("le téléchargement mocké doit réussir");

        assert_eq!(media.title, "Fake Title");
        assert!(media.path.is_file());

        let _ = fs::remove_dir_all(&bin_dir);
        let _ = fs::remove_dir_all(&output_dir);
    }

    #[test]
    fn download_failure_surfaces_stderr() {
        let bin_dir = unique_temp_dir("bin-fail");
        write_fake_yt_dlp(&bin_dir, FAKE_YT_DLP_FAILURE);
        let output_dir = unique_temp_dir("out-fail");

        let err = download_with_path(
            "https://example.com/video",
            &output_dir,
            bin_dir.as_os_str(),
        )
        .expect_err("le téléchargement mocké doit échouer");

        assert!(format!("{err:#}").contains("boom: fake yt-dlp failure"));

        let _ = fs::remove_dir_all(&bin_dir);
        let _ = fs::remove_dir_all(&output_dir);
    }

    #[test]
    fn download_missing_binary_fails_fast() {
        let empty_bin_dir = unique_temp_dir("bin-missing");
        let output_dir = unique_temp_dir("out-missing");

        let err = download_with_path(
            "https://example.com/video",
            &output_dir,
            empty_bin_dir.as_os_str(),
        )
        .expect_err("doit échouer si yt-dlp est absent du PATH");

        assert!(format!("{err:#}").contains("yt-dlp"));

        let _ = fs::remove_dir_all(&empty_bin_dir);
        let _ = fs::remove_dir_all(&output_dir);
    }
}
