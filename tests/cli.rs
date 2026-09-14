//! Tests d'intégration du contrat `capture` du binaire `scriptor`. Les
//! Providers externes sont remplacés par des scripts shell injectés dans le
//! `PATH` du test.

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

const FAKE_LOCAL_DERIVE_PROVIDER: &str = r#"#!/bin/sh
set -eu
request=""
output=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --request) request="$2"; shift 2 ;;
    --output) output="$2"; shift 2 ;;
    *) shift ;;
  esac
done
while IFS= read -r line || [ -n "$line" ]; do
  printf '%s\n' "$line"
done < "$request" > "$XDG_CACHE_HOME/derive-request.json"
printf '# Note derivee\n\nContenu local.\n' > "$output"
printf '{"mime":"text/markdown","effective_parameters":{"style":"concise"}}'
"#;

const FAKE_LOCAL_DERIVE_PROVIDER_FAILURE: &str = r"#!/bin/sh
echo 'modele local indisponible' >&2
exit 1
";

const FAKE_LOCAL_DERIVE_PROVIDER_SECRET: &str = r#"#!/bin/sh
set -eu
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) output="$2"; shift 2 ;;
    *) shift ;;
  esac
done
printf 'contenu' > "$output"
printf '{"mime":"text/plain","effective_parameters":{"api_token":"secret"}}'
"#;

const FAKE_LOCAL_DERIVE_PROVIDER_BLOCKING: &str = r#"#!/bin/sh
set -eu
printf 'started' > "$XDG_CACHE_HOME/derive-started"
while [ ! -f "$XDG_CACHE_HOME/derive-release" ]; do :; done
"#;

const FAKE_PAGE_RENDERER: &str = r#"#!/bin/sh
set -eu
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--output-dir" ]; then out="$2"; break; fi
  shift
done
printf '<main>preuve</main>' > "$out/proofs/dom.html"
printf 'preuve' > "$out/proofs/screenshot.png"
printf '# contenu\n' > "$out/extractions/page.md"
printf '[{"url":"https://93.184.216.34/document.pdf","parent_locator":{"kind":"url","value":"https://93.184.216.34/"},"locator":{"kind":"css-selector","value":"html > body:nth-of-type(1) > a:nth-of-type(1)"},"order":0,"status":"inventoried","reason":"linked_document"}]' > "$out/discoveries.json"
printf '{"final_url":"https://93.184.216.34/"}' > "$out/provenance.json"
"#;

const FAKE_PAGE_RENDERER_PRIVATE_TARGET: &str =
    "#!/bin/sh\necho web_private_target_refused >&2\nexit 1\n";

const FAKE_PAGE_RENDERER_SKIPPED_DISCOVERY: &str = r#"#!/bin/sh
set -eu
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--output-dir" ]; then out="$2"; break; fi
  shift
done
printf '<main>preuve</main>' > "$out/proofs/dom.html"
printf 'preuve' > "$out/proofs/screenshot.png"
printf '# contenu\n' > "$out/extractions/page.md"
printf '[{"url":"https://93.184.216.34/document.pdf","parent_locator":{"kind":"url","value":"https://93.184.216.34/"},"locator":{"kind":"css-selector","value":"a"},"order":0,"status":"skipped_budget","reason":"budget"}]' > "$out/discoveries.json"
printf '{"final_url":"https://93.184.216.34/"}' > "$out/provenance.json"
"#;

const FAKE_BINARY_ACQUIRER: &str = r#"#!/bin/sh
set -eu
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--output-dir" ]; then out="$2"; break; fi
  shift
done
printf '%s' '%PDF-1.4' > "$out/payload"
printf '{"requested_url":"https://93.184.216.34/document.pdf","final_url":"https://93.184.216.34/document.pdf","mime":"application/pdf","sha256":"e16fa5d9b51928755db85b917f0297babaf22c7a47e97d9212adab56e61ba04e","size_bytes":8}' > "$out/metadata.json"
"#;

const FAKE_INSTAGRAM_YT_DLP: &str = r#"#!/bin/sh
set -eu
if [ "$1" = "--version" ]; then printf '2026.08.19\n'; exit 0; fi
printf '{"id":"post-1","webpage_url":"https://www.instagram.com/p/post-1/","uploader":"alice","upload_date":"20260914","description":"Caption Instagram complete","thumbnails":[{"url":"https://93.184.216.34/photo.jpg"}]}'
"#;

const FAKE_INSTAGRAM_VIDEO_YT_DLP: &str = r#"#!/bin/sh
set -eu
if [ "$1" = "--version" ]; then printf '2026.08.19\n'; exit 0; fi
printf '{"id":"video-1","webpage_url":"https://www.instagram.com/reel/video-1/","uploader":"alice","upload_date":"20260914","description":"Caption Instagram video","formats":[{"url":"https://93.184.216.34/video.mp4","vcodec":"avc1"}]}'
"#;

const FAKE_INSTAGRAM_BINARY_ACQUIRER: &str = r#"#!/bin/sh
set -eu
url=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --url) url="$2"; shift 2 ;;
    --output-dir) out="$2"; shift 2 ;;
    *) shift ;;
  esac
done
printf 'photo' > "$out/payload"
printf '{"requested_url":"%s","final_url":"%s","mime":"image/jpeg","sha256":"55c64d0fcd6f9d5f7c828093857e3fdfda68478bb4e9bd24d481ef391c7804e8","size_bytes":5,"redirect_chain":["%s"]}' "$url" "$url" "$url" > "$out/metadata.json"
"#;

const FAKE_INSTAGRAM_VIDEO_BINARY_ACQUIRER: &str = r#"#!/bin/sh
set -eu
url=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --url) url="$2"; shift 2 ;;
    --output-dir) out="$2"; shift 2 ;;
    *) shift ;;
  esac
done
printf 'video' > "$out/payload"
printf '{"requested_url":"%s","final_url":"%s","mime":"video/mp4","sha256":"0cab1c9617404faf2b24e221e189ca5945813e14d3f766345b09ca13bbe28ffc","size_bytes":5,"redirect_chain":["%s"]}' "$url" "$url" "$url" > "$out/metadata.json"
"#;

const FAKE_LINKEDIN_BINARY_ACQUIRER: &str = r#"#!/bin/sh
set -eu
url=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --url) url="$2"; shift 2 ;;
    --output-dir) out="$2"; shift 2 ;;
    *) shift ;;
  esac
done
if [ "$url" = "https://www.linkedin.com/posts/post-1/" ]; then
  printf '<script type="application/ld+json">{"@type":"SocialMediaPosting","articleBody":"Caption LinkedIn complete","image":{"url":"https://93.184.216.34/linkedin-photo.jpg"},"url":"https://www.linkedin.com/posts/post-1/","identifier":"post-1","author":{"name":"Alice"},"datePublished":"2026-09-14"}</script>' > "$out/payload"
  printf '{"requested_url":"%s","final_url":"%s","mime":"text/html","sha256":"4dd05b7a8767936145cbbbba6d585558624b9fe118fd28e68084a704c572d9c6","size_bytes":299,"redirect_chain":["%s"]}' "$url" "$url" "$url" > "$out/metadata.json"
else
  printf 'photo' > "$out/payload"
  printf '{"requested_url":"%s","final_url":"%s","mime":"image/jpeg","sha256":"55c64d0fcd6f9d5f7c828093857e3fdfda68478bb4e9bd24d481ef391c7804e8","size_bytes":5,"redirect_chain":["%s"]}' "$url" "$url" "$url" > "$out/metadata.json"
fi
"#;

const FAKE_PAGE_RENDERER_TREE: &str = r#"#!/bin/sh
set -eu
url=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --url) url="$2"; shift 2 ;;
    --output-dir) out="$2"; shift 2 ;;
    *) shift ;;
  esac
done
case "$url" in
  https://93.184.216.34/tree-root)
    final="$url"
    discoveries='[{"url":"https://93.184.216.34/tree-a","parent_locator":{"kind":"url","value":"https://93.184.216.34/tree-root"},"locator":{"kind":"css-selector","value":"iframe:nth-of-type(1)"},"order":0,"status":"skipped_budget","reason":"budget","kind":"web"},{"url":"https://93.184.216.34/tree-b","parent_locator":{"kind":"url","value":"https://93.184.216.34/tree-root"},"locator":{"kind":"css-selector","value":"iframe:nth-of-type(2)"},"order":1,"status":"skipped_budget","reason":"budget","kind":"web"}]'
    ;;
  https://93.184.216.34/tree-a)
    final="https://93.184.216.34/tree-a-final"
    discoveries='[{"url":"https://93.184.216.34/tree-a1","parent_locator":{"kind":"url","value":"https://93.184.216.34/tree-a-final"},"locator":{"kind":"css-selector","value":"iframe:nth-of-type(1)"},"order":0,"status":"inventoried","reason":"embedded_content","kind":"web"},{"url":"https://93.184.216.34/tree-a2","parent_locator":{"kind":"url","value":"https://93.184.216.34/tree-a-final"},"locator":{"kind":"css-selector","value":"iframe:nth-of-type(2)"},"order":1,"status":"inventoried","reason":"embedded_content","kind":"web"}]'
    ;;
  https://93.184.216.34/tree-b)
    final="$url"
    discoveries='[{"url":"https://93.184.216.34/tree-b1","parent_locator":{"kind":"url","value":"https://93.184.216.34/tree-b"},"locator":{"kind":"css-selector","value":"iframe:nth-of-type(1)"},"order":0,"status":"inventoried","reason":"embedded_content","kind":"web"}]'
    ;;
  *)
    final="$url"
    discoveries='[]'
    ;;
esac
printf '<main>preuve</main>' > "$out/proofs/dom.html"
printf 'preuve' > "$out/proofs/screenshot.png"
printf '# contenu\n' > "$out/extractions/page.md"
printf '%s' "$discoveries" > "$out/discoveries.json"
printf '{"initial_url":"%s","final_url":"%s","redirect_chain":["%s","%s"]}' "$url" "$final" "$url" "$final" > "$out/provenance.json"
if [ "$url" != "https://93.184.216.34/tree-root" ]; then
  printf '%s\n' "$url" >> "$XDG_CACHE_HOME/renderer-order"
fi
"#;

const FAKE_PAGE_RENDERER_RESOLUTION_CASES: &str = r#"#!/bin/sh
set -eu
url=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --url) url="$2"; shift 2 ;;
    --output-dir) out="$2"; shift 2 ;;
    *) shift ;;
  esac
done
case "$url" in
  https://93.184.216.34/many-root)
    discoveries="["
    i=0
    while [ "$i" -lt 51 ]; do
      [ "$i" -eq 0 ] || discoveries="$discoveries,"
      discoveries="$discoveries{\"url\":\"https://93.184.216.34/many-$i\",\"parent_locator\":{\"kind\":\"url\",\"value\":\"$url\"},\"locator\":{\"kind\":\"css-selector\",\"value\":\"iframe:nth-of-type($i)\"},\"order\":$i,\"status\":\"skipped_budget\",\"reason\":\"budget\",\"kind\":\"web\"}"
      i=$((i + 1))
    done
    discoveries="$discoveries]"
    final="$url"
    ;;
  https://93.184.216.34/cycle-root)
    discoveries='[{"url":"https://93.184.216.34/cycle-child","parent_locator":{"kind":"url","value":"https://93.184.216.34/cycle-root"},"locator":{"kind":"css-selector","value":"iframe"},"order":0,"status":"skipped_budget","reason":"budget","kind":"web"}]'
    final="$url"
    ;;
  https://93.184.216.34/cycle-child)
    discoveries='[]'
    final="https://93.184.216.34/cycle-root"
    ;;
  https://93.184.216.34/slow-root)
    discoveries='[{"url":"https://93.184.216.34/slow-child","parent_locator":{"kind":"url","value":"https://93.184.216.34/slow-root"},"locator":{"kind":"css-selector","value":"iframe"},"order":0,"status":"skipped_budget","reason":"budget","kind":"web"}]'
    final="$url"
    ;;
  https://93.184.216.34/slow-child)
    printf 'x' > "$XDG_CACHE_HOME/renderer-started"
    while :; do :; done
    ;;
  https://93.184.216.34/crash-root)
    discoveries='[{"url":"https://93.184.216.34/crash-child","parent_locator":{"kind":"url","value":"https://93.184.216.34/crash-root"},"locator":{"kind":"css-selector","value":"iframe"},"order":0,"status":"skipped_budget","reason":"budget","kind":"web"}]'
    final="$url"
    ;;
  https://93.184.216.34/crash-child)
    exit 1
    ;;
  *)
    discoveries='[]'
    final="$url"
    ;;
esac
printf '<main>preuve</main>' > "$out/proofs/dom.html"
printf 'preuve' > "$out/proofs/screenshot.png"
printf '# contenu\n' > "$out/extractions/page.md"
printf '%s' "$discoveries" > "$out/discoveries.json"
printf '{"initial_url":"%s","final_url":"%s","redirect_chain":["%s","%s"]}' "$url" "$final" "$url" "$final" > "$out/provenance.json"
case "$url" in
  https://93.184.216.34/many-[0-9]*) printf '%s\n' "$url" >> "$XDG_CACHE_HOME/renderer-order" ;;
esac
"#;

const FAKE_PAGE_RENDERER_MIME_LIES: &str = r#"#!/bin/sh
set -eu
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output-dir) out="$2"; shift 2 ;;
    *) shift ;;
  esac
done
printf '<main>preuve</main>' > "$out/proofs/dom.html"
printf 'preuve' > "$out/proofs/screenshot.png"
printf '# contenu\n' > "$out/extractions/page.md"
printf '[{"url":"https://93.184.216.34/fake-video","parent_locator":{"kind":"url","value":"https://93.184.216.34/mime-root"},"locator":{"kind":"css-selector","value":"video"},"order":0,"status":"skipped_budget","reason":"budget"},{"url":"https://93.184.216.34/fake-audio","parent_locator":{"kind":"url","value":"https://93.184.216.34/mime-root"},"locator":{"kind":"css-selector","value":"audio"},"order":1,"status":"skipped_budget","reason":"budget"}]' > "$out/discoveries.json"
printf '{"initial_url":"https://93.184.216.34/mime-root","final_url":"https://93.184.216.34/mime-root","redirect_chain":["https://93.184.216.34/mime-root"]}' > "$out/provenance.json"
"#;

const FAKE_BINARY_ACQUIRER_MIME_LIES: &str = r#"#!/bin/sh
set -eu
url=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --url) url="$2"; shift 2 ;;
    --output-dir) out="$2"; shift 2 ;;
    *) shift ;;
  esac
done
case "$url" in
  *fake-video) mime="video/mp4" ;;
  *fake-audio) mime="audio/mpeg" ;;
  *) exit 1 ;;
esac
printf 'not media' > "$out/payload"
printf '{"requested_url":"%s","final_url":"%s","mime":"%s","sha256":"58aa1545ba429abc10d60b9b4431ee615d7e87e3ddcf15904f7199efe6fc8570","size_bytes":9,"redirect_chain":["%s"]}' "$url" "$url" "$mime" "$url" > "$out/metadata.json"
"#;

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

        Self {
            bin_dir,
            xdg_config,
            xdg_cache,
            xdg_data,
            work_dir,
        }
    }

    fn write_config(&self, _output_dir: &Path) {
        let config_dir = self.xdg_config.join("scriptor");
        fs::create_dir_all(&config_dir).expect("création du répertoire de config de test");
        let config = format!(
            "model = \"small\"\nmodels_dir = \"{}\"\nlanguage = \"en\"\nthreads = 1\n",
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

fn wait_for_job_state(path: &Path, state: &str, timeout: Duration) -> bool {
    let deadline = Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(Instant::now);
    while Instant::now() < deadline {
        if fs::read(path)
            .ok()
            .and_then(|content| serde_json::from_slice::<Value>(&content).ok())
            .is_some_and(|job| job["state"] == state)
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn create_safe_web_capture(env: &TestEnv, source: &str) -> Value {
    serde_json::from_slice(
        &env.command()
            .args(["capture", source, "--policy", "safe-web@1"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job JSON valide")
}

fn wait_for_agent_job(env: &TestEnv, job_id: &str) -> Value {
    serde_json::from_slice(
        &env.command()
            .args(["job", "wait", job_id, "--timeout-secs", "10"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job JSON valide")
}

fn continue_safe_web_capture(env: &TestEnv, capture_id: &str) -> Value {
    serde_json::from_slice(
        &env.command()
            .args(["capture", "continue", capture_id, "--policy", "safe-web@1"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job de continuation JSON valide")
}

fn inspect_agent_capture(env: &TestEnv, capture_id: &str) -> Value {
    serde_json::from_slice(
        &env.command()
            .args(["capture", "inspect", capture_id])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Capture JSON valide")
}

fn create_text_capture(env: &TestEnv) -> String {
    let source = env.work_dir.join("notes.txt");
    fs::write(&source, "Une preuve locale.\n").expect("ecriture de la Source texte");
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
    .expect("Job de Capture JSON valide");
    let job_id = created["job"]["job_id"]
        .as_str()
        .expect("identifiant du Job de Capture");
    let finished = wait_for_agent_job(env, job_id);
    assert_eq!(finished["state"], "succeeded", "{finished}");
    finished["capture_id"]
        .as_str()
        .expect("identifiant de Capture")
        .to_string()
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
    assert_eq!(finished["state"], "succeeded", "{finished}");
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
    let captures = env.xdg_data.join("scriptor/v2/captures");
    assert!(
        !captures.exists()
            || fs::read_dir(captures)
                .expect("lecture des Captures")
                .next()
                .is_none(),
        "un staging en échec ne doit pas rester sur disque"
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
    assert!(
        fs::read_dir(env.xdg_data.join("scriptor/v2/captures"))
            .expect("lecture des Captures")
            .next()
            .is_none(),
        "un staging en échec ne doit pas rester sur disque"
    );
}

#[test]
fn safe_web_publishes_a_portable_capture() {
    let env = TestEnv::new("safe-web");
    env.install_binary("scriptor-page-renderer", FAKE_PAGE_RENDERER);
    let created: Value = serde_json::from_slice(
        &env.command()
            .args([
                "capture",
                "https://93.184.216.34/",
                "--policy",
                "safe-web@1",
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
    let job: Value = serde_json::from_slice(
        &env.command()
            .args(["job", "wait", job_id, "--timeout-secs", "5"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job JSON valide");
    assert_eq!(job["state"], "succeeded", "{job}");
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
    assert_eq!(capture["manifest"]["proof"]["artifact_id"], "proof-dom");
    assert_eq!(
        capture["manifest"]["extractions"][0]["artifact_id"],
        "extraction-markdown"
    );
    assert_eq!(
        capture["manifest"]["discoveries"][0]["parent"]["artifact_id"],
        "proof-dom"
    );
    assert_eq!(capture["manifest"]["discoveries"][0]["order"], 0);
    let provenance = &capture["manifest"]["remote_provenance"];
    assert_eq!(provenance["requested_url"], "https://93.184.216.34/");
    assert_eq!(provenance["final_url"], "https://93.184.216.34/");
    assert_eq!(provenance["mime"], "text/html");
    assert!(provenance["sha256"].is_string());
    assert!(provenance["size_bytes"].is_u64());
    assert!(provenance["redirect_chain"].is_array());
    env.command()
        .args(["capture", "search", "preuve"])
        .assert()
        .success()
        .stdout(predicate::str::contains(capture_id));
    let duplicate: Value = serde_json::from_slice(
        &env.command()
            .args([
                "capture",
                "https://93.184.216.34/",
                "--policy",
                "safe-web@1",
            ])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job de Doublon JSON valide");
    let duplicate_job = duplicate["job"]["job_id"]
        .as_str()
        .expect("identifiant du Job de Doublon");
    let reused: Value = serde_json::from_slice(
        &env.command()
            .args(["job", "wait", duplicate_job, "--timeout-secs", "5"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job de Doublon terminé JSON valide");
    assert_eq!(reused["state"], "succeeded");
    assert_eq!(reused["capture_id"], capture_id);
}

#[test]
fn capture_instagram_photo_preserves_caption_and_media_without_renderer() {
    let env = TestEnv::new("instagram-photo");
    env.install_binary("yt-dlp", FAKE_INSTAGRAM_YT_DLP);
    env.install_binary("scriptor-binary-acquirer", FAKE_INSTAGRAM_BINARY_ACQUIRER);
    env.install_binary("scriptor-page-renderer", "#!/bin/sh\nexit 99\n");
    let created: Value = serde_json::from_slice(
        &env.command()
            .args([
                "capture",
                "https://www.instagram.com/p/post-1/",
                "--policy",
                "safe-web@1",
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
    let job: Value = serde_json::from_slice(
        &env.command()
            .args(["job", "wait", job_id, "--timeout-secs", "5"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job terminé JSON valide");
    assert_eq!(job["state"], "succeeded", "{job}");
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
    let manifest = &capture["manifest"];
    assert_eq!(manifest["proof"]["artifact_id"], "proof-instagram-metadata");
    assert_eq!(manifest["extractions"][0]["size_bytes"], 26);
    assert_eq!(manifest["artifacts"][0]["order"], 0);
    assert_eq!(manifest["artifacts"][0]["mime"], "image/jpeg");
    assert_eq!(manifest["remote_provenance"]["media"][0]["size_bytes"], 5);
    assert_eq!(manifest["capabilities"][1]["state"], "succeeded");
}

#[test]
fn capture_instagram_video_transcribes_extracts_frames_and_indexes_text() {
    let env = TestEnv::new("instagram-video");
    env.write_config(&env.work_dir.join("unused"));
    env.install_binary("yt-dlp", FAKE_INSTAGRAM_VIDEO_YT_DLP);
    env.install_binary(
        "scriptor-binary-acquirer",
        FAKE_INSTAGRAM_VIDEO_BINARY_ACQUIRER,
    );
    env.install_binary("scriptor-page-renderer", "#!/bin/sh\nexit 99\n");
    let created: Value = serde_json::from_slice(
        &env.command()
            .args([
                "capture",
                "https://www.instagram.com/reel/video-1/",
                "--policy",
                "safe-web@1",
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
    let job = wait_for_agent_job(&env, job_id);
    assert_eq!(job["state"], "succeeded", "{job}");
    let capture_id = job["capture_id"].as_str().expect("identifiant de Capture");
    let capture = inspect_agent_capture(&env, capture_id);
    let extractions = capture["manifest"]["extractions"]
        .as_array()
        .expect("extractions sociales");
    assert!(extractions.iter().any(|extraction| {
        extraction["artifact_id"] == "extraction-media-0-transcription"
            && extraction["path"] == "extractions/media-0/transcription.txt"
            && extraction["proof_artifact_id"] == "media-0"
    }));
    assert!(extractions.iter().any(|extraction| {
        extraction["artifact_id"] == "extraction-media-0-frame-0000"
            && extraction["path"] == "extractions/media-0/frames/frame-0001-0.000s.jpg"
            && extraction["proof_artifact_id"] == "media-0"
    }));
    let search: Value = serde_json::from_slice(
        &env.command()
            .args(["capture", "search", "faux contenu transcrit"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("résultat de recherche JSON valide");
    assert_eq!(search["captures"][0]["capture_id"], capture_id);
}

#[test]
fn capture_linkedin_photo_preserves_caption_and_media_without_renderer() {
    let env = TestEnv::new("linkedin-photo");
    env.install_binary("scriptor-binary-acquirer", FAKE_LINKEDIN_BINARY_ACQUIRER);
    env.install_binary("scriptor-page-renderer", "#!/bin/sh\nexit 99\n");
    let created: Value = serde_json::from_slice(
        &env.command()
            .args([
                "capture",
                "https://www.linkedin.com/posts/post-1/",
                "--policy",
                "safe-web@1",
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
    let job: Value = serde_json::from_slice(
        &env.command()
            .args(["job", "wait", job_id, "--timeout-secs", "5"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job terminé JSON valide");
    assert_eq!(job["state"], "succeeded", "{job}");
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
    let manifest = &capture["manifest"];
    assert_eq!(manifest["proof"]["artifact_id"], "proof-linkedin-metadata");
    assert_eq!(manifest["extractions"][0]["size_bytes"], 25);
    assert_eq!(manifest["artifacts"][0]["order"], 0);
    assert_eq!(manifest["artifacts"][0]["mime"], "image/jpeg");
    assert_eq!(manifest["remote_provenance"]["platform"], "linkedin");
    assert_eq!(manifest["remote_provenance"]["media"][0]["size_bytes"], 5);
}

#[test]
#[allow(clippy::too_many_lines)]
fn capture_continue_publishes_a_discovered_pdf_without_mutating_its_parent() {
    let env = TestEnv::new("continue-web-discovery");
    env.install_binary(
        "scriptor-page-renderer",
        FAKE_PAGE_RENDERER_SKIPPED_DISCOVERY,
    );
    env.install_binary("scriptor-binary-acquirer", FAKE_BINARY_ACQUIRER);
    env.install_binary("pdfinfo", FAKE_PDFINFO);
    env.install_binary("pdftotext", FAKE_PDFTOTEXT);
    let root: Value = serde_json::from_slice(
        &env.command()
            .args([
                "capture",
                "https://93.184.216.34/",
                "--policy",
                "safe-web@1",
            ])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job JSON valide");
    let root_job = root["job"]["job_id"].as_str().expect("Job racine");
    let root_done: Value = serde_json::from_slice(
        &env.command()
            .args(["job", "wait", root_job, "--timeout-secs", "5"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job racine fini");
    let capture_id = root_done["capture_id"].as_str().expect("Capture racine");
    let root_capture: Value = serde_json::from_slice(
        &env.command()
            .args(["capture", "inspect", capture_id])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Capture racine inspectée");
    let discovery_id = root_capture["manifest"]["discoveries"][0]["discovery_id"]
        .as_str()
        .expect("identifiant de Découverte");
    let continued: Value = serde_json::from_slice(
        &env.command()
            .args([
                "capture",
                "continue",
                capture_id,
                "--policy",
                "safe-web@1",
                "--discovery",
                discovery_id,
            ])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job continue JSON");
    assert!(continued["job"].is_object(), "{continued}");
    let continue_id = continued["job"]["job_id"].as_str().expect("Job continue");
    let finished: Value = serde_json::from_slice(
        &env.command()
            .args(["job", "wait", continue_id, "--timeout-secs", "5"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job continue fini");
    assert_eq!(finished["state"], "succeeded", "{finished}");
    assert_eq!(
        finished["child_capture_ids"].as_array().map(Vec::len),
        Some(1)
    );
    let child_id = finished["child_capture_ids"][0]
        .as_str()
        .expect("Capture enfant");
    let child: Value = serde_json::from_slice(
        &env.command()
            .args(["capture", "inspect", child_id])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Capture enfant inspectée");
    assert_eq!(
        child["manifest"]["remote_provenance"]["requested_url"],
        "https://93.184.216.34/document.pdf"
    );
    assert_eq!(
        child["manifest"]["source"]["locator"],
        "https://93.184.216.34/document.pdf"
    );
    let parent: Value = serde_json::from_slice(
        &env.command()
            .args(["capture", "inspect", capture_id])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Capture parent");
    assert_eq!(
        parent["manifest"]["discoveries"][0]["status"],
        "skipped_budget"
    );
    assert_eq!(
        parent["ledger"].as_array().map(Vec::len),
        Some(2),
        "{parent}"
    );
    assert_eq!(
        parent["ledger"][1]["details"]["status"], "captured",
        "{parent}"
    );
    assert_eq!(
        parent["ledger"][1]["details"]["requested_url"],
        "https://93.184.216.34/document.pdf"
    );
    assert_eq!(
        parent["ledger"][1]["details"]["sha256"],
        "e16fa5d9b51928755db85b917f0297babaf22c7a47e97d9212adab56e61ba04e"
    );
    let duplicate: Value = serde_json::from_slice(
        &env.command()
            .args([
                "capture",
                "continue",
                capture_id,
                "--policy",
                "safe-web@1",
                "--discovery",
                discovery_id,
            ])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job de Doublon JSON");
    let duplicate_id = duplicate["job"]["job_id"].as_str().expect("Job de Doublon");
    env.command()
        .args(["job", "wait", duplicate_id, "--timeout-secs", "5"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"state\":\"succeeded\""));
    let parent: Value = serde_json::from_slice(
        &env.command()
            .args(["capture", "inspect", capture_id])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Capture parent avec Doublon");
    assert_eq!(parent["ledger"][2]["details"]["status"], "reused");
    let reference = &parent["ledger"][2]["details"]["reference"];
    assert_eq!(reference["capture_id"], child_id);
    assert!(reference["artifact_id"].is_string());
    assert!(reference["sha256"].is_string());
    assert!(reference["locator"].is_object());
}

#[test]
#[allow(clippy::too_many_lines)]
fn capture_continue_walks_web_discoveries_breadth_first_with_remote_provenance() {
    let env = TestEnv::new("continue-web-tree");
    env.install_binary("scriptor-page-renderer", FAKE_PAGE_RENDERER_TREE);
    let created: Value = serde_json::from_slice(
        &env.command()
            .args([
                "capture",
                "https://93.184.216.34/tree-root",
                "--policy",
                "safe-web@1",
            ])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job racine JSON");
    let root_job = created["job"]["job_id"].as_str().expect("Job racine");
    let root: Value = serde_json::from_slice(
        &env.command()
            .args(["job", "wait", root_job, "--timeout-secs", "5"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job racine terminé");
    let root_capture = root["capture_id"].as_str().expect("Capture racine");
    let order_path = env.xdg_cache.join("renderer-order");
    let continued: Value = serde_json::from_slice(
        &env.command()
            .args([
                "capture",
                "continue",
                root_capture,
                "--policy",
                "safe-web@1",
            ])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job de continuation JSON");
    let continuation_id = continued["job"]["job_id"]
        .as_str()
        .expect("Job de continuation");
    let completed: Value = serde_json::from_slice(
        &env.command()
            .args(["job", "wait", continuation_id, "--timeout-secs", "5"])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job de continuation terminé");
    assert_eq!(completed["state"], "succeeded", "{completed}");
    assert_eq!(
        fs::read_to_string(&order_path).expect("ordre du renderer"),
        "https://93.184.216.34/tree-a\nhttps://93.184.216.34/tree-b\nhttps://93.184.216.34/tree-a1\nhttps://93.184.216.34/tree-a2\nhttps://93.184.216.34/tree-b1\n"
    );
    let child_a = completed["child_capture_ids"]
        .as_array()
        .expect("Captures enfants")
        .iter()
        .find_map(Value::as_str)
        .expect("Capture enfant a");
    let child: Value = serde_json::from_slice(
        &env.command()
            .args(["capture", "inspect", child_a])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Capture enfant inspectée");
    let provenance = &child["manifest"]["remote_provenance"];
    assert_eq!(provenance["requested_url"], "https://93.184.216.34/tree-a");
    assert_eq!(
        provenance["final_url"],
        "https://93.184.216.34/tree-a-final"
    );
    assert_eq!(provenance["mime"], "text/html");
    assert!(provenance["sha256"].is_string());
    assert!(provenance["size_bytes"].is_u64());
    assert_eq!(
        provenance["redirect_chain"],
        serde_json::json!([
            "https://93.184.216.34/tree-a",
            "https://93.184.216.34/tree-a-final"
        ])
    );
}

#[test]
fn capture_continue_marks_a_web_final_url_cycle_without_publishing_a_child() {
    let env = TestEnv::new("continue-web-cycle");
    env.install_binary(
        "scriptor-page-renderer",
        FAKE_PAGE_RENDERER_RESOLUTION_CASES,
    );
    let created = create_safe_web_capture(&env, "https://93.184.216.34/cycle-root");
    let root = wait_for_agent_job(&env, created["job"]["job_id"].as_str().expect("Job racine"));
    let root_capture = root["capture_id"].as_str().expect("Capture racine");
    let continued = continue_safe_web_capture(&env, root_capture);
    let completed = wait_for_agent_job(
        &env,
        continued["job"]["job_id"]
            .as_str()
            .expect("Job de continuation"),
    );
    assert_eq!(completed["state"], "partial", "{completed}");
    assert_eq!(completed["child_capture_ids"], serde_json::json!([]));
    let parent = inspect_agent_capture(&env, root_capture);
    assert_eq!(parent["ledger"][1]["details"]["status"], "cycle");
    assert_eq!(
        parent["ledger"][1]["details"]["discovery_id"],
        parent["manifest"]["discoveries"][0]["discovery_id"]
    );
}

#[test]
fn capture_continue_rejects_lies_for_audio_and_video_mime_types() {
    let env = TestEnv::new("continue-web-mime-lies");
    env.install_binary("scriptor-page-renderer", FAKE_PAGE_RENDERER_MIME_LIES);
    env.install_binary("scriptor-binary-acquirer", FAKE_BINARY_ACQUIRER_MIME_LIES);
    let created = create_safe_web_capture(&env, "https://93.184.216.34/mime-root");
    let root = wait_for_agent_job(&env, created["job"]["job_id"].as_str().expect("Job racine"));
    let root_capture = root["capture_id"].as_str().expect("Capture racine");
    let continued = continue_safe_web_capture(&env, root_capture);
    let completed = wait_for_agent_job(
        &env,
        continued["job"]["job_id"]
            .as_str()
            .expect("Job de continuation"),
    );
    assert_eq!(completed["state"], "partial", "{completed}");
    assert_eq!(completed["child_capture_ids"], serde_json::json!([]));
    let parent = inspect_agent_capture(&env, root_capture);
    assert_eq!(parent["ledger"][1]["details"]["status"], "unsupported");
    assert_eq!(parent["ledger"][2]["details"]["status"], "unsupported");
    assert_eq!(parent["ledger"][1]["details"]["mime"], "video/mp4");
    assert_eq!(parent["ledger"][2]["details"]["mime"], "audio/mpeg");
}

#[test]
fn capture_continue_stops_after_fifty_web_sources() {
    let env = TestEnv::new("continue-web-source-limit");
    env.install_binary(
        "scriptor-page-renderer",
        FAKE_PAGE_RENDERER_RESOLUTION_CASES,
    );
    let created = create_safe_web_capture(&env, "https://93.184.216.34/many-root");
    let root = wait_for_agent_job(&env, created["job"]["job_id"].as_str().expect("Job racine"));
    let root_capture = root["capture_id"].as_str().expect("Capture racine");
    let continued = continue_safe_web_capture(&env, root_capture);
    let completed = wait_for_agent_job(
        &env,
        continued["job"]["job_id"]
            .as_str()
            .expect("Job de continuation"),
    );
    assert_eq!(completed["state"], "partial", "{completed}");
    assert_eq!(
        completed["child_capture_ids"].as_array().map(Vec::len),
        Some(50)
    );
    let order = fs::read_to_string(env.xdg_cache.join("renderer-order"))
        .expect("Sources acquises par le renderer");
    assert_eq!(order.lines().count(), 50);
    assert!(!order.contains("many-50\n"));
    let parent = inspect_agent_capture(&env, root_capture);
    assert_eq!(parent["ledger"].as_array().map(Vec::len), Some(52));
    assert_eq!(parent["ledger"][51]["details"]["status"], "skipped_budget");
}

#[test]
fn capture_continue_fails_cleanly_when_a_child_acquisition_crashes() {
    let env = TestEnv::new("continue-web-crash");
    env.install_binary(
        "scriptor-page-renderer",
        FAKE_PAGE_RENDERER_RESOLUTION_CASES,
    );
    let created = create_safe_web_capture(&env, "https://93.184.216.34/crash-root");
    let root = wait_for_agent_job(&env, created["job"]["job_id"].as_str().expect("Job racine"));
    let root_capture = root["capture_id"].as_str().expect("Capture racine");
    let continued = continue_safe_web_capture(&env, root_capture);
    let completed = wait_for_agent_job(
        &env,
        continued["job"]["job_id"]
            .as_str()
            .expect("Job de continuation"),
    );
    assert_eq!(completed["state"], "failed", "{completed}");
    assert!(completed["worker_pid"].is_null());
    let captures = env.xdg_data.join("scriptor/v2/captures");
    assert_eq!(
        fs::read_dir(captures)
            .expect("Référentiel de Captures")
            .filter_map(Result::ok)
            .filter(|entry| entry.path().join("manifest.json").is_file())
            .count(),
        1
    );
}

#[test]
fn capture_continue_cancellation_stops_a_running_web_child() {
    let env = TestEnv::new("continue-web-cancellation");
    env.install_binary(
        "scriptor-page-renderer",
        FAKE_PAGE_RENDERER_RESOLUTION_CASES,
    );
    let created = create_safe_web_capture(&env, "https://93.184.216.34/slow-root");
    let root = wait_for_agent_job(&env, created["job"]["job_id"].as_str().expect("Job racine"));
    let root_capture = root["capture_id"].as_str().expect("Capture racine");
    let continued = continue_safe_web_capture(&env, root_capture);
    let continuation_id = continued["job"]["job_id"]
        .as_str()
        .expect("Job de continuation");
    assert!(
        wait_for_file(
            &env.xdg_cache.join("renderer-started"),
            Duration::from_secs(2)
        ),
        "le renderer enfant ne démarre pas"
    );
    env.command()
        .args(["job", "cancel", continuation_id])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"state\":\"cancelled\""));
    let cancelled = wait_for_agent_job(&env, continuation_id);
    assert_eq!(cancelled["state"], "cancelled", "{cancelled}");
    assert_eq!(cancelled["child_capture_ids"], serde_json::json!([]));
}

#[test]
fn safe_web_preserves_a_renderer_refusal_code_in_the_job_contract() {
    let env = TestEnv::new("safe-web-refusal");
    env.install_binary("scriptor-page-renderer", FAKE_PAGE_RENDERER_PRIVATE_TARGET);
    let created: Value = serde_json::from_slice(
        &env.command()
            .args([
                "capture",
                "https://93.184.216.34/",
                "--policy",
                "safe-web@1",
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
    env.command()
        .args(["job", "wait", job_id, "--timeout-secs", "5"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"code\":\"web_private_target_refused\"",
        ));
}

#[test]
fn capture_bounds_provider_diagnostics_in_memory() {
    let env = TestEnv::new("capture-provider-diagnostics");
    env.install_binary(
        "pdfinfo",
        "#!/bin/sh\ni=0\nwhile [ \"$i\" -lt 16385 ]; do\n  printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'\n  i=$((i + 1))\ndone\n",
    );
    env.install_binary("pdftotext", FAKE_PDFTOTEXT);
    let source = env.work_dir.join("verbose.pdf");
    fs::write(&source, b"original pdf bytes").expect("écriture du PDF");
    let path = write_capture_worker_job(&env, "job-63", &source, &["pdfinfo", "pdftotext"], 30);

    env.command()
        .args(["capture-worker", "--job-id", "job-63"])
        .assert()
        .success();
    let job: Value =
        serde_json::from_slice(&fs::read(path).expect("lecture du Job")).expect("Job JSON valide");
    assert_eq!(job["state"], "partial");
    assert!(job["capture_id"].is_string());
}

#[test]
fn capture_admission_enforces_concurrency_before_launching_a_provider() {
    let env = TestEnv::new("capture-concurrency");
    env.write_config(&env.work_dir.join("out"));
    let started = env.work_dir.join("provider-started");
    let release = env.work_dir.join("provider-release");
    env.install_binary(
        "ffmpeg",
        &format!(
            "#!/bin/sh\nset -eu\nprintf x > \"{}\"\nwhile [ ! -f \"{}\" ]; do :; done\n",
            started.display(),
            release.display()
        ),
    );
    let first_source = env.write_media_file("first.mp4");
    let second_source = env.write_media_file("second.mp4");
    let first_path = write_capture_worker_job(&env, "job-61", &first_source, &["ffmpeg"], 30);
    let second_path = write_capture_worker_job(&env, "job-62", &second_source, &["ffmpeg"], 30);
    for path in [&first_path, &second_path] {
        let mut job: Value = serde_json::from_slice(&fs::read(path).expect("lecture du Job"))
            .expect("Job JSON valide");
        job["policy"]["snapshot"]["limits"]["max_concurrency"] = Value::from(1);
        fs::write(
            path,
            serde_json::to_vec(&job).expect("sérialisation du Job"),
        )
        .expect("écriture du Job");
    }

    let mut first_worker = std::process::Command::new(env!("CARGO_BIN_EXE_scriptor"))
        .env("PATH", &env.bin_dir)
        .env("XDG_CONFIG_HOME", &env.xdg_config)
        .env("XDG_CACHE_HOME", &env.xdg_cache)
        .env("XDG_DATA_HOME", &env.xdg_data)
        .args(["capture-worker", "--job-id", "job-61"])
        .spawn()
        .expect("lancement du premier Worker");
    assert!(wait_for_job_state(
        &first_path,
        "running",
        Duration::from_secs(1)
    ));
    assert!(wait_for_file(&started, Duration::from_secs(1)));
    env.command()
        .args(["capture-worker", "--job-id", "job-62"])
        .assert()
        .success();
    fs::write(&release, b"release").expect("libération du faux Provider");
    first_worker.wait().expect("attente du premier Worker");
    let second: Value = serde_json::from_slice(&fs::read(&second_path).expect("lecture du Job"))
        .expect("Job JSON valide");
    assert_eq!(second["state"], "failed");
    assert_eq!(second["error"]["code"], "concurrency_limit_exceeded");
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
fn cancelling_a_running_local_capture_stops_the_provider_without_publishing() {
    let env = TestEnv::new("capture-cancellation");
    env.write_config(&env.work_dir.join("out"));
    let source = env.write_media_file("interview.mp4");
    let provider_started = env.work_dir.join("provider-started");
    write_executable(
        &env.bin_dir,
        "ffmpeg",
        &format!(
            "#!/bin/sh\n: > \"{}\"\nwhile :; do :; done\n",
            provider_started.display()
        ),
    );
    let job_id = "job-64";
    let job_path = write_capture_worker_job(&env, job_id, &source, &["ffmpeg"], 30);
    let mut worker = std::process::Command::new(env!("CARGO_BIN_EXE_scriptor"))
        .env("PATH", &env.bin_dir)
        .env("XDG_CONFIG_HOME", &env.xdg_config)
        .env("XDG_CACHE_HOME", &env.xdg_cache)
        .env("XDG_DATA_HOME", &env.xdg_data)
        .args(["capture-worker", "--job-id", job_id])
        .spawn()
        .expect("lancement du Worker");
    let deadline = Instant::now() + Duration::from_secs(1);
    while !provider_started.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        provider_started.exists(),
        "le Provider a démarré: {}",
        fs::read_to_string(&job_path).expect("lecture du Job en cours")
    );

    env.command()
        .args(["job", "cancel", job_id])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"state\":\"cancelled\""));
    worker.wait().expect("arrêt du Worker annulé");

    let job: Value = serde_json::from_slice(&fs::read(job_path).expect("lecture du Job"))
        .expect("Job JSON valide");
    assert_eq!(job["state"], "cancelled");
    assert!(job["capture_id"].is_null());
    let captures = env.xdg_data.join("scriptor").join("v2").join("captures");
    assert!(
        fs::read_dir(captures)
            .expect("lecture des Captures")
            .next()
            .is_none(),
        "aucune Capture, même staging, ne doit survivre à l annulation"
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

#[test]
fn derive_whole_capture_publishes_a_readable_traced_derivative() {
    let env = TestEnv::new("derive-whole-capture");
    env.install_binary("scriptor-local-derive", FAKE_LOCAL_DERIVE_PROVIDER);
    let capture_id = create_text_capture(&env);

    let created: Value = serde_json::from_slice(
        &env.command()
            .args([
                "derive",
                &capture_id,
                "--recipe",
                "markdown-note",
                "--provider",
                "scriptor-local-derive",
                "--policy",
                "safe-local@1",
                "--whole-capture",
                "--parameters",
                r#"{"style":"requested"}"#,
            ])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job de Derive JSON valide");
    assert_eq!(created["job"]["operation"]["kind"], "derive");
    assert_eq!(
        created["job"]["operation"]["recipe"]["kind"],
        "markdown-note"
    );
    let job_id = created["job"]["job_id"]
        .as_str()
        .expect("identifiant du Job de Derive");
    let finished = wait_for_agent_job(&env, job_id);
    assert_eq!(finished["state"], "succeeded", "{finished}");
    let derive_id = finished["derive_id"]
        .as_str()
        .expect("identifiant de Derive");

    let capture = inspect_agent_capture(&env, &capture_id);
    assert_eq!(capture["derivatives"].as_array().map(Vec::len), Some(1));
    let derivative = &capture["derivatives"][0];
    assert_eq!(derivative["derive_id"], derive_id);
    assert_eq!(derivative["recipe"]["kind"], "markdown-note");
    assert_eq!(derivative["recipe"]["target"]["kind"], "capture");
    assert_eq!(derivative["provider"]["name"], "scriptor-local-derive");
    assert!(
        derivative["provider"]["version"]
            .as_str()
            .is_some_and(|version| version.starts_with("sha256:"))
    );
    assert_eq!(
        derivative["provider"]["parameters"],
        serde_json::json!({"style": "concise"})
    );
    assert_eq!(derivative["inputs"].as_array().map(Vec::len), Some(1));
    assert_eq!(derivative["inputs"][0]["artifact_id"], "proof-source");
    assert_eq!(derivative["capability"]["state"], "succeeded");

    let reference = derivative["reference"].to_string();
    let read: Value = serde_json::from_slice(
        &env.command()
            .args(["capture", "read", "--reference", &reference])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("lecture du Derive JSON valide");
    assert_eq!(
        read["content"]["text"],
        "# Note derivee\n\nContenu local.\n"
    );
    assert_eq!(
        read["artifact"]["provider"]["name"],
        "scriptor-local-derive"
    );
    assert_eq!(capture["ledger"][1]["event"], "derivative_published");
    assert_eq!(capture["ledger"][1]["job_id"], job_id);
}

#[test]
fn derive_transmits_only_selected_verified_references() {
    let env = TestEnv::new("derive-selected-reference");
    env.install_binary("scriptor-local-derive", FAKE_LOCAL_DERIVE_PROVIDER);
    env.install_binary("pdfinfo", FAKE_PDFINFO);
    env.install_binary("pdftotext", FAKE_PDFTOTEXT);
    let source = env.work_dir.join("document.pdf");
    fs::write(&source, b"%PDF-1.4").expect("ecriture de la Source PDF");
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
    .expect("Job de Capture JSON valide");
    let capture_job_id = created["job"]["job_id"].as_str().expect("Job de Capture");
    let capture_job = wait_for_agent_job(&env, capture_job_id);
    assert_eq!(capture_job["state"], "succeeded", "{capture_job}");
    let capture_id = capture_job["capture_id"].as_str().expect("Capture");
    let capture = inspect_agent_capture(&env, capture_id);
    let extraction = &capture["manifest"]["extractions"][0];
    let reference = serde_json::json!({
        "capture_id": capture_id,
        "artifact_id": extraction["artifact_id"],
        "sha256": extraction["sha256"],
        "locator": extraction["locator"],
    });

    let derived: Value = serde_json::from_slice(
        &env.command()
            .args([
                "derive",
                capture_id,
                "--recipe",
                "structured-summary",
                "--provider",
                "scriptor-local-derive",
                "--policy",
                "safe-local@1",
                "--reference",
                &reference.to_string(),
            ])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job de Derive JSON valide");
    let derive_job_id = derived["job"]["job_id"].as_str().expect("Job de Derive");
    assert_eq!(
        wait_for_agent_job(&env, derive_job_id)["state"],
        "succeeded"
    );
    let request: Value = serde_json::from_slice(
        &fs::read(env.xdg_cache.join("derive-request.json"))
            .expect("requete du Provider conservee"),
    )
    .expect("requete du Provider JSON valide");
    assert_eq!(request["inputs"].as_array().map(Vec::len), Some(1));
    assert_eq!(request["inputs"][0]["reference"], reference);
    assert_eq!(request["recipe"]["target"]["kind"], "references");

    fs::remove_file(env.xdg_cache.join("derive-request.json"))
        .expect("suppression du marqueur Provider");
    let mut forged = reference;
    forged["sha256"] = Value::String("00".repeat(32));
    let refused: Value = serde_json::from_slice(
        &env.command()
            .args([
                "derive",
                capture_id,
                "--recipe",
                "structured-summary",
                "--provider",
                "scriptor-local-derive",
                "--policy",
                "safe-local@1",
                "--reference",
                &forged.to_string(),
            ])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("refus JSON valide");
    assert_eq!(refused["error"]["code"], "reference_mismatch");
    assert!(!env.xdg_cache.join("derive-request.json").exists());
}

#[test]
fn derive_refuses_missing_policy_and_unauthorized_recipe_or_provider() {
    let env = TestEnv::new("derive-policy-refusals");
    env.install_binary("scriptor-local-derive", FAKE_LOCAL_DERIVE_PROVIDER);
    let capture_id = create_text_capture(&env);
    let cases = [
        (
            vec![
                "derive",
                &capture_id,
                "--recipe",
                "markdown-note",
                "--provider",
                "scriptor-local-derive",
                "--whole-capture",
            ],
            "policy_required",
        ),
        (
            vec![
                "derive",
                &capture_id,
                "--recipe",
                "markdown-note",
                "--provider",
                "scriptor-local-derive",
                "--policy",
                "safe-web@1",
                "--whole-capture",
            ],
            "recipe_not_allowed",
        ),
        (
            vec![
                "derive",
                &capture_id,
                "--recipe",
                "markdown-note",
                "--provider",
                "other-provider",
                "--policy",
                "safe-local@1",
                "--whole-capture",
            ],
            "provider_not_allowed",
        ),
    ];
    for (arguments, expected_code) in cases {
        let refused: Value = serde_json::from_slice(
            &env.command()
                .args(arguments)
                .assert()
                .success()
                .get_output()
                .stdout,
        )
        .expect("refus de Policy JSON valide");
        assert_eq!(refused["error"]["code"], expected_code);
        assert!(!env.xdg_cache.join("derive-request.json").exists());
    }
}

#[test]
fn rerunning_a_recipe_creates_distinct_immutable_derivatives() {
    let env = TestEnv::new("derive-rerun");
    env.install_binary("scriptor-local-derive", FAKE_LOCAL_DERIVE_PROVIDER);
    let capture_id = create_text_capture(&env);
    let run = |retry_of: Option<&str>| {
        let mut arguments = vec![
            "derive".to_string(),
            capture_id.clone(),
            "--recipe".to_string(),
            "markdown-note".to_string(),
            "--provider".to_string(),
            "scriptor-local-derive".to_string(),
            "--policy".to_string(),
            "safe-local@1".to_string(),
            "--whole-capture".to_string(),
        ];
        if let Some(job_id) = retry_of {
            arguments.extend(["--retry-of".to_string(), job_id.to_string()]);
        }
        let created: Value = serde_json::from_slice(
            &env.command()
                .args(arguments)
                .assert()
                .success()
                .get_output()
                .stdout,
        )
        .expect("Job de Derive JSON valide");
        let job_id = created["job"]["job_id"].as_str().expect("Job de Derive");
        let finished = wait_for_agent_job(&env, job_id);
        assert_eq!(finished["state"], "succeeded", "{finished}");
        (
            job_id.to_string(),
            finished["derive_id"]
                .as_str()
                .expect("identifiant de Derive")
                .to_string(),
        )
    };

    let (first_job, first_derive) = run(None);
    let first_path = env
        .xdg_data
        .join("scriptor/v2/captures")
        .join(&capture_id)
        .join("derivatives")
        .join(&first_derive)
        .join("content");
    let first_bytes = fs::read(&first_path).expect("premier Derive lisible");
    let mismatched_retry: Value = serde_json::from_slice(
        &env.command()
            .args([
                "derive",
                &capture_id,
                "--recipe",
                "markdown-note",
                "--provider",
                "scriptor-local-derive",
                "--policy",
                "safe-local@1",
                "--whole-capture",
                "--parameters",
                r#"{"style":"different"}"#,
                "--retry-of",
                &first_job,
            ])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("refus de relance JSON valide");
    assert_eq!(mismatched_retry["error"]["code"], "invalid_retry");
    let (second_job, second_derive) = run(Some(&first_job));

    assert_ne!(first_job, second_job);
    assert_ne!(first_derive, second_derive);
    assert_eq!(
        fs::read(&first_path).expect("premier Derive toujours lisible"),
        first_bytes
    );
    let capture = inspect_agent_capture(&env, &capture_id);
    assert_eq!(capture["derivatives"].as_array().map(Vec::len), Some(2));
    assert_eq!(capture["ledger"].as_array().map(Vec::len), Some(3));
    assert_eq!(capture["ledger"][1]["details"]["derive_id"], first_derive);
    assert_eq!(capture["ledger"][2]["details"]["derive_id"], second_derive);
    let second_job_state: Value = serde_json::from_slice(
        &env.command()
            .args(["job", "get", &second_job])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job de relance JSON valide");
    assert_eq!(second_job_state["retry_of"], first_job);
}

#[test]
fn failed_derive_capability_is_structured_and_retry_is_a_distinct_attempt() {
    let env = TestEnv::new("derive-failure-retry");
    env.install_binary("scriptor-local-derive", FAKE_LOCAL_DERIVE_PROVIDER_FAILURE);
    let capture_id = create_text_capture(&env);
    let create = |retry_of: Option<&str>| {
        let mut arguments = vec![
            "derive".to_string(),
            capture_id.clone(),
            "--recipe".to_string(),
            "sourced-answer".to_string(),
            "--provider".to_string(),
            "scriptor-local-derive".to_string(),
            "--policy".to_string(),
            "safe-local@1".to_string(),
            "--whole-capture".to_string(),
        ];
        if let Some(job_id) = retry_of {
            arguments.extend(["--retry-of".to_string(), job_id.to_string()]);
        }
        serde_json::from_slice::<Value>(
            &env.command()
                .args(arguments)
                .assert()
                .success()
                .get_output()
                .stdout,
        )
        .expect("Job de Derive JSON valide")
    };

    let failed = create(None);
    let failed_job_id = failed["job"]["job_id"]
        .as_str()
        .expect("premiere tentative");
    let failed_job = wait_for_agent_job(&env, failed_job_id);
    assert_eq!(failed_job["state"], "failed", "{failed_job}");
    assert_eq!(failed_job["error"]["code"], "derive_provider_failed");
    assert_eq!(failed_job["error"]["capability"], "sourced-answer");
    assert!(
        failed_job["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("modele local indisponible"))
    );
    assert!(failed_job["derive_id"].is_null());
    assert!(
        inspect_agent_capture(&env, &capture_id)["derivatives"]
            .as_array()
            .is_some_and(Vec::is_empty)
    );

    env.install_binary("scriptor-local-derive", FAKE_LOCAL_DERIVE_PROVIDER);
    let retried = create(Some(failed_job_id));
    assert_eq!(retried["job"]["retry_of"], failed_job_id);
    let retry_job_id = retried["job"]["job_id"].as_str().expect("relance");
    assert_ne!(failed_job_id, retry_job_id);
    let retry_job = wait_for_agent_job(&env, retry_job_id);
    assert_eq!(retry_job["state"], "succeeded", "{retry_job}");
    assert!(retry_job["derive_id"].is_string());
}

#[test]
fn derive_never_persists_parameters_marked_as_sensitive() {
    let env = TestEnv::new("derive-sensitive-parameters");
    env.install_binary("scriptor-local-derive", FAKE_LOCAL_DERIVE_PROVIDER);
    let capture_id = create_text_capture(&env);
    let refused: Value = serde_json::from_slice(
        &env.command()
            .args([
                "derive",
                &capture_id,
                "--recipe",
                "markdown-note",
                "--provider",
                "scriptor-local-derive",
                "--policy",
                "safe-local@1",
                "--whole-capture",
                "--parameters",
                r#"{"header":"Bearer secret"}"#,
            ])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("refus des parametres JSON valide");
    assert_eq!(refused["error"]["code"], "sensitive_parameters");
    assert!(!env.xdg_cache.join("derive-request.json").exists());

    env.install_binary("scriptor-local-derive", FAKE_LOCAL_DERIVE_PROVIDER_SECRET);
    let created: Value = serde_json::from_slice(
        &env.command()
            .args([
                "derive",
                &capture_id,
                "--recipe",
                "markdown-note",
                "--provider",
                "scriptor-local-derive",
                "--policy",
                "safe-local@1",
                "--whole-capture",
            ])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job de Derive JSON valide");
    let job_id = created["job"]["job_id"].as_str().expect("Job de Derive");
    let failed = wait_for_agent_job(&env, job_id);
    assert_eq!(failed["state"], "failed", "{failed}");
    assert_eq!(failed["error"]["code"], "sensitive_parameters");
    assert_eq!(failed["error"]["capability"], "markdown-note");
    assert!(
        inspect_agent_capture(&env, &capture_id)["derivatives"]
            .as_array()
            .is_some_and(Vec::is_empty)
    );
}

#[test]
fn cancelling_a_running_derive_never_publishes_it() {
    let env = TestEnv::new("derive-cancellation");
    env.install_binary("scriptor-local-derive", FAKE_LOCAL_DERIVE_PROVIDER_BLOCKING);
    let capture_id = create_text_capture(&env);
    let created: Value = serde_json::from_slice(
        &env.command()
            .args([
                "derive",
                &capture_id,
                "--recipe",
                "markdown-note",
                "--provider",
                "scriptor-local-derive",
                "--policy",
                "safe-local@1",
                "--whole-capture",
            ])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job de Derive JSON valide");
    let job_id = created["job"]["job_id"].as_str().expect("Job de Derive");
    assert!(wait_for_file(
        &env.xdg_cache.join("derive-started"),
        Duration::from_secs(5)
    ));
    let cancelled: Value = serde_json::from_slice(
        &env.command()
            .args(["job", "cancel", job_id])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("annulation JSON valide");
    assert_eq!(cancelled["state"], "cancelled");
    assert!(
        inspect_agent_capture(&env, &capture_id)["derivatives"]
            .as_array()
            .is_some_and(Vec::is_empty)
    );
}

#[test]
fn derive_concurrency_failure_keeps_the_recipe_capability() {
    let env = TestEnv::new("derive-concurrency");
    env.install_binary("scriptor-local-derive", FAKE_LOCAL_DERIVE_PROVIDER);
    let capture_id = create_text_capture(&env);
    let blocker_source = env.work_dir.join("blocker.txt");
    fs::write(&blocker_source, "blocker").expect("ecriture de la Source de blocage");
    for job_id in ["job-9001", "job-9002"] {
        let path = write_capture_worker_job(&env, job_id, &blocker_source, &[], 30);
        let mut job: Value = serde_json::from_slice(&fs::read(&path).expect("lecture du Job"))
            .expect("Job JSON valide");
        job["state"] = Value::String("running".to_string());
        fs::write(
            path,
            serde_json::to_vec(&job).expect("serialisation du Job"),
        )
        .expect("ecriture du Job concurrent");
    }
    let created: Value = serde_json::from_slice(
        &env.command()
            .args([
                "derive",
                &capture_id,
                "--recipe",
                "checklist",
                "--provider",
                "scriptor-local-derive",
                "--policy",
                "safe-local@1",
                "--whole-capture",
            ])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("Job de Derive JSON valide");
    let job_id = created["job"]["job_id"].as_str().expect("Job de Derive");
    let failed = wait_for_agent_job(&env, job_id);
    assert_eq!(failed["state"], "failed", "{failed}");
    assert_eq!(failed["error"]["code"], "concurrency_limit_exceeded");
    assert_eq!(failed["error"]["capability"], "checklist");
}
