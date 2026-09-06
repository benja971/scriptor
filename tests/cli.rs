//! Tests d'intégration du binaire `scriptor` compilé (seam unique décrit
//! dans `docs/spec/scriptor-v1.md`, section "Testing Decisions") : couvre le
//! happy path Source locale, le binaire manquant (échec immédiat sans
//! détachement), la gestion de collision de Sortie, et la création
//! automatique de `config.toml` au premier lancement.
//!
//! `yt-dlp`/`ffmpeg`/`whisper-cli`/`notify-send` sont remplacés par de faux
//! scripts shell injectés en tête de `PATH`. Le Worker étant détaché, ces
//! tests ne font pas de `.wait()` sur le process ; ils "pollent" (avec
//! timeout court) l'apparition du fichier de Sortie ou de log.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use assert_cmd::Command;
use predicates::prelude::*;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Répertoire temporaire unique pour un test donné, nettoyé au mieux à la
/// fin (via `Drop` implicite du `TempDir` de la structure `TestEnv`).
fn unique_temp_dir(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "scriptor-test-cli-{label}-{}-{n}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("création du répertoire temporaire de test");
    dir
}

fn write_executable(dir: &Path, name: &str, script: &str) {
    let path = dir.join(name);
    fs::write(&path, script).expect("écriture du faux binaire");
    let mut perms = fs::metadata(&path)
        .expect("lecture des métadonnées du faux binaire")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod du faux binaire");
}

const FAKE_FFMPEG: &str = r#"#!/bin/sh
set -eu
last=""
for arg in "$@"; do
  last="$arg"
done
: > "$last"
"#;

const FAKE_WHISPER_CLI: &str = r#"#!/bin/sh
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
printf 'faux contenu transcrit\n' > "${of}.txt"
"#;

const FAKE_NOTIFY_SEND_SUCCESS: &str = r"#!/bin/sh
exit 0
";

/// Environnement isolé pour un test : dossier de faux binaires, config XDG
/// dédiée, dossier de travail pour la Source et la Sortie.
struct TestEnv {
    bin_dir: PathBuf,
    xdg_config: PathBuf,
    xdg_cache: PathBuf,
    xdg_data: PathBuf,
    work_dir: PathBuf,
}

impl TestEnv {
    fn new(label: &str) -> Self {
        let bin_dir = unique_temp_dir(&format!("{label}-bin"));
        let xdg_config = unique_temp_dir(&format!("{label}-xdg-config"));
        let xdg_cache = unique_temp_dir(&format!("{label}-xdg-cache"));
        let xdg_data = unique_temp_dir(&format!("{label}-xdg-data"));
        let work_dir = unique_temp_dir(&format!("{label}-work"));

        write_executable(&bin_dir, "ffmpeg", FAKE_FFMPEG);
        write_executable(&bin_dir, "whisper-cli", FAKE_WHISPER_CLI);
        write_executable(&bin_dir, "notify-send", FAKE_NOTIFY_SEND_SUCCESS);

        Self {
            bin_dir,
            xdg_config,
            xdg_cache,
            xdg_data,
            work_dir,
        }
    }

    fn write_config(&self, output_dir: &Path) {
        let config_dir = self.xdg_config.join("scriptor");
        fs::create_dir_all(&config_dir).expect("création du répertoire de config de test");
        let config = format!(
            "output_dir = \"{}\"\nmodel = \"small\"\nmodels_dir = \"{}\"\nlanguage = \"en\"\nthreads = 1\n",
            output_dir.display(),
            self.work_dir.join("models").display(),
        );
        fs::write(config_dir.join("config.toml"), config).expect("écriture du config.toml de test");
    }

    fn command(&self) -> Command {
        let mut command = Command::cargo_bin("scriptor").expect("binaire scriptor introuvable");
        command
            .env("PATH", &self.bin_dir)
            .env("XDG_CONFIG_HOME", &self.xdg_config)
            .env("XDG_CACHE_HOME", &self.xdg_cache)
            .env("XDG_DATA_HOME", &self.xdg_data);
        command
    }

    fn write_media_file(&self, name: &str) -> PathBuf {
        let path = self.work_dir.join(name);
        fs::write(&path, b"faux contenu video").expect("écriture du faux fichier média");
        path
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.bin_dir);
        let _ = fs::remove_dir_all(&self.xdg_config);
        let _ = fs::remove_dir_all(&self.xdg_cache);
        let _ = fs::remove_dir_all(&self.xdg_data);
        let _ = fs::remove_dir_all(&self.work_dir);
    }
}

/// Attend l'apparition de `path`, avec un timeout court : le Worker étant
/// détaché, le process initial rend la main avant que le fichier n'existe.
fn wait_for_file(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(Instant::now);
    while Instant::now() < deadline {
        if path.is_file() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    path.is_file()
}

#[test]
fn local_source_happy_path_produces_output_next_to_source() {
    let env = TestEnv::new("happy");
    env.write_config(&env.work_dir.join("out"));
    let media = env.write_media_file("interview.mp4");

    env.command()
        .arg(media.to_str().expect("chemin utf-8"))
        .assert()
        .success()
        .stdout(predicate::str::contains("Worker lancé"));

    let expected_output = env.work_dir.join("interview.txt");
    assert!(
        wait_for_file(&expected_output, Duration::from_secs(5)),
        "la Sortie {} n'est jamais apparue",
        expected_output.display()
    );
    let content = fs::read_to_string(&expected_output).expect("lecture de la Sortie produite");
    assert_eq!(content, "faux contenu transcrit\n");
}

#[test]
fn missing_required_binary_fails_immediately_without_output() {
    let env = TestEnv::new("missing-binary");
    env.write_config(&env.work_dir.join("out"));
    let media = env.write_media_file("interview.mp4");

    // PATH pointe vers un dossier vide : aucun binaire requis n'est présent.
    let empty_bin_dir = unique_temp_dir("missing-binary-empty");

    Command::cargo_bin("scriptor")
        .expect("binaire scriptor introuvable")
        .env("PATH", &empty_bin_dir)
        .env("XDG_CONFIG_HOME", &env.xdg_config)
        .env("XDG_CACHE_HOME", &env.xdg_cache)
        .env("XDG_DATA_HOME", &env.xdg_data)
        .arg(media.to_str().expect("chemin utf-8"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("ffmpeg"));

    assert!(
        !env.work_dir.join("interview.txt").exists(),
        "aucune Sortie ne doit être produite si un binaire requis est absent"
    );
    assert!(
        !env.xdg_cache.join("scriptor").join("logs").exists(),
        "aucun fichier de log ne doit être créé avant la vérification des binaires requis"
    );

    let _ = fs::remove_dir_all(&empty_bin_dir);
}

#[test]
fn output_collision_appends_numeric_suffix() {
    let env = TestEnv::new("collision");
    env.write_config(&env.work_dir.join("out"));
    let media = env.write_media_file("interview.mp4");
    fs::write(env.work_dir.join("interview.txt"), "sortie déjà existante")
        .expect("écriture d'une Sortie déjà existante");

    env.command()
        .arg(media.to_str().expect("chemin utf-8"))
        .assert()
        .success();

    let expected_output = env.work_dir.join("interview-1.txt");
    assert!(
        wait_for_file(&expected_output, Duration::from_secs(5)),
        "la Sortie {} n'est jamais apparue",
        expected_output.display()
    );
    assert_eq!(
        fs::read_to_string(env.work_dir.join("interview.txt")).expect("lecture Sortie existante"),
        "sortie déjà existante",
        "la Sortie déjà existante ne doit jamais être écrasée"
    );
}

#[test]
fn creates_default_config_on_first_launch() {
    let env = TestEnv::new("first-config");
    let media = env.write_media_file("interview.mp4");
    let config_path = env.xdg_config.join("scriptor").join("config.toml");
    assert!(
        !config_path.exists(),
        "le config.toml ne doit pas exister avant le premier lancement"
    );

    env.command()
        .arg(media.to_str().expect("chemin utf-8"))
        .assert()
        .success()
        .stdout(predicate::str::contains("Fichier de configuration créé"));

    assert!(
        config_path.is_file(),
        "le config.toml doit être créé au premier lancement"
    );
}
