//! Calcul du dossier de Sortie : à côté du fichier pour une Source locale,
//! dans `output_dir` nommé d'après le titre pour une Source distante, avec
//! gestion de collision (suffixe `-N`, jamais d'écrasement). Le dossier
//! réservé contient `transcription.txt` et, si la Source a produit des
//! Frames, un sous-dossier `frames/`.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Dossier de Sortie pour une Source locale : même basename que `source_path`
/// (extension retirée), à côté du fichier, avec gestion de collision.
///
/// # Errors
///
/// Retourne une erreur si `source_path` n'a pas de nom de fichier exploitable,
/// ou si plus aucun suffixe de collision n'est disponible.
pub fn output_dir_for_local(source_path: &Path) -> Result<PathBuf> {
    let candidate = source_path.with_extension("");
    resolve_dir_collision(&candidate)
}

/// Dossier de Sortie pour une Source distante : titre slugifié, dans
/// `output_dir`, avec gestion de collision.
///
/// # Errors
///
/// Retourne une erreur si plus aucun suffixe de collision n'est disponible.
pub fn output_dir_for_remote(output_dir: &Path, title: &str) -> Result<PathBuf> {
    let candidate = output_dir.join(slugify(title));
    resolve_dir_collision(&candidate)
}

/// Basename à transmettre à `whisper-cli` (`-of`) pour que le `.txt` qu'il
/// produit atterrisse dans le dossier de Sortie sous le nom
/// `transcription.txt` (whisper-cli ajoutant lui-même l'extension `.txt`).
#[must_use]
pub fn transcription_basename(output_dir: &Path) -> PathBuf {
    output_dir.join("transcription")
}

/// Chemin du sous-dossier de Frames à l'intérieur du dossier de Sortie déjà
/// réservé : `frames/`. Pas de gestion de collision séparée nécessaire ici,
/// le dossier de Sortie étant déjà unique. N'est créé sur disque que si des
/// Frames sont effectivement écrites (cf. `frames::extract_frames`).
#[must_use]
pub fn frames_dir_for(output_dir: &Path) -> PathBuf {
    output_dir.join("frames")
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

/// Réserve `candidate` comme dossier de Sortie si aucun n'existe déjà à ce
/// chemin, sinon le premier `<candidate>-N` libre à partir de `N = 1`.
///
/// La réservation est atomique (création exclusive du dossier) plutôt qu'un
/// `exists()` suivi d'une création séparée : deux lancements concurrents sur
/// la même Source ne peuvent jamais calculer le même dossier de Sortie,
/// contrairement à un simple test d'existence qui laisserait une fenêtre
/// entre la vérification et l'écriture réelle par le Pipeline.
fn resolve_dir_collision(candidate: &Path) -> Result<PathBuf> {
    if try_reserve_dir(candidate)? {
        return Ok(candidate.to_path_buf());
    }

    let parent = candidate.parent().unwrap_or_else(|| Path::new("."));
    let name = candidate
        .file_name()
        .and_then(OsStr::to_str)
        .with_context(|| {
            format!(
                "could not determine directory name of {}",
                candidate.display()
            )
        })?;

    for n in 1..=u32::MAX {
        let path = parent.join(format!("{name}-{n}"));
        if try_reserve_dir(&path)? {
            return Ok(path);
        }
    }

    Err(anyhow::anyhow!(
        "ran out of collision suffixes for the output directory"
    ))
}

/// Tente de créer le dossier `path` de façon exclusive (`fs::create_dir`
/// échoue si le chemin existe déjà, que ce soit un fichier ou un dossier).
/// Retourne `true` si la réservation a réussi, `false` en cas de collision.
fn try_reserve_dir(path: &Path) -> Result<bool> {
    match std::fs::create_dir(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(err) => {
            Err(err).with_context(|| format!("reserving output directory {}", path.display()))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::fs;

    use assert_fs::TempDir;

    use super::{
        frames_dir_for, output_dir_for_local, output_dir_for_remote, transcription_basename,
        try_reserve_dir,
    };

    #[test]
    fn local_output_dir_uses_source_basename_without_extension() {
        let temp = TempDir::new().expect("temporary directory");
        let source = temp.path().join("interview.mp4");
        fs::write(&source, b"fake video").expect("writing fake source file");

        let output_dir = output_dir_for_local(&source).expect("resolving output directory");

        assert_eq!(output_dir, temp.path().join("interview"));
        assert!(output_dir.is_dir());
    }

    #[test]
    fn local_output_dir_adds_numeric_suffix_on_collision() {
        let temp = TempDir::new().expect("temporary directory");
        let source = temp.path().join("interview.mp4");
        fs::write(&source, b"fake video").expect("writing fake source file");
        fs::create_dir(temp.path().join("interview"))
            .expect("writing an already existing output directory");

        let output_dir = output_dir_for_local(&source).expect("resolving output directory");

        assert_eq!(output_dir, temp.path().join("interview-1"));
    }

    #[test]
    fn local_output_dir_skips_taken_suffixes() {
        let temp = TempDir::new().expect("temporary directory");
        let source = temp.path().join("interview.mp4");
        fs::write(&source, b"fake video").expect("writing fake source file");
        fs::create_dir(temp.path().join("interview")).expect("writing output dir -0");
        fs::create_dir(temp.path().join("interview-1")).expect("writing output dir -1");
        fs::create_dir(temp.path().join("interview-2")).expect("writing output dir -2");

        let output_dir = output_dir_for_local(&source).expect("resolving output directory");

        assert_eq!(output_dir, temp.path().join("interview-3"));
    }

    #[test]
    fn remote_output_dir_slugifies_title() {
        let temp = TempDir::new().expect("temporary directory");

        let output_dir = output_dir_for_remote(temp.path(), "Amazing Video! (2026)")
            .expect("resolving output directory");

        assert_eq!(output_dir, temp.path().join("amazing-video-2026"));
    }

    #[test]
    fn remote_output_dir_falls_back_when_title_has_no_alphanumeric() {
        let temp = TempDir::new().expect("temporary directory");

        let output_dir =
            output_dir_for_remote(temp.path(), "***").expect("resolving output directory");

        assert_eq!(output_dir, temp.path().join("sans-titre"));
    }

    #[test]
    fn remote_output_dir_adds_numeric_suffix_on_collision() {
        let temp = TempDir::new().expect("temporary directory");
        fs::create_dir(temp.path().join("my-video"))
            .expect("writing an already existing output directory");

        let output_dir =
            output_dir_for_remote(temp.path(), "My Video").expect("resolving output directory");

        assert_eq!(output_dir, temp.path().join("my-video-1"));
    }

    #[test]
    fn transcription_basename_joins_output_dir() {
        let output_dir = std::path::Path::new("/tmp/out/interview-1");

        assert_eq!(
            transcription_basename(output_dir),
            std::path::PathBuf::from("/tmp/out/interview-1/transcription")
        );
    }

    #[test]
    fn frames_dir_for_joins_output_dir_with_frames() {
        let output_dir = std::path::Path::new("/tmp/out/interview-1");

        assert_eq!(
            frames_dir_for(output_dir),
            std::path::PathBuf::from("/tmp/out/interview-1/frames")
        );
    }

    #[test]
    fn local_output_dir_reserves_the_path_atomically() {
        let temp = TempDir::new().expect("temporary directory");
        let source = temp.path().join("interview.mp4");
        fs::write(&source, b"fake video").expect("writing fake source file");

        let output_dir = output_dir_for_local(&source).expect("resolving output directory");

        // La réservation crée le dossier : un lancement concurrent qui
        // tenterait le même chemin échouerait immédiatement, sans jamais
        // pouvoir croire (à tort) que le chemin est encore libre.
        assert!(output_dir.is_dir());
        assert!(!try_reserve_dir(&output_dir).expect("checking reservation"));
    }
}
