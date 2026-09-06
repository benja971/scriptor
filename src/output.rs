//! Calcul du chemin de la Sortie : à côté du fichier pour une Source locale,
//! dans `output_dir` nommé d'après le titre pour une Source distante, avec
//! gestion de collision (suffixe `-N`, jamais d'écrasement).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Chemin de Sortie pour une Source locale : même basename que `source_path`,
/// extension `.txt`, à côté du fichier, avec gestion de collision.
///
/// # Errors
///
/// Retourne une erreur si `source_path` n'a pas de nom de fichier exploitable.
pub fn output_path_for_local(source_path: &Path) -> Result<PathBuf> {
    let candidate = source_path.with_extension("txt");
    resolve_collision(&candidate)
}

/// Chemin de Sortie pour une Source distante : titre slugifié, extension
/// `.txt`, dans `output_dir`, avec gestion de collision.
///
/// # Errors
///
/// Retourne une erreur si plus aucun suffixe de collision n'est disponible.
pub fn output_path_for_remote(output_dir: &Path, title: &str) -> Result<PathBuf> {
    let candidate = output_dir.join(format!("{}.txt", slugify(title)));
    resolve_collision(&candidate)
}

/// Basename à transmettre à `whisper-cli` (`-of`) pour que le `.txt` qu'il
/// produit corresponde exactement à `output_path` (whisper-cli ajoutant
/// lui-même l'extension `.txt`).
#[must_use]
pub fn basename_for_transcription(output_path: &Path) -> PathBuf {
    output_path.with_extension("")
}

/// Convertit un titre libre en slug de nom de fichier : minuscules,
/// caractères non alphanumériques remplacés par `-`, tirets consécutifs
/// fusionnés, tirets de bord retirés. Retombe sur `sans-titre` si le
/// résultat est vide (titre entièrement composé de séparateurs).
fn slugify(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut previous_was_dash = false;
    for ch in title.chars() {
        if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                slug.push(lower);
            }
            previous_was_dash = false;
        } else if !previous_was_dash {
            slug.push('-');
            previous_was_dash = true;
        }
    }
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "sans-titre".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Retourne `candidate` si aucun fichier n'existe déjà à ce chemin, sinon le
/// premier chemin `<stem>-N.<ext>` libre à partir de `N = 1`.
fn resolve_collision(candidate: &Path) -> Result<PathBuf> {
    if !candidate.exists() {
        return Ok(candidate.to_path_buf());
    }

    let parent = candidate.parent().unwrap_or_else(|| Path::new("."));
    let stem = candidate
        .file_stem()
        .and_then(OsStr::to_str)
        .with_context(|| {
            format!(
                "impossible de déterminer le nom de fichier de {}",
                candidate.display()
            )
        })?;
    let extension = candidate.extension().and_then(OsStr::to_str);

    (1..=u32::MAX)
        .map(|n| {
            let filename = extension.map_or_else(
                || format!("{stem}-{n}"),
                |extension| format!("{stem}-{n}.{extension}"),
            );
            parent.join(filename)
        })
        .find(|path| !path.exists())
        .context("plus aucun suffixe de collision disponible pour la Sortie")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::fs;

    use assert_fs::TempDir;

    use super::{basename_for_transcription, output_path_for_local, output_path_for_remote};

    #[test]
    fn local_output_uses_same_basename_with_txt_extension() {
        let temp = TempDir::new().expect("répertoire temporaire");
        let source = temp.path().join("interview.mp4");
        fs::write(&source, b"fake video").expect("écriture du faux fichier source");

        let output = output_path_for_local(&source).expect("résolution de la Sortie");

        assert_eq!(output, temp.path().join("interview.txt"));
    }

    #[test]
    fn local_output_adds_numeric_suffix_on_collision() {
        let temp = TempDir::new().expect("répertoire temporaire");
        let source = temp.path().join("interview.mp4");
        fs::write(&source, b"fake video").expect("écriture du faux fichier source");
        fs::write(temp.path().join("interview.txt"), b"already there")
            .expect("écriture d'une Sortie déjà existante");

        let output = output_path_for_local(&source).expect("résolution de la Sortie");

        assert_eq!(output, temp.path().join("interview-1.txt"));
    }

    #[test]
    fn local_output_skips_taken_suffixes() {
        let temp = TempDir::new().expect("répertoire temporaire");
        let source = temp.path().join("interview.mp4");
        fs::write(&source, b"fake video").expect("écriture du faux fichier source");
        fs::write(temp.path().join("interview.txt"), b"1").expect("écriture Sortie -0");
        fs::write(temp.path().join("interview-1.txt"), b"2").expect("écriture Sortie -1");
        fs::write(temp.path().join("interview-2.txt"), b"3").expect("écriture Sortie -2");

        let output = output_path_for_local(&source).expect("résolution de la Sortie");

        assert_eq!(output, temp.path().join("interview-3.txt"));
    }

    #[test]
    fn remote_output_slugifies_title() {
        let temp = TempDir::new().expect("répertoire temporaire");

        let output = output_path_for_remote(temp.path(), "Amazing Video! (2026)")
            .expect("résolution de la Sortie");

        assert_eq!(output, temp.path().join("amazing-video-2026.txt"));
    }

    #[test]
    fn remote_output_falls_back_when_title_has_no_alphanumeric() {
        let temp = TempDir::new().expect("répertoire temporaire");

        let output = output_path_for_remote(temp.path(), "***").expect("résolution de la Sortie");

        assert_eq!(output, temp.path().join("sans-titre.txt"));
    }

    #[test]
    fn remote_output_adds_numeric_suffix_on_collision() {
        let temp = TempDir::new().expect("répertoire temporaire");
        fs::write(temp.path().join("my-video.txt"), b"already there")
            .expect("écriture d'une Sortie déjà existante");

        let output =
            output_path_for_remote(temp.path(), "My Video").expect("résolution de la Sortie");

        assert_eq!(output, temp.path().join("my-video-1.txt"));
    }

    #[test]
    fn basename_for_transcription_strips_txt_extension() {
        let path = std::path::Path::new("/tmp/out/interview-1.txt");

        assert_eq!(
            basename_for_transcription(path),
            std::path::PathBuf::from("/tmp/out/interview-1")
        );
    }
}
