use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

/// Notifie sur le bureau qu'une transcription s'est terminée avec succès.
pub fn notify_success(output_path: &Path) -> Result<()> {
    send(&format!(
        "Transcription terminée : {}",
        output_path.display()
    ))
}

/// Notifie sur le bureau qu'une transcription a échoué.
pub fn notify_failure(source: &str, log_path: &Path) -> Result<()> {
    send(&format!(
        "Échec transcription {source} : voir {}",
        log_path.display()
    ))
}

/// Envoie une notification desktop native via `notify-send`.
fn send(message: &str) -> Result<()> {
    let status = Command::new("notify-send")
        .arg(message)
        .status()
        .context("échec du lancement de notify-send")?;

    if !status.success() {
        anyhow::bail!("notify-send a retourné un code d'échec : {status}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use anyhow::{Context, Result};
    use assert_fs::TempDir;

    use super::{notify_failure, notify_success};

    /// Empêche les tests de cette table de modifier `PATH` en parallèle : la
    /// mutation de variables d'environnement de process n'est pas
    /// thread-safe, donc un seul test à la fois peut la faire.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Restaure la valeur d'origine de `PATH` à la fin du test, y compris en
    /// cas d'échec d'assertion.
    struct PathOverrideGuard {
        original: Option<String>,
    }

    impl PathOverrideGuard {
        /// Remplace `PATH` par exactement `dir`, sans repli sur le `PATH`
        /// d'origine : garantit qu'un `notify-send` réel installé ailleurs
        /// sur la machine ne peut pas être trouvé pendant le test.
        fn isolated_to(dir: &Path) -> Self {
            let original = env::var("PATH").ok();
            // SAFETY: protégé par ENV_LOCK (un seul test à la fois modifie
            // PATH pour la durée de ce guard).
            unsafe {
                env::set_var("PATH", dir);
            }
            Self { original }
        }
    }

    impl Drop for PathOverrideGuard {
        fn drop(&mut self) {
            // SAFETY: protégé par ENV_LOCK (un seul test à la fois modifie
            // PATH pour la durée de ce guard).
            unsafe {
                match &self.original {
                    Some(value) => env::set_var("PATH", value),
                    None => env::remove_var("PATH"),
                }
            }
        }
    }

    /// Installe un faux `notify-send` dans un dossier temporaire, qui écrit
    /// les arguments reçus dans `captured-args.txt` (un argument par ligne).
    fn install_fake_notify_send(dir: &TempDir) -> Result<PathBuf> {
        let script_path = dir.path().join("notify-send");
        let capture_path = dir.path().join("captured-args.txt");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\n",
            capture_path.display()
        );
        fs::write(&script_path, script).context("écriture du faux notify-send")?;
        let mut perms = fs::metadata(&script_path)
            .context("lecture des permissions du faux notify-send")?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms)
            .context("passage en exécutable du faux notify-send")?;
        Ok(capture_path)
    }

    #[test]
    #[allow(clippy::panic_in_result_fn)]
    fn notify_success_envoie_le_message_attendu() -> Result<()> {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = TempDir::new().context("création du dossier temporaire")?;
        let capture_path = install_fake_notify_send(&dir)?;
        let _guard = PathOverrideGuard::isolated_to(dir.path());

        notify_success(Path::new("/home/user/video.txt"))?;

        let captured =
            fs::read_to_string(&capture_path).context("lecture des arguments capturés")?;
        assert_eq!(
            captured.trim_end_matches('\n'),
            "Transcription terminée : /home/user/video.txt"
        );
        Ok(())
    }

    #[test]
    #[allow(clippy::panic_in_result_fn)]
    fn notify_failure_envoie_le_message_attendu() -> Result<()> {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = TempDir::new().context("création du dossier temporaire")?;
        let capture_path = install_fake_notify_send(&dir)?;
        let _guard = PathOverrideGuard::isolated_to(dir.path());

        notify_failure(
            "https://example.com/video",
            Path::new("/home/user/.cache/scriptor/logs/2026-09-06.log"),
        )?;

        let captured =
            fs::read_to_string(&capture_path).context("lecture des arguments capturés")?;
        assert_eq!(
            captured.trim_end_matches('\n'),
            "Échec transcription https://example.com/video : voir /home/user/.cache/scriptor/logs/2026-09-06.log"
        );
        Ok(())
    }

    #[test]
    #[allow(clippy::panic_in_result_fn)]
    fn notify_success_echoue_si_notify_send_absent() -> Result<()> {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = TempDir::new().context("création du dossier temporaire")?;
        let _guard = PathOverrideGuard::isolated_to(dir.path());

        let result = notify_success(Path::new("/home/user/video.txt"));

        assert!(result.is_err());
        Ok(())
    }
}
