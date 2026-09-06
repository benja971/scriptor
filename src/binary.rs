use std::env;
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Indique si un binaire nommé `name` est présent et exécutable quelque part
/// dans le `PATH` du process courant.
///
/// Utilisée par les wrappers subprocess (`download`, `audio`, `transcribe`)
/// avant tout lancement de binaire externe, et par l'orchestration pour la
/// vérification préalable des dépendances requises.
#[must_use]
#[allow(dead_code)]
pub fn binary_exists(name: &str) -> bool {
    let path_env = env::var_os("PATH").unwrap_or_default();
    binary_exists_in(name, &path_env)
}

/// Variante de [`binary_exists`] acceptant un `PATH` explicite plutôt que
/// celui, ambiant, du process courant. Permet aux wrappers subprocess de
/// vérifier et d'exécuter un binaire mocké lors des tests, sans dépendre du
/// `PATH` réel de la machine.
pub fn binary_exists_in(name: &str, path_env: &OsStr) -> bool {
    env::split_paths(path_env).any(|dir| is_executable_file(&dir.join(name)))
}

fn is_executable_file(candidate: &Path) -> bool {
    let Ok(metadata) = candidate.metadata() else {
        return false;
    };
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::panic_in_result_fn
    )]

    use std::ffi::OsStr;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::binary_exists_in;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir(label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "scriptor-test-binary-{label}-{}-{n}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("création du répertoire temporaire de test");
        dir
    }

    fn write_executable(dir: &std::path::Path, name: &str) {
        let path = dir.join(name);
        fs::write(&path, "#!/bin/sh\nexit 0\n").expect("écriture du faux binaire");
        let mut perms = fs::metadata(&path)
            .expect("lecture des métadonnées du faux binaire")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod du faux binaire");
    }

    #[test]
    fn finds_executable_binary_in_path() {
        let dir = unique_temp_dir("found");
        write_executable(&dir, "faketool");

        assert!(binary_exists_in("faketool", OsStr::new(&dir)));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn returns_false_when_binary_absent() {
        let dir = unique_temp_dir("absent");

        assert!(!binary_exists_in("doesnotexist", OsStr::new(&dir)));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn returns_false_for_non_executable_file() {
        let dir = unique_temp_dir("non-exec");
        let path = dir.join("notexec");
        fs::write(&path, "not a script").expect("écriture du fichier non-exécutable");
        let mut perms = fs::metadata(&path)
            .expect("lecture des métadonnées")
            .permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&path, perms).expect("chmod du fichier non-exécutable");

        assert!(!binary_exists_in("notexec", OsStr::new(&dir)));

        let _ = fs::remove_dir_all(&dir);
    }
}
