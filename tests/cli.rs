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

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use sha2::{Digest, Sha256};

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

const FAKE_PDFINFO: &str = "#!/bin/sh\necho 'Pages: 2'\n";

const FAKE_PDFTOTEXT: &str = r#"#!/bin/sh
set -eu
printf 'Premiere page\nDeuxieme page\n' > "$3"
"#;

const FAKE_TESSERACT: &str = r"#!/bin/sh
printf 'level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n'
printf '5\t1\t1\t1\t1\t1\t12\t24\t36\t48\t95\tBonjour\n'
";

const FAKE_TESSERACT_FAILURE: &str = "#!/bin/sh\necho 'ocr indisponible' >&2\nexit 1\n";

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
        let models_dir = self.work_dir.join("models");
        fs::create_dir_all(&models_dir).expect("création du répertoire des Modèles de test");
        fs::write(models_dir.join("ggml-small.bin"), b"fake whisper model")
            .expect("écriture du Modèle de test");
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

    fn install_binary(&self, name: &str, script: &str) {
        write_executable(&self.bin_dir, name, script);
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

fn write_capture_worker_job(
    env: &TestEnv,
    job_id: &str,
    source: &Path,
    allowed_providers: &[&str],
    duration_secs: u64,
) -> PathBuf {
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("horloge système après l'époque Unix")
        .as_secs();
    let jobs_dir = env.xdg_data.join("scriptor/v2/jobs");
    fs::create_dir_all(&jobs_dir).expect("création du répertoire de Jobs");
    let job = serde_json::json!({
        "job_id": job_id,
        "state": "queued",
        "source": source,
        "policy": {
            "id": "safe-local",
            "version": 1,
            "sha256": "test-policy",
            "snapshot": {
                "duplicate_mode": "reuse",
                "limits": {
                    "max_depth": 2,
                    "max_sources": 50,
                    "max_download_bytes": 2_147_483_648_u64,
                    "max_disk_bytes": 10_737_418_240_u64,
                    "max_duration_secs": duration_secs,
                    "max_concurrency": 2
                },
                "allows_remote_calls": false,
                "allowed_providers": allowed_providers
            }
        },
        "created_at": created_at,
        "updated_at": created_at,
        "worker_pid": null,
        "capture_id": null,
        "error": null
    });
    let path = jobs_dir.join(format!("{job_id}.json"));
    fs::write(
        &path,
        serde_json::to_vec(&job).expect("sérialisation du Job de test"),
    )
    .expect("écriture du Job de test");
    path
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

fn assert_readable_transcription(env: &TestEnv, capture_id: &str, capture: &Value) {
    let transcription = capture["manifest"]["extractions"]
        .as_array()
        .and_then(|extractions| {
            extractions
                .iter()
                .find(|extraction| extraction["artifact_id"] == "extraction-transcription")
        })
        .expect("Extraction de transcription");
    let reference = serde_json::json!({
        "capture_id": capture_id,
        "artifact_id": transcription["artifact_id"],
        "sha256": transcription["sha256"],
        "locator": transcription["locator"],
    })
    .to_string();
    let read: Value = serde_json::from_slice(
        &env.command()
            .args(["capture", "read", "--reference", &reference])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("lecture JSON valide");
    assert_eq!(read["artifact"]["artifact_id"], "extraction-transcription");
    assert_eq!(read["artifact"]["provider"]["name"], "whisper-cli");
    assert_eq!(read["reference"]["locator"], Value::Null);
    assert_eq!(read["content"]["text"], "faux contenu transcrit\n");
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

#[test]
fn capture_returns_a_persistent_job_then_publishes_an_inspectable_capture() {
    let env = TestEnv::new("capture-contract");
    let source = env.write_media_file("notes.txt");

    let output = env
        .command()
        .args([
            "capture",
            source.to_str().expect("chemin utf-8"),
            "--policy",
            "safe-local@1",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let created: Value = serde_json::from_slice(&output).expect("Job JSON valide");
    let job_id = created["job"]["job_id"]
        .as_str()
        .expect("identifiant de Job")
        .to_string();
    assert_eq!(created["job"]["policy"]["id"], "safe-local");
    assert_eq!(created["job"]["policy"]["version"], 1);
    assert_eq!(
        created["job"]["policy"]["snapshot"]["duplicate_mode"],
        "reuse"
    );
    let policy_snapshot = serde_json::to_vec(&created["job"]["policy"]["snapshot"])
        .expect("snapshot de Policy sérialisable");
    assert_eq!(
        created["job"]["policy"]["sha256"],
        format!("{:x}", Sha256::digest(policy_snapshot))
    );

    let output = env
        .command()
        .args(["job", "wait", &job_id, "--timeout-secs", "5"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let finished: Value = serde_json::from_slice(&output).expect("Job JSON valide");
    assert_eq!(finished["state"], "succeeded");
    let capture_id = finished["capture_id"]
        .as_str()
        .expect("identifiant de Capture");

    let output = env
        .command()
        .args(["capture", "inspect", capture_id])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let capture: Value = serde_json::from_slice(&output).expect("Capture JSON valide");
    assert_eq!(capture["manifest"]["capture_id"], capture_id);
    assert_eq!(
        capture["manifest"]["proof"]["sha256"]
            .as_str()
            .map(str::len),
        Some(64)
    );
    assert_eq!(capture["ledger"][0]["event"], "capture_published");
}

#[test]
fn capture_requires_an_explicit_policy_and_reuses_an_identical_source() {
    let env = TestEnv::new("capture-policy-duplicate");
    let source = env.write_media_file("notes.txt");

    env.command()
        .args(["capture", source.to_str().expect("chemin utf-8")])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"code\":\"policy_required\""))
        .stderr(predicate::str::is_empty());

    let create_job = |env: &TestEnv| -> String {
        let output = env
            .command()
            .args([
                "capture",
                source.to_str().expect("chemin utf-8"),
                "--policy",
                "safe-local@1",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        serde_json::from_slice::<Value>(&output).expect("Job JSON valide")["job"]["job_id"]
            .as_str()
            .expect("identifiant de Job")
            .to_string()
    };

    let first_job = create_job(&env);
    let first: Value = serde_json::from_slice(
        &env.command()
            .args(["job", "wait", &first_job, "--timeout-secs", "5"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job JSON valide");
    let second_job = create_job(&env);
    let second: Value = serde_json::from_slice(
        &env.command()
            .args(["job", "wait", &second_job, "--timeout-secs", "5"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job JSON valide");

    assert_eq!(first["state"], "succeeded");
    assert_eq!(second["state"], "succeeded");
    assert_eq!(first["capture_id"], second["capture_id"]);
}

#[test]
fn capture_budget_failure_is_reported_as_a_structured_job_error() {
    let env = TestEnv::new("capture-budget");
    let source = env.work_dir.join("too-large.bin");
    fs::File::create(&source)
        .expect("création de la Source")
        .set_len(10 * 1024 * 1024 * 1024 + 1)
        .expect("création d'une Source sparse hors budget");

    let output = env
        .command()
        .args([
            "capture",
            source.to_str().expect("chemin utf-8"),
            "--policy",
            "safe-local@1",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let job_id =
        serde_json::from_slice::<Value>(&output).expect("Job JSON valide")["job"]["job_id"]
            .as_str()
            .expect("identifiant de Job")
            .to_string();

    let finished: Value = serde_json::from_slice(
        &env.command()
            .args(["job", "wait", &job_id, "--timeout-secs", "5"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job JSON valide");
    assert_eq!(finished["state"], "failed");
    assert_eq!(finished["error"]["code"], "capture_failed");
    assert!(
        finished["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("disk budget"))
    );
}

#[test]
fn capture_rejects_a_proof_that_leaves_no_disk_budget_for_its_metadata() {
    let env = TestEnv::new("capture-metadata-budget");
    let source = env.work_dir.join("source.bin");
    fs::write(&source, b"x").expect("écriture de la Source");
    let job_id = "job-2";
    let job_path = write_capture_worker_job(&env, job_id, &source, &[], 30);
    let mut job: Value = serde_json::from_slice(&fs::read(&job_path).expect("lecture du Job"))
        .expect("Job JSON valide");
    job["policy"]["snapshot"]["limits"]["max_disk_bytes"] = Value::from(1_u64);
    fs::write(
        &job_path,
        serde_json::to_vec(&job).expect("sérialisation du Job modifié"),
    )
    .expect("écriture du Job modifié");

    env.command()
        .args(["capture-worker", "--job-id", job_id])
        .assert()
        .success();

    let finished: Value =
        serde_json::from_slice(&fs::read(job_path).expect("lecture du Job terminé"))
            .expect("Job JSON valide");
    assert_eq!(finished["state"], "failed");
    assert!(
        finished["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("disk budget"))
    );
}

#[test]
fn capture_stops_a_running_provider_at_its_duration_budget_before_starting_frames() {
    let env = TestEnv::new("capture-provider-duration-budget");
    env.write_config(&env.work_dir.join("out"));
    let source = env.write_media_file("interview.mp4");
    let audio_started = env.work_dir.join("audio-started");
    let frames_started = env.work_dir.join("frames-started");
    write_executable(
        &env.bin_dir,
        "ffmpeg",
        &format!(
            "#!/bin/sh\nset -eu\nfor arg in \"$@\"; do\n  if [ \"$arg\" = \"-map\" ]; then\n    : > \"{}\"\n    exit 0\n  fi\ndone\n: > \"{}\"\nwhile :; do :; done\n",
            frames_started.display(),
            audio_started.display(),
        ),
    );
    let job_id = "job-1";
    let job_path = write_capture_worker_job(
        &env,
        job_id,
        &source,
        &["ffmpeg", "ffprobe", "whisper-cli"],
        2,
    );

    let started = Instant::now();
    env.command()
        .args(["capture-worker", "--job-id", job_id])
        .assert()
        .success();

    let finished: Value =
        serde_json::from_slice(&fs::read(job_path).expect("lecture du Job terminé"))
            .expect("Job JSON valide");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(finished["state"], "partial");
    assert!(audio_started.exists());
    assert!(!frames_started.exists());
    let capture_id = finished["capture_id"]
        .as_str()
        .expect("la Capture partielle est publiée");
    let capture: Value = serde_json::from_slice(
        &env.command()
            .args(["capture", "inspect", capture_id])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Capture JSON valide");
    assert!(
        capture["manifest"]["capabilities"]
            .as_array()
            .is_some_and(|capabilities| capabilities.iter().any(|capability| {
                capability["name"] == "audio-extraction"
                    && capability["state"] == "failed"
                    && capability["error"]["message"]
                        .as_str()
                        .is_some_and(|message| message.contains("duration budget"))
            }))
    );
    assert!(
        capture["manifest"]["capabilities"]
            .as_array()
            .is_some_and(|capabilities| capabilities.iter().any(|capability| {
                capability["name"] == "transcription" && capability["state"] == "not_attempted"
            }))
    );
}

#[test]
fn capture_keeps_authorized_media_capabilities_when_policy_denies_only_frames() {
    let env = TestEnv::new("capture-partial-provider-policy");
    env.write_config(&env.work_dir.join("out"));
    let source = env.write_media_file("interview.mp4");
    let job_id = "job-1";
    let job_path = write_capture_worker_job(&env, job_id, &source, &["ffmpeg", "whisper-cli"], 30);

    env.command()
        .args(["capture-worker", "--job-id", job_id])
        .assert()
        .success();

    let finished: Value =
        serde_json::from_slice(&fs::read(job_path).expect("lecture du Job terminé"))
            .expect("Job JSON valide");
    assert_eq!(finished["state"], "partial");
    let capture_id = finished["capture_id"]
        .as_str()
        .expect("la Capture partielle est publiée");
    let capture: Value = serde_json::from_slice(
        &env.command()
            .args(["capture", "inspect", capture_id])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Capture JSON valide");
    let capabilities = capture["manifest"]["capabilities"]
        .as_array()
        .expect("capabilities présentes");
    assert!(capabilities.iter().any(|capability| {
        capability["name"] == "audio-extraction" && capability["state"] == "succeeded"
    }));
    assert!(capabilities.iter().any(|capability| {
        capability["name"] == "transcription" && capability["state"] == "succeeded"
    }));
    assert!(capabilities.iter().any(|capability| {
        capability["name"] == "frames"
            && capability["state"] == "not_attempted"
            && capability["provider"]["name"] == "ffprobe"
    }));
}

#[test]
fn capture_local_media_publishes_proof_and_located_extractions() {
    let env = TestEnv::new("capture-local-media");
    env.write_config(&env.work_dir.join("out"));
    let source = env.write_media_file("interview.MP4");

    let created: Value = serde_json::from_slice(
        &env.command()
            .args([
                "capture",
                source.to_str().expect("chemin utf-8"),
                "--policy",
                "safe-local@1",
            ])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job JSON valide");
    let job_id = created["job"]["job_id"]
        .as_str()
        .expect("identifiant de Job");
    let finished: Value = serde_json::from_slice(
        &env.command()
            .args(["job", "wait", job_id, "--timeout-secs", "5"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job JSON valide");
    assert_eq!(finished["state"], "succeeded");
    let capture_id = finished["capture_id"]
        .as_str()
        .expect("identifiant de Capture");

    let capture: Value = serde_json::from_slice(
        &env.command()
            .args(["capture", "inspect", capture_id])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Capture JSON valide");
    assert_eq!(capture["manifest"]["proof"]["path"], "proofs/source");
    assert_eq!(capture["manifest"]["proof"]["locator"]["kind"], "file");
    assert_eq!(capture["manifest"]["proof"]["mime"], "video/mp4");
    assert_eq!(
        capture["manifest"]["extractions"].as_array().map(Vec::len),
        Some(2)
    );
    assert!(
        capture["manifest"]["extractions"]
            .as_array()
            .is_some_and(|extractions| extractions.iter().any(|extraction| {
                extraction["artifact_id"] == "extraction-transcription"
                    && extraction["path"] == "extractions/transcription.txt"
                    && extraction["locator"].is_null()
                    && extraction["provider"]["name"] == "whisper-cli"
                    && extraction["provider"]["version"].as_str().is_some()
                    && extraction["provider"]["parameters"]["model"]["sha256"]
                        .as_str()
                        .is_some()
            }))
    );
    assert!(
        capture["manifest"]["extractions"]
            .as_array()
            .is_some_and(|extractions| extractions.iter().any(|extraction| {
                extraction["artifact_id"] == "extraction-frame-0000"
                    && extraction["locator"]["kind"] == "media-timestamp"
                    && extraction["provider"]["name"] == "ffmpeg"
                    && extraction["provider"]["version"].as_str().is_some()
                    && extraction["provider"]["dependencies"][0]["name"] == "ffprobe"
            }))
    );

    assert_readable_transcription(&env, capture_id, &capture);
}

#[test]
fn capture_local_media_publishes_partial_results_when_transcription_capability_fails() {
    let env = TestEnv::new("capture-media-partial");
    env.write_config(&env.work_dir.join("out"));
    write_executable(&env.bin_dir, "whisper-cli", FAKE_WHISPER_CLI_FAILURE);
    let source = env.write_media_file("interview.M4A");

    let created: Value = serde_json::from_slice(
        &env.command()
            .args([
                "capture",
                source.to_str().expect("chemin utf-8"),
                "--policy",
                "safe-local@1",
            ])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job JSON valide");
    let job_id = created["job"]["job_id"]
        .as_str()
        .expect("identifiant de Job");
    let finished: Value = serde_json::from_slice(
        &env.command()
            .args(["job", "wait", job_id, "--timeout-secs", "5"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job JSON valide");
    assert_eq!(finished["state"], "partial");
    let capture_id = finished["capture_id"]
        .as_str()
        .expect("une Capture partielle reste inspectable");

    let capture: Value = serde_json::from_slice(
        &env.command()
            .args(["capture", "inspect", capture_id])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Capture JSON valide");
    assert_eq!(capture["manifest"]["proof"]["path"], "proofs/source");
    assert_eq!(capture["manifest"]["proof"]["mime"], "audio/mp4");
    assert!(
        capture["manifest"]["extractions"]
            .as_array()
            .is_some_and(|extractions| {
                extractions
                    .iter()
                    .any(|extraction| extraction["artifact_id"] == "extraction-frame-0000")
            })
    );
    assert!(
        capture["manifest"]["capabilities"]
            .as_array()
            .is_some_and(|capabilities| capabilities.iter().any(|capability| {
                capability["name"] == "transcription"
                    && capability["state"] == "failed"
                    && capability["error"]["code"] == "capability_failed"
                    && capability["error"]["message"]
                        .as_str()
                        .is_some_and(|message| message.contains("fake whisper-cli failure"))
            }))
    );
}

#[test]
fn local_pdf_publishes_a_traced_text_extraction_and_capability() {
    let env = TestEnv::new("capture-pdf");
    env.install_binary("pdfinfo", FAKE_PDFINFO);
    env.install_binary("pdftotext", FAKE_PDFTOTEXT);
    let source = env.work_dir.join("contract.pdf");
    fs::write(&source, b"original pdf bytes").expect("écriture du PDF");
    let job_id = "job-42";
    let job_path = write_capture_worker_job(&env, job_id, &source, &["pdftotext", "pdfinfo"], 30);

    env.command()
        .args(["capture-worker", "--job-id", job_id])
        .assert()
        .success();

    let job: Value = serde_json::from_slice(&fs::read(&job_path).expect("lecture du Job"))
        .expect("Job JSON valide");
    assert_eq!(job["state"], "succeeded");
    let capture_id = job["capture_id"].as_str().expect("identifiant de Capture");
    let capture: Value = serde_json::from_slice(
        &env.command()
            .args(["capture", "inspect", capture_id])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Capture JSON valide");

    assert_eq!(capture["manifest"]["proof"]["mime"], "application/pdf");
    assert!(capture["manifest"]["proof"]["created_at"].is_u64());
    assert!(
        capture["manifest"]["proof"]["created_at"]
            .as_u64()
            .is_some_and(|proof_created_at| {
                capture["manifest"]["extractions"][0]["created_at"]
                    .as_u64()
                    .is_some_and(|extraction_created_at| proof_created_at <= extraction_created_at)
            }),
        "la preuve est horodatée lors de sa copie, avant l'extraction"
    );
    assert_eq!(
        capture["manifest"]["extractions"][0]["provider"]["name"],
        "pdftotext"
    );
    assert_eq!(
        capture["manifest"]["extractions"][0]["provider"]["parameters"],
        serde_json::json!({ "arguments": ["-layout"] })
    );
    assert_eq!(
        capture["manifest"]["extractions"][0]["locator"]["kind"],
        "pdf-pages"
    );
    assert_eq!(
        capture["manifest"]["extractions"][0]["locator"]["last_page"],
        2
    );
    assert_eq!(
        capture["manifest"]["extractions"][0]["locator_provider"]["name"],
        "pdfinfo"
    );
    assert!(
        capture["manifest"]["capabilities"]
            .as_array()
            .is_some_and(|capabilities| capabilities.iter().any(|capability| {
                capability["name"] == "pdf-text-extraction" && capability["state"] == "succeeded"
            }))
    );
}

#[test]
fn document_provider_refusal_is_a_not_attempted_capability() {
    let env = TestEnv::new("capture-pdf-policy");
    let marker = env.work_dir.join("pdftotext-invoked");
    env.install_binary(
        "pdftotext",
        &format!("#!/bin/sh\nprintf invoked > '{}'\n", marker.display()),
    );
    let source = env.work_dir.join("contract.pdf");
    fs::write(&source, b"original pdf bytes").expect("écriture du PDF");
    let job_id = "job-43";
    let job_path = write_capture_worker_job(&env, job_id, &source, &[], 30);

    env.command()
        .args(["capture-worker", "--job-id", job_id])
        .assert()
        .success();

    assert!(
        !marker.exists(),
        "un Provider refusé ne doit pas être invoqué"
    );
    let job: Value = serde_json::from_slice(&fs::read(job_path).expect("lecture du Job"))
        .expect("Job JSON valide");
    assert_eq!(job["state"], "partial");
    let capture_id = job["capture_id"].as_str().expect("identifiant de Capture");
    let capture: Value = serde_json::from_slice(
        &env.command()
            .args(["capture", "inspect", capture_id])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Capture JSON valide");
    assert!(
        capture["manifest"]["capabilities"]
            .as_array()
            .is_some_and(|capabilities| capabilities.iter().any(|capability| {
                capability["name"] == "pdf-text-extraction"
                    && capability["state"] == "not_attempted"
                    && capability["error"]["code"] == "provider_not_allowed"
                    && capability["provider"]["name"] == "pdftotext"
            }))
    );
}

#[test]
fn local_image_ocr_keeps_typed_regions_and_capability_failures() {
    let env = TestEnv::new("capture-image-ocr");
    env.install_binary("tesseract", FAKE_TESSERACT);
    let source = env.work_dir.join("receipt.png");
    fs::write(&source, b"original image bytes").expect("écriture de l'image");
    let job_id = "job-44";
    let job_path = write_capture_worker_job(&env, job_id, &source, &["tesseract"], 30);

    env.command()
        .args(["capture-worker", "--job-id", job_id])
        .assert()
        .success();

    let job: Value = serde_json::from_slice(&fs::read(&job_path).expect("lecture du Job"))
        .expect("Job JSON valide");
    assert_eq!(job["state"], "succeeded");
    let capture_id = job["capture_id"].as_str().expect("identifiant de Capture");
    let capture: Value = serde_json::from_slice(
        &env.command()
            .args(["capture", "inspect", capture_id])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Capture JSON valide");
    assert_eq!(
        capture["manifest"]["extractions"][0]["locator"]["kind"],
        "image-regions"
    );
    assert_eq!(
        capture["manifest"]["extractions"][0]["locator"]["regions"][0]["left"],
        12
    );
    assert!(
        capture["manifest"]["capabilities"]
            .as_array()
            .is_some_and(|capabilities| capabilities.iter().any(|capability| {
                capability["name"] == "image-ocr" && capability["state"] == "succeeded"
            }))
    );

    env.install_binary("tesseract", FAKE_TESSERACT_FAILURE);
    let failed_source = env.work_dir.join("failed-receipt.png");
    fs::write(&failed_source, b"other image bytes").expect("écriture de l'image");
    let failed_job_id = "job-45";
    let failed_job_path =
        write_capture_worker_job(&env, failed_job_id, &failed_source, &["tesseract"], 30);
    env.command()
        .args(["capture-worker", "--job-id", failed_job_id])
        .assert()
        .success();
    let failed_job: Value =
        serde_json::from_slice(&fs::read(failed_job_path).expect("lecture du Job"))
            .expect("Job JSON valide");
    assert_eq!(failed_job["state"], "partial");
    let failed_capture_id = failed_job["capture_id"]
        .as_str()
        .expect("identifiant de Capture");
    let failed_capture: Value = serde_json::from_slice(
        &env.command()
            .args(["capture", "inspect", failed_capture_id])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Capture JSON valide");
    assert!(
        failed_capture["manifest"]["capabilities"]
            .as_array()
            .is_some_and(|capabilities| capabilities.iter().any(|capability| {
                capability["name"] == "image-ocr"
                    && capability["state"] == "failed"
                    && capability["error"]["code"] == "extraction_failed"
            }))
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
