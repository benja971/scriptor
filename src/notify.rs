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
    send_with(&mut Command::new("notify-send"), message)
}

fn send_with(command: &mut Command, message: &str) -> Result<()> {
    let status = command
        .arg(message)
        .status()
        .context("failed to launch notify-send")?;

    if !status.success() {
        anyhow::bail!("notify-send exited with a failure status: {status}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use anyhow::{Context, Result};
    use assert_fs::TempDir;

    use super::send_with;

    fn install_fake_notify_send(dir: &TempDir) -> Result<(PathBuf, PathBuf)> {
        let script_path = dir.path().join("notify-send");
        let capture_path = dir.path().join("captured-args.txt");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\n",
            capture_path.display()
        );
        fs::write(&script_path, script).context("writing fake notify-send")?;
        Ok((script_path, capture_path))
    }

    fn fake_notify_send(script_path: &Path) -> Command {
        let mut command = Command::new("sh");
        command.arg(script_path);
        command
    }

    #[test]
    #[allow(clippy::panic_in_result_fn)]
    fn notify_success_envoie_le_message_attendu() -> Result<()> {
        let dir = TempDir::new().context("creating temporary directory")?;
        let (script_path, capture_path) = install_fake_notify_send(&dir)?;
        let mut command = fake_notify_send(&script_path);

        send_with(
            &mut command,
            &format!(
                "Transcription terminée : {}",
                Path::new("/home/user/video.txt").display()
            ),
        )?;

        let captured = fs::read_to_string(&capture_path).context("reading captured arguments")?;
        assert_eq!(
            captured.trim_end_matches('\n'),
            "Transcription terminée : /home/user/video.txt"
        );
        Ok(())
    }

    #[test]
    #[allow(clippy::panic_in_result_fn)]
    fn notify_failure_envoie_le_message_attendu() -> Result<()> {
        let dir = TempDir::new().context("creating temporary directory")?;
        let (script_path, capture_path) = install_fake_notify_send(&dir)?;
        let mut command = fake_notify_send(&script_path);

        send_with(
            &mut command,
            &format!(
                "Échec transcription https://example.com/video : voir {}",
                Path::new("/home/user/.cache/scriptor/logs/2026-09-06.log").display()
            ),
        )?;

        let captured = fs::read_to_string(&capture_path).context("reading captured arguments")?;
        assert_eq!(
            captured.trim_end_matches('\n'),
            "Échec transcription https://example.com/video : voir /home/user/.cache/scriptor/logs/2026-09-06.log"
        );
        Ok(())
    }

    #[test]
    #[allow(clippy::panic_in_result_fn)]
    fn notify_success_echoue_si_notify_send_absent() -> Result<()> {
        let dir = TempDir::new().context("creating temporary directory")?;
        let mut command = Command::new(dir.path().join("notify-send-absent"));

        let result = send_with(&mut command, "message");

        assert!(result.is_err());
        Ok(())
    }
}
