use std::env;
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{Result, bail};

/// Indique si un binaire nommé `name` est présent et exécutable dans le
/// `PATH` explicite fourni. Permet aux wrappers subprocess de vérifier et
/// d'exécuter un binaire mocké lors des tests, sans dépendre du `PATH` réel
/// de la machine.
pub fn binary_exists_in(name: &str, path_env: &OsStr) -> bool {
    env::split_paths(path_env).any(|dir| is_executable_file(&dir.join(name)))
}

/// Vérifie que le binaire `name` est présent dans le `PATH` ambiant du
/// process courant, avec le message d'erreur standard sinon.
///
/// # Errors
///
/// Retourne une erreur si le binaire est absent du `PATH`.
pub fn ensure_present(name: &str) -> Result<()> {
    let path_env = env::var_os("PATH").unwrap_or_default();
    ensure_present_in(name, &path_env)
}

/// Variante de [`ensure_present`] acceptant un `PATH` explicite (cf.
/// [`binary_exists_in`]).
///
/// # Errors
///
/// Retourne une erreur si le binaire est absent de `path_env`.
pub fn ensure_present_in(name: &str, path_env: &OsStr) -> Result<()> {
    if !binary_exists_in(name, path_env) {
        bail!("binary `{name}` not found in PATH");
    }
    Ok(())
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
        fs::create_dir_all(&dir).expect("creating test temporary directory");
        dir
    }

    fn write_executable(dir: &std::path::Path, name: &str) {
        let path = dir.join(name);
        fs::write(&path, "#!/bin/sh\nexit 0\n").expect("writing fake binary");
        let mut perms = fs::metadata(&path)
            .expect("reading fake binary metadata")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod on fake binary");
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
        fs::write(&path, "not a script").expect("writing non-executable file");
        let mut perms = fs::metadata(&path).expect("reading metadata").permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&path, perms).expect("chmod on non-executable file");

        assert!(!binary_exists_in("notexec", OsStr::new(&dir)));

        let _ = fs::remove_dir_all(&dir);
    }
}
