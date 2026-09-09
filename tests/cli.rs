//! Tests d'intégration du binaire `scriptor` compilé (seam unique décrit
//! dans `docs/spec/scriptor-v1.md`, section "Testing Decisions") : couvre le
//! happy path Source locale, le happy path Source distante, le binaire
//! manquant (échec immédiat sans détachement), la gestion de collision de
//! Sortie, la notification de succès, la notification d'échec (message et
//! contenu du fichier de log), et la création automatique de `config.toml`
//! au premier lancement.
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

/// Faux `ffmpeg` couvrant les usages du Pipeline : extraction audio et
/// conservation d'artefacts (`-an`/`-vn`/copie, écrit un octet au dernier
/// argument - `wait_for_file` exige une taille non nulle, comme le ferait un
/// vrai `ffmpeg`), extraction de Frames (reconnue à la présence de `-map`,
/// écrit une seule Frame et une ligne `showinfo` par passe, pour les deux
/// passes intervalle/scène), et filtre couleur unie (reconnu à
/// `signalstats`, annonce toujours un large écart de luminance : aucune
/// Frame de ces tests n'est censée être filtrée).
const FAKE_FFMPEG: &str = r#"#!/bin/sh
set -eu
last=""
has_map=0
has_signalstats=0
for arg in "$@"; do
  last="$arg"
  if [ "$arg" = "-map" ]; then
    has_map=1
  fi
  case "$arg" in
    *signalstats*) has_signalstats=1 ;;
  esac
done
if [ "$has_signalstats" = "1" ]; then
  echo "[Parsed_metadata_1 @ 0x0] lavfi.signalstats.YMIN=0" >&2
  echo "[Parsed_metadata_1 @ 0x0] lavfi.signalstats.YMAX=255" >&2
elif [ "$has_map" = "1" ]; then
  dir="${last%/*}"
  printf 'x' > "$dir/frame-000001.jpg"
  echo "[Parsed_showinfo @ 0x0] n:0 pts_time:0.000" >&2
else
  printf 'x' > "$last"
fi
"#;

/// Faux `ffprobe` annonçant la présence d'un flux vidéo (cf. `frames.rs`,
/// `has_video_stream`) : sortie non vide sur stdout.
const FAKE_FFPROBE: &str = "#!/bin/sh\necho 0\n";

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

const FAKE_WHISPER_CLI_FAILURE: &str = r#"#!/bin/sh
echo "boom: fake whisper-cli failure" >&2
exit 1
"#;

/// Faux `yt-dlp` reproduisant exactement les arguments passés par
/// `download.rs` (`--paths`, `--output`, `--print-to-file after_move:filepath
/// <marker>`) : écrit un faux fichier vidéo (non vide - `wait_for_file` exige
/// une taille non nulle, comme le ferait un vrai téléchargement) dont le nom
/// sert de titre, et enregistre son chemin dans le fichier marqueur attendu.
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
video_path="$output_dir/My Remote Video.mp4"
printf 'x' > "$video_path"
printf '%s' "$video_path" > "$marker"
"#;

/// Faux `notify-send` qui capture ses arguments (le message) dans
/// `capture_path`, un argument par ligne, pour vérifier le contenu exact de
/// la notification envoyée.
fn fake_notify_send_capture_script(capture_path: &Path) -> String {
    format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\n",
        capture_path.display()
    )
}

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
        write_executable(&bin_dir, "ffprobe", FAKE_FFPROBE);
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

/// Attend que `path` existe avec un contenu non vide, avec un timeout court :
/// le Worker étant détaché, le process initial rend la main avant que le
/// fichier n'existe.
fn wait_for_file(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(Instant::now);
    let has_content = |p: &Path| p.metadata().is_ok_and(|meta| meta.len() > 0);
    while Instant::now() < deadline {
        if has_content(path) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    has_content(path)
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

    let expected_output = env.work_dir.join("interview").join("transcription.txt");
    assert!(
        wait_for_file(&expected_output, Duration::from_secs(5)),
        "la Sortie {} n'est jamais apparue",
        expected_output.display()
    );
    let content = fs::read_to_string(&expected_output).expect("lecture de la Sortie produite");
    assert_eq!(
        content,
        format!("Source : {}\n\nfaux contenu transcrit\n", media.display())
    );
}

#[test]
fn frames_directory_is_produced_inside_output_dir() {
    let env = TestEnv::new("frames");
    env.write_config(&env.work_dir.join("out"));
    let media = env.write_media_file("interview.mp4");

    env.command()
        .arg(media.to_str().expect("chemin utf-8"))
        .assert()
        .success();

    let expected_output = env.work_dir.join("interview").join("transcription.txt");
    assert!(
        wait_for_file(&expected_output, Duration::from_secs(5)),
        "la Sortie {} n'est jamais apparue",
        expected_output.display()
    );

    let expected_frame = env
        .work_dir
        .join("interview")
        .join("frames")
        .join("frame-0001-0.000s.jpg");
    assert!(
        wait_for_file(&expected_frame, Duration::from_secs(5)),
        "la Frame {} n'est jamais apparue",
        expected_frame.display()
    );
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
        !env.work_dir.join("interview").exists(),
        "aucun dossier de Sortie ne doit être produit si un binaire requis est absent"
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
    fs::create_dir(env.work_dir.join("interview"))
        .expect("écriture d'un dossier de Sortie déjà existant");

    env.command()
        .arg(media.to_str().expect("chemin utf-8"))
        .assert()
        .success();

    let expected_output = env.work_dir.join("interview-1").join("transcription.txt");
    assert!(
        wait_for_file(&expected_output, Duration::from_secs(5)),
        "la Sortie {} n'est jamais apparue",
        expected_output.display()
    );
    assert!(
        !env.work_dir
            .join("interview")
            .join("transcription.txt")
            .exists(),
        "le dossier de Sortie déjà existant ne doit jamais recevoir de nouvelle transcription"
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

#[test]
fn remote_source_happy_path_produces_output_named_from_title() {
    let env = TestEnv::new("remote-happy");
    let output_dir = env.work_dir.join("out");
    env.write_config(&output_dir);
    write_executable(&env.bin_dir, "yt-dlp", FAKE_YT_DLP_SUCCESS);

    env.command()
        .arg("https://example.com/video")
        .assert()
        .success()
        .stdout(predicate::str::contains("Worker lancé"));

    let expected_output = output_dir.join("my-remote-video").join("transcription.txt");
    assert!(
        wait_for_file(&expected_output, Duration::from_secs(5)),
        "la Sortie {} n'est jamais apparue",
        expected_output.display()
    );
    let content = fs::read_to_string(&expected_output).expect("lecture de la Sortie produite");
    assert_eq!(
        content,
        "Source : https://example.com/video\n\nfaux contenu transcrit\n"
    );

    let output_video_dir = output_dir.join("my-remote-video");
    assert!(
        !output_video_dir.join("source.mp4").exists(),
        "aucun artefact conservé par défaut (source.mp4)"
    );
    assert!(
        !output_video_dir.join("video-muted.mp4").exists(),
        "aucun artefact conservé par défaut (video-muted.mp4)"
    );
    assert!(
        !output_video_dir.join("audio.mka").exists(),
        "aucun artefact conservé par défaut (audio.mka)"
    );
}

#[test]
fn keep_flags_produce_extra_artifacts_for_remote_source() {
    let env = TestEnv::new("remote-keep");
    let output_dir = env.work_dir.join("out");
    env.write_config(&output_dir);
    write_executable(&env.bin_dir, "yt-dlp", FAKE_YT_DLP_SUCCESS);

    env.command()
        .arg("https://example.com/video")
        .arg("--keep-source-video")
        .arg("--keep-muted-video")
        .arg("--keep-audio")
        .assert()
        .success()
        .stdout(predicate::str::contains("Worker lancé"));

    let output_video_dir = output_dir.join("my-remote-video");
    let expected_output = output_video_dir.join("transcription.txt");
    assert!(
        wait_for_file(&expected_output, Duration::from_secs(5)),
        "la Sortie {} n'est jamais apparue",
        expected_output.display()
    );

    let source_video = output_video_dir.join("source.mp4");
    assert!(
        wait_for_file(&source_video, Duration::from_secs(5)),
        "la vidéo source {} n'est jamais apparue",
        source_video.display()
    );
    let muted_video = output_video_dir.join("video-muted.mp4");
    assert!(
        wait_for_file(&muted_video, Duration::from_secs(5)),
        "la vidéo muette {} n'est jamais apparue",
        muted_video.display()
    );
    let audio = output_video_dir.join("audio.mka");
    assert!(
        wait_for_file(&audio, Duration::from_secs(5)),
        "l'audio {} n'est jamais apparu",
        audio.display()
    );
}

#[test]
fn success_notification_reports_output_path() {
    let env = TestEnv::new("notify-success");
    env.write_config(&env.work_dir.join("out"));
    let media = env.write_media_file("interview.mp4");
    let capture_path = env.bin_dir.join("notify-capture.txt");
    write_executable(
        &env.bin_dir,
        "notify-send",
        &fake_notify_send_capture_script(&capture_path),
    );

    env.command()
        .arg(media.to_str().expect("chemin utf-8"))
        .assert()
        .success();

    let expected_output = env.work_dir.join("interview").join("transcription.txt");
    assert!(
        wait_for_file(&expected_output, Duration::from_secs(5)),
        "la Sortie {} n'est jamais apparue",
        expected_output.display()
    );
    assert!(
        wait_for_file(&capture_path, Duration::from_secs(5)),
        "notify-send n'a jamais été appelé"
    );
    let captured = fs::read_to_string(&capture_path).expect("lecture des arguments capturés");
    let expected_output_dir = env.work_dir.join("interview");
    assert_eq!(
        captured.trim_end_matches('\n'),
        format!("Transcription terminée : {}", expected_output_dir.display())
    );
}

#[test]
fn failure_notification_reports_source_and_log_which_contains_the_error() {
    let env = TestEnv::new("notify-failure");
    env.write_config(&env.work_dir.join("out"));
    let media = env.write_media_file("interview.mp4");
    write_executable(&env.bin_dir, "whisper-cli", FAKE_WHISPER_CLI_FAILURE);
    let capture_path = env.bin_dir.join("notify-capture.txt");
    write_executable(
        &env.bin_dir,
        "notify-send",
        &fake_notify_send_capture_script(&capture_path),
    );

    let assert = env
        .command()
        .arg(media.to_str().expect("chemin utf-8"))
        .assert()
        .success()
        .stdout(predicate::str::contains("Worker lancé"));

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let log_path = stdout
        .lines()
        .find_map(|line| line.strip_prefix("Worker lancé, log : "))
        .map(PathBuf::from)
        .expect("le message affiché doit contenir le chemin du log");

    assert!(
        wait_for_file(&capture_path, Duration::from_secs(5)),
        "notify-send n'a jamais été appelé"
    );
    let captured = fs::read_to_string(&capture_path).expect("lecture des arguments capturés");
    let expected_message = format!(
        "Échec transcription {} : voir {}",
        media.display(),
        log_path.display()
    );
    assert_eq!(captured.trim_end_matches('\n'), expected_message);

    assert!(
        wait_for_file(&log_path, Duration::from_secs(5)),
        "le fichier de log {} n'est jamais apparu",
        log_path.display()
    );
    let log_content = fs::read_to_string(&log_path).expect("lecture du fichier de log");
    assert!(
        log_content.contains("boom: fake whisper-cli failure"),
        "le fichier de log doit contenir le détail de l'échec, contenu : {log_content}"
    );

    assert!(
        !env.work_dir.join("interview").exists(),
        "aucun dossier de Sortie orphelin ne doit rester si le Pipeline échoue"
    );
}

/// Test end-to-end avec les **vrais** binaires (`ffmpeg`, `whisper-cli`,
/// aucun mock) et un **vrai** modèle whisper installé à l'emplacement par
/// défaut (`~/.local/share/scriptor/models/ggml-small.bin`, cf. spec section
/// "Further Notes"). Génère lui-même son fichier audio de test (silence de
/// 2 secondes via le vrai `ffmpeg`) plutôt que de dépendre d'un fichier
/// externe.
///
/// Volontairement ignoré par défaut : la Testing Decision de la spec
/// (`docs/spec/scriptor-v1.md`) est explicite - "Tests avec vrais binaires :
/// marqués `#[ignore]`, lancés manuellement uniquement" (la CI n'a ni le
/// modèle whisper ni forcément les binaires réels installés). À lancer
/// manuellement, dans le devShell Nix, via `cargo test -- --ignored`.
#[test]
#[ignore = "vrais binaires + vrai modèle whisper requis ; lancer via `cargo test -- --ignored`"]
fn real_binaries_local_source_happy_path() {
    let xdg_config = unique_temp_dir("real-xdg-config");
    let xdg_cache = unique_temp_dir("real-xdg-cache");
    let work_dir = unique_temp_dir("real-work");

    // Génère 2 secondes de silence avec le vrai `ffmpeg` (attendu sur le
    // PATH du devShell Nix), plutôt que de dépendre d'un fichier audio/vidéo
    // externe versionné dans le dépôt.
    let media = work_dir.join("silence.wav");
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=16000:cl=mono",
            "-t",
            "2",
            "-y",
        ])
        .arg(&media)
        .status()
        .expect("lancement du vrai ffmpeg pour générer le fichier audio de test");
    assert!(
        status.success(),
        "la génération du fichier audio de test (vrai ffmpeg) a échoué"
    );

    // `models_dir` volontairement absent de la config de test : le défaut
    // (`~/.local/share/scriptor/models`) est l'emplacement réel où le vrai
    // modèle `ggml-small.bin` est installé sur cette machine (cf. spec).
    // `XDG_DATA_HOME` n'est donc pas surchargé ici, contrairement à
    // `XDG_CONFIG_HOME`/`XDG_CACHE_HOME` qui restent isolés dans un
    // répertoire temporaire propre à ce test.
    let config_dir = xdg_config.join("scriptor");
    fs::create_dir_all(&config_dir).expect("création du répertoire de config de test");
    fs::write(
        config_dir.join("config.toml"),
        "output_dir = \"/tmp/scriptor-real-test-unused\"\nmodel = \"small\"\nlanguage = \"en\"\nthreads = 1\n",
    )
    .expect("écriture du config.toml de test");

    Command::cargo_bin("scriptor")
        .expect("binaire scriptor introuvable")
        .env("XDG_CONFIG_HOME", &xdg_config)
        .env("XDG_CACHE_HOME", &xdg_cache)
        .arg(media.to_str().expect("chemin utf-8"))
        .assert()
        .success()
        .stdout(predicate::str::contains("Worker lancé"));

    let expected_output = work_dir.join("silence").join("transcription.txt");
    assert!(
        wait_for_file(&expected_output, Duration::from_mins(1)),
        "la Sortie {} n'est jamais apparue (transcription réelle, peut être lente)",
        expected_output.display()
    );

    let _ = fs::remove_dir_all(&xdg_config);
    let _ = fs::remove_dir_all(&xdg_cache);
    let _ = fs::remove_dir_all(&work_dir);
}
