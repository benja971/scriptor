//! Extraction des Frames illustrant une Source vidéo, via `ffmpeg`/`ffprobe` :
//! une passe à intervalle fixe, une passe par détection de changement de
//! scène, fusionnées avec anti-doublon (une Frame de changement de scène trop
//! proche d'une Frame à intervalle fixe est ignorée), puis filtrées pour
//! retirer les Frames de couleur quasi unie (écrans de fondu). Ne produit
//! aucune Frame si la Source ne contient pas de flux vidéo (Source audio).

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::binary::ensure_present_in;

/// Paramètres pilotant l'extraction de Frames, résolus depuis la configuration.
#[derive(Debug, Clone, Copy)]
pub struct FrameExtractionParams {
    /// Intervalle (en secondes) entre deux Frames extraites à fréquence fixe.
    pub interval_secs: u32,
    /// Seuil de détection ffmpeg (`0.0`-`1.0`) déclenchant l'extraction d'une
    /// Frame supplémentaire à un changement de scène.
    pub scene_threshold: f64,
    /// Fenêtre (en secondes) en-deçà de laquelle une Frame de changement de
    /// scène trop proche d'une Frame à intervalle fixe est ignorée.
    pub dedup_window_secs: u32,
}

/// Une Frame extraite par une passe ffmpeg, avec son timestamp réel dans la
/// Source (lu depuis la sortie du filtre `showinfo`, pas déduit du nom de
/// fichier).
struct ExtractedFrame {
    path: PathBuf,
    timestamp_secs: f64,
}

/// Extrait les Frames de `input` vers `frames_dir` (créé seulement si au
/// moins une Frame est produite), combinant intervalle fixe et changement de
/// scène. Utilise `tmp_dir` pour les passes intermédiaires. Retourne le
/// nombre de Frames écrites dans `frames_dir` ; `0` si `input` ne contient
/// aucun flux vidéo.
///
/// # Errors
///
/// Retourne une erreur si `ffmpeg` ou `ffprobe` est absent du `PATH`, si l'un
/// des deux process ne peut pas être lancé, si l'un d'eux termine avec un
/// code de sortie non nul, ou si le nombre de Frames extraites par une passe
/// ne correspond pas au nombre de timestamps lus dans sa sortie.
pub fn extract_frames(
    input: &Path,
    tmp_dir: &Path,
    frames_dir: &Path,
    params: FrameExtractionParams,
) -> Result<usize> {
    let path_env = env::var_os("PATH").unwrap_or_default();
    extract_frames_with_path(input, tmp_dir, frames_dir, params, &path_env)
}

fn extract_frames_with_path(
    input: &Path,
    tmp_dir: &Path,
    frames_dir: &Path,
    params: FrameExtractionParams,
    path_env: &OsStr,
) -> Result<usize> {
    ensure_present_in("ffmpeg", path_env)?;
    ensure_present_in("ffprobe", path_env)?;

    if !has_video_stream(input, path_env)? {
        return Ok(0);
    }

    let interval_filter = format!("fps=1/{},showinfo", params.interval_secs);
    let interval_frames = run_extraction_pass(
        input,
        &tmp_dir.join("frames-interval"),
        &interval_filter,
        path_env,
    )
    .context("failed to extract fixed-interval Frames")?;

    let scene_filter = format!("select='gt(scene,{})',showinfo", params.scene_threshold);
    let scene_frames = run_extraction_pass(
        input,
        &tmp_dir.join("frames-scene"),
        &scene_filter,
        path_env,
    )
    .context("failed to extract scene-change Frames")?;

    merge_and_write(
        &interval_frames,
        &scene_frames,
        frames_dir,
        params.dedup_window_secs,
        path_env,
    )
}

/// Indique si `input` contient au moins un flux vidéo, via `ffprobe`.
fn has_video_stream(input: &Path, path_env: &OsStr) -> Result<bool> {
    let output = Command::new("ffprobe")
        .env("PATH", path_env)
        .args(["-v", "error", "-select_streams", "v"])
        .args(["-show_entries", "stream=index", "-of", "csv=p=0"])
        .arg(input)
        .output()
        .context("failed to launch `ffprobe`")?;

    if !output.status.success() {
        bail!(
            "`ffprobe` failed (exit code {:?})\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
}

/// Lance une passe d'extraction ffmpeg avec le filtre `filter` (qui doit
/// inclure `showinfo`, utilisé pour retrouver le timestamp réel de chaque
/// Frame écrite), vers un dossier dédié créé à la volée dans `dest_dir`.
fn run_extraction_pass(
    input: &Path,
    dest_dir: &Path,
    filter: &str,
    path_env: &OsStr,
) -> Result<Vec<ExtractedFrame>> {
    fs::create_dir_all(dest_dir)
        .with_context(|| format!("creating temporary directory {}", dest_dir.display()))?;

    let pattern = dest_dir.join("frame-%06d.jpg");
    let output = Command::new("ffmpeg")
        .env("PATH", path_env)
        .arg("-y")
        .arg("-i")
        .arg(input)
        .args(["-map", "0:v:0"])
        .args(["-vf", filter])
        .args(["-fps_mode", "vfr"])
        // Sans format de pixel explicite, l'encodeur mjpeg déduit ses
        // paramètres de la première frame qui traverse le filtre : sur une
        // Source en colorimétrie limited/`tv` range (courant en VP9/H.264
        // web) et un filtre `select` qui peut ne produire aucune frame avant
        // la négociation, l'encodeur refuse ce format et l'extraction entière
        // échoue (`Error while opening encoder`). Fixer `yuvj420p` (JPEG
        // full-range) rend la négociation indépendante du contenu de la
        // Source.
        .args(["-pix_fmt", "yuvj420p"])
        .arg(&pattern)
        .output()
        .context("failed to launch `ffmpeg`")?;

    if !output.status.success() {
        bail!(
            "`ffmpeg` failed (exit code {:?})\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    let timestamps = parse_showinfo_timestamps(&String::from_utf8_lossy(&output.stderr));

    let mut paths = fs::read_dir(dest_dir)
        .with_context(|| format!("reading temporary directory {}", dest_dir.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<PathBuf>>>()
        .with_context(|| format!("reading temporary directory {}", dest_dir.display()))?;
    paths.sort();

    if paths.len() != timestamps.len() {
        bail!(
            "ffmpeg produced {} Frame(s) but {} timestamp(s) were read from its output",
            paths.len(),
            timestamps.len(),
        );
    }

    Ok(paths
        .into_iter()
        .zip(timestamps)
        .map(|(path, timestamp_secs)| ExtractedFrame {
            path,
            timestamp_secs,
        })
        .collect())
}

/// Extrait, dans l'ordre, le `pts_time` de chaque ligne de log émise par le
/// filtre ffmpeg `showinfo` (une ligne par Frame traversant le filtre).
fn parse_showinfo_timestamps(stderr: &str) -> Vec<f64> {
    stderr
        .lines()
        .filter_map(|line| line.split("pts_time:").nth(1))
        .filter_map(|rest| rest.split_whitespace().next())
        .filter_map(|token| token.parse::<f64>().ok())
        .collect()
}

/// Fusionne les deux passes : toutes les Frames à intervalle fixe sont
/// gardées ; une Frame de changement de scène n'est gardée que si elle est à
/// plus de `dedup_window_secs` secondes de toute Frame à intervalle fixe déjà
/// gardée. Les survivantes sont ensuite filtrées pour retirer celles de
/// couleur quasi unie (cf. [`is_near_uniform_color`]), puis écrites dans
/// `frames_dir` (créé au passage, seulement s'il en reste au moins une),
/// triées par timestamp croissant, nommées `frame-<N>-<timestamp>s.jpg`.
///
/// Ne filtre PAS les Frames adjacentes quasi identiques (deux Frames
/// consécutives montrant un contenu très proche visuellement) : trois
/// métriques de similarité entre Frames adjacentes (SSIM `ffmpeg -lavfi
/// ssim`, différence de pixels brute `blend=all_mode=difference`, score
/// `scene` déjà utilisé pour la détection de changement de scène) ont été
/// testées empiriquement sur un vrai jeu de 12 Frames contenant deux paires
/// connues comme quasi identiques. Dans les trois cas, le score de la paire
/// non-redondante la plus proche chevauche celui des paires redondantes (ex.
/// score `scene` : 0.0385 et 0.0309 pour les deux paires redondantes, contre
/// 0.0343 pour une paire non-redondante intercalée entre les deux) : aucun
/// seuil ne peut séparer les deux catégories sans faux positif ni faux
/// négatif. La redondance perçue ici tient à un contenu sémantique (une
/// étiquette ou une valeur qui change dans une petite zone de l'image), pas à
/// une différence de pixels globale : hors de portée d'une comparaison
/// pixel/structurelle via `ffmpeg` en sous-process, sans decoder d'image dédié.
fn merge_and_write(
    interval: &[ExtractedFrame],
    scene: &[ExtractedFrame],
    frames_dir: &Path,
    dedup_window_secs: u32,
    path_env: &OsStr,
) -> Result<usize> {
    let dedup_window = f64::from(dedup_window_secs);

    let mut kept: Vec<&ExtractedFrame> = interval.iter().collect();
    for candidate in scene {
        let too_close = interval.iter().any(|kept_frame| {
            (kept_frame.timestamp_secs - candidate.timestamp_secs).abs() < dedup_window
        });
        if !too_close {
            kept.push(candidate);
        }
    }
    kept.sort_by(|left, right| left.timestamp_secs.total_cmp(&right.timestamp_secs));

    let mut retained = Vec::with_capacity(kept.len());
    for frame in kept {
        if !is_near_uniform_color(&frame.path, path_env)? {
            retained.push(frame);
        }
    }

    if retained.is_empty() {
        return Ok(0);
    }

    fs::create_dir_all(frames_dir)
        .with_context(|| format!("creating Frames directory {}", frames_dir.display()))?;

    for (index, frame) in retained.iter().enumerate() {
        let sequence = index.checked_add(1).context("too many Frames to number")?;
        let filename = format!("frame-{sequence:04}-{:.3}s.jpg", frame.timestamp_secs);
        fs::copy(&frame.path, frames_dir.join(filename))
            .with_context(|| format!("writing Frame to {}", frames_dir.display()))?;
    }

    Ok(retained.len())
}

/// Écart de luminance (`YMAX - YMIN`, sur une échelle 0-255) en-deçà duquel
/// une Frame est considérée de couleur quasi unie (écran de fondu, transition)
/// et rejetée. Calibré empiriquement sur une vraie Frame de fondu noir/vert
/// en fin de vidéo (écart observé : 46) face aux onze autres Frames de la
/// même vidéo, toutes à l'écart maximal (255) : grande marge de part et
/// d'autre, un seuil précis n'est pas critique ici.
const UNIFORM_LUMA_RANGE_MAX: i32 = 120;

/// Indique si la Frame à `path` est de couleur quasi unie, via le filtre
/// `signalstats` de `ffmpeg` (écart de luminance de l'image entière contre
/// [`UNIFORM_LUMA_RANGE_MAX`]).
fn is_near_uniform_color(path: &Path, path_env: &OsStr) -> Result<bool> {
    let output = Command::new("ffmpeg")
        .env("PATH", path_env)
        .arg("-i")
        .arg(path)
        .args(["-vf", "signalstats,metadata=print"])
        .args(["-f", "null"])
        .arg("-")
        .output()
        .context("failed to launch `ffmpeg`")?;

    if !output.status.success() {
        bail!(
            "`ffmpeg` failed (exit code {:?})\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let (Some(ymin), Some(ymax)) = (
        parse_signalstat(&stderr, "YMIN"),
        parse_signalstat(&stderr, "YMAX"),
    ) else {
        bail!(
            "could not read the luma range from ffmpeg's signalstats output for {}",
            path.display()
        );
    };

    Ok(ymax.saturating_sub(ymin) < UNIFORM_LUMA_RANGE_MAX)
}

/// Extrait la valeur entière d'une clé `lavfi.signalstats.<key>=<valeur>`
/// depuis la sortie du filtre ffmpeg `metadata=print` (une occurrence
/// attendue, une seule Frame étant analysée par appel).
fn parse_signalstat(stderr: &str, key: &str) -> Option<i32> {
    let needle = format!("lavfi.signalstats.{key}=");
    stderr
        .lines()
        .find_map(|line| line.split(&needle).nth(1))
        .and_then(|value| value.trim().parse::<i32>().ok())
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::panic_in_result_fn
    )]

    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{
        FrameExtractionParams, extract_frames_with_path, is_near_uniform_color,
        parse_showinfo_timestamps, parse_signalstat,
    };

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir(label: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "scriptor-test-frames-{label}-{}-{n}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("creating test temporary directory");
        dir
    }

    fn write_script(bin_dir: &Path, name: &str, script: &str) {
        let path = bin_dir.join(name);
        fs::write(&path, script).expect("writing fake binary");
        let mut perms = fs::metadata(&path)
            .expect("reading fake binary metadata")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod on fake binary");
    }

    fn params() -> FrameExtractionParams {
        FrameExtractionParams {
            interval_secs: 10,
            scene_threshold: 0.4,
            dedup_window_secs: 3,
        }
    }

    // Fausse `ffprobe` annonçant la présence (ou l'absence) d'un flux vidéo
    // selon `$STUB_HAS_VIDEO` (positionné par le test, transmis par le fake
    // `ffmpeg`/`ffprobe` étant impossible à distinguer autrement de deux
    // process indépendants sans état partagé sur disque).
    fn fake_ffprobe(has_video: bool) -> String {
        if has_video {
            "#!/bin/sh\necho 0\n".to_string()
        } else {
            "#!/bin/sh\n\n".to_string()
        }
    }

    // Fausse `ffmpeg` distinguant la passe intervalle fixe (filtre `fps=`) de
    // la passe changement de scène (filtre `select=`) en inspectant ses
    // propres arguments, pour simuler un mix réaliste : la passe intervalle
    // produit des Frames à 0.0s et 10.0s, la passe scène une Frame à 4.0s
    // (à plus de 3s de toute Frame intervalle : doit survivre à la
    // déduplication) et une Frame à 9.0s (à moins de 3s de 10.0s : doit être
    // rejetée). Répond aussi à l'appel `signalstats` (filtre couleur unie) en
    // annonçant toujours une Frame de contenu normal (large écart de
    // luminance) : ce fake sert à tester la fusion/déduplication, pas le
    // filtre couleur unie, qui a ses propres tests dédiés plus bas.
    const FAKE_FFMPEG_MIXED_PASSES: &str = r#"#!/bin/sh
set -eu
is_scene=0
is_uniform_check=0
for arg in "$@"; do
  case "$arg" in
    *select=*) is_scene=1 ;;
    *signalstats*) is_uniform_check=1 ;;
  esac
done
if [ "$is_uniform_check" = "1" ]; then
  echo "[Parsed_metadata_1 @ 0x0] lavfi.signalstats.YMIN=0" >&2
  echo "[Parsed_metadata_1 @ 0x0] lavfi.signalstats.YMAX=255" >&2
  exit 0
fi
last=""
for arg in "$@"; do
  last="$arg"
done
dir="${last%/*}"
if [ "$is_scene" = "1" ]; then
  printf '' > "$dir/frame-000001.jpg"
  echo "[Parsed_showinfo @ 0x0] n:0 pts_time:4.000" >&2
  printf '' > "$dir/frame-000002.jpg"
  echo "[Parsed_showinfo @ 0x0] n:1 pts_time:9.000" >&2
else
  printf '' > "$dir/frame-000001.jpg"
  echo "[Parsed_showinfo @ 0x0] n:0 pts_time:0.000" >&2
  printf '' > "$dir/frame-000002.jpg"
  echo "[Parsed_showinfo @ 0x0] n:1 pts_time:10.000" >&2
fi
"#;

    // Fausse `ffmpeg` pour les tests dédiés au filtre couleur unie : répond à
    // l'appel `signalstats` selon que le chemin analysé (dernier argument,
    // passé après `-i`) contient `frame-000002` ou non, pour distinguer une
    // Frame uniforme d'une Frame normale sans dépendre d'un vrai fichier
    // image. N'implémente pas les passes d'extraction (non utilisé pour ça).
    const FAKE_FFMPEG_UNIFORM_CHECK: &str = r#"#!/bin/sh
set -eu
case "$*" in
  *frame-000002*)
    echo "[Parsed_metadata_1 @ 0x0] lavfi.signalstats.YMIN=4" >&2
    echo "[Parsed_metadata_1 @ 0x0] lavfi.signalstats.YMAX=50" >&2
    ;;
  *)
    echo "[Parsed_metadata_1 @ 0x0] lavfi.signalstats.YMIN=0" >&2
    echo "[Parsed_metadata_1 @ 0x0] lavfi.signalstats.YMAX=255" >&2
    ;;
esac
"#;

    // Fausse `ffmpeg` pour le test de bout en bout du filtre couleur unie :
    // la passe intervalle produit deux Frames (0.0s normale, 10.0s uniforme,
    // distinguées par leur nom de fichier comme pour `FAKE_FFMPEG_UNIFORM_CHECK`),
    // la passe scène n'en produit aucune (garde le scénario simple).
    const FAKE_FFMPEG_ONE_UNIFORM_ONE_NORMAL: &str = r#"#!/bin/sh
set -eu
is_scene=0
is_uniform_check=0
for arg in "$@"; do
  case "$arg" in
    *select=*) is_scene=1 ;;
    *signalstats*) is_uniform_check=1 ;;
  esac
done
if [ "$is_uniform_check" = "1" ]; then
  case "$*" in
    *frame-000002*)
      echo "[Parsed_metadata_1 @ 0x0] lavfi.signalstats.YMIN=4" >&2
      echo "[Parsed_metadata_1 @ 0x0] lavfi.signalstats.YMAX=50" >&2
      ;;
    *)
      echo "[Parsed_metadata_1 @ 0x0] lavfi.signalstats.YMIN=0" >&2
      echo "[Parsed_metadata_1 @ 0x0] lavfi.signalstats.YMAX=255" >&2
      ;;
  esac
  exit 0
fi
if [ "$is_scene" = "1" ]; then
  exit 0
fi
last=""
for arg in "$@"; do
  last="$arg"
done
dir="${last%/*}"
printf '' > "$dir/frame-000001.jpg"
echo "[Parsed_showinfo @ 0x0] n:0 pts_time:0.000" >&2
printf '' > "$dir/frame-000002.jpg"
echo "[Parsed_showinfo @ 0x0] n:1 pts_time:10.000" >&2
"#;

    const FAKE_FFMPEG_FAILURE: &str = r#"#!/bin/sh
echo "boom: fake ffmpeg failure" >&2
exit 1
"#;

    #[test]
    fn parses_pts_time_from_showinfo_lines() {
        let stderr = "[Parsed_showinfo @ 0x1] n:0 pts_time:0.000 pos:0\n\
                       some unrelated line\n\
                       [Parsed_showinfo @ 0x2] n:1 pts_time:12.5 pos:1\n";

        assert_eq!(parse_showinfo_timestamps(stderr), vec![0.0, 12.5]);
    }

    #[test]
    fn no_video_stream_returns_zero_frames_without_running_ffmpeg() {
        let bin_dir = unique_temp_dir("bin-no-video");
        write_script(&bin_dir, "ffprobe", &fake_ffprobe(false));
        // Si `extract_frames` invoquait quand même `ffmpeg` malgré l'absence
        // de flux vidéo, cette fausse commande échouerait bruyamment.
        write_script(&bin_dir, "ffmpeg", FAKE_FFMPEG_FAILURE);

        let work_dir = unique_temp_dir("work-no-video");
        let input = work_dir.join("audio.mp3");
        fs::write(&input, "fake audio bytes").expect("writing fake input file");
        let tmp_dir = work_dir.join("tmp");
        let frames_dir = work_dir.join("frames");
        fs::create_dir_all(&tmp_dir).expect("creating tmp dir");

        let count =
            extract_frames_with_path(&input, &tmp_dir, &frames_dir, params(), bin_dir.as_os_str())
                .expect("extraction should succeed with zero Frames");

        assert_eq!(count, 0);
        assert!(
            !frames_dir.exists(),
            "no Frames directory should be created when there is no video stream"
        );

        let _ = fs::remove_dir_all(&bin_dir);
        let _ = fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn video_stream_extracts_and_dedups_frames() {
        let bin_dir = unique_temp_dir("bin-video");
        write_script(&bin_dir, "ffprobe", &fake_ffprobe(true));
        write_script(&bin_dir, "ffmpeg", FAKE_FFMPEG_MIXED_PASSES);

        let work_dir = unique_temp_dir("work-video");
        let input = work_dir.join("video.mp4");
        fs::write(&input, "fake video bytes").expect("writing fake input file");
        let tmp_dir = work_dir.join("tmp");
        let frames_dir = work_dir.join("frames");
        fs::create_dir_all(&tmp_dir).expect("creating tmp dir");

        let count =
            extract_frames_with_path(&input, &tmp_dir, &frames_dir, params(), bin_dir.as_os_str())
                .expect("extraction should succeed");

        // Intervalle fixe : 0.0s, 10.0s (toujours gardées). Scène : 4.0s (à
        // plus de 3s des deux Frames intervalle : gardée) et 9.0s (à moins de
        // 3s de 10.0s : rejetée). Total attendu : 3 Frames.
        assert_eq!(count, 3);
        let mut written: Vec<_> = fs::read_dir(&frames_dir)
            .expect("reading frames dir")
            .map(|entry| entry.expect("dir entry").file_name())
            .collect();
        written.sort();
        assert_eq!(written.len(), 3);
        let names: Vec<String> = written
            .iter()
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().any(|name| name.contains("0.000s")));
        assert!(names.iter().any(|name| name.contains("4.000s")));
        assert!(names.iter().any(|name| name.contains("10.000s")));
        assert!(!names.iter().any(|name| name.contains("9.000s")));

        let _ = fs::remove_dir_all(&bin_dir);
        let _ = fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn ffmpeg_failure_surfaces_stderr() {
        let bin_dir = unique_temp_dir("bin-fail");
        write_script(&bin_dir, "ffprobe", &fake_ffprobe(true));
        write_script(&bin_dir, "ffmpeg", FAKE_FFMPEG_FAILURE);

        let work_dir = unique_temp_dir("work-fail");
        let input = work_dir.join("video.mp4");
        fs::write(&input, "fake video bytes").expect("writing fake input file");
        let tmp_dir = work_dir.join("tmp");
        let frames_dir = work_dir.join("frames");
        fs::create_dir_all(&tmp_dir).expect("creating tmp dir");

        let err =
            extract_frames_with_path(&input, &tmp_dir, &frames_dir, params(), bin_dir.as_os_str())
                .expect_err("extraction should fail");

        assert!(format!("{err:#}").contains("boom: fake ffmpeg failure"));

        let _ = fs::remove_dir_all(&bin_dir);
        let _ = fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn missing_ffmpeg_fails_fast() {
        let empty_bin_dir = unique_temp_dir("bin-missing");
        let work_dir = unique_temp_dir("work-missing");
        let input = work_dir.join("video.mp4");
        fs::write(&input, "fake video bytes").expect("writing fake input file");
        let tmp_dir = work_dir.join("tmp");
        let frames_dir = work_dir.join("frames");
        fs::create_dir_all(&tmp_dir).expect("creating tmp dir");

        let err = extract_frames_with_path(
            &input,
            &tmp_dir,
            &frames_dir,
            params(),
            empty_bin_dir.as_os_str(),
        )
        .expect_err("should fail if ffmpeg/ffprobe are absent from PATH");

        assert!(format!("{err:#}").contains("ffmpeg") || format!("{err:#}").contains("ffprobe"));

        let _ = fs::remove_dir_all(&empty_bin_dir);
        let _ = fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn parses_signalstat_values_from_metadata_print_lines() {
        let stderr = "[Parsed_metadata_1 @ 0x1] lavfi.signalstats.YMIN=4\n\
                       some unrelated line\n\
                       [Parsed_metadata_1 @ 0x1] lavfi.signalstats.YMAX=50\n";

        assert_eq!(parse_signalstat(stderr, "YMIN"), Some(4));
        assert_eq!(parse_signalstat(stderr, "YMAX"), Some(50));
        assert_eq!(parse_signalstat(stderr, "UMIN"), None);
    }

    #[test]
    fn uniform_frame_is_detected_via_narrow_luma_range() {
        let bin_dir = unique_temp_dir("bin-uniform-check");
        write_script(&bin_dir, "ffmpeg", FAKE_FFMPEG_UNIFORM_CHECK);

        // `frame-000002` déclenche la branche "uniforme" (YMIN=4, YMAX=50,
        // écart 46) du fake ; tout autre chemin déclenche la branche
        // "normale" (écart 255). Calibré sur une vraie Frame de fondu.
        let uniform = is_near_uniform_color(Path::new("frame-000002.jpg"), bin_dir.as_os_str())
            .expect("checking uniformity should succeed");
        let normal = is_near_uniform_color(Path::new("frame-000001.jpg"), bin_dir.as_os_str())
            .expect("checking uniformity should succeed");

        assert!(uniform, "narrow luma range should be flagged as uniform");
        assert!(!normal, "wide luma range should not be flagged as uniform");

        let _ = fs::remove_dir_all(&bin_dir);
    }

    #[test]
    fn uniform_color_frame_is_filtered_out_of_the_final_frames() {
        let bin_dir = unique_temp_dir("bin-uniform-e2e");
        write_script(&bin_dir, "ffprobe", &fake_ffprobe(true));
        write_script(&bin_dir, "ffmpeg", FAKE_FFMPEG_ONE_UNIFORM_ONE_NORMAL);

        let work_dir = unique_temp_dir("work-uniform-e2e");
        let input = work_dir.join("video.mp4");
        fs::write(&input, "fake video bytes").expect("writing fake input file");
        let tmp_dir = work_dir.join("tmp");
        let frames_dir = work_dir.join("frames");
        fs::create_dir_all(&tmp_dir).expect("creating tmp dir");

        let count =
            extract_frames_with_path(&input, &tmp_dir, &frames_dir, params(), bin_dir.as_os_str())
                .expect("extraction should succeed");

        // Intervalle fixe : 0.0s (normale, gardée) et 10.0s (uniforme,
        // rejetée par le filtre couleur unie). Aucune Frame de la passe
        // scène (fake conçu pour n'en produire aucune, scénario simplifié).
        assert_eq!(count, 1);
        let written: Vec<_> = fs::read_dir(&frames_dir)
            .expect("reading frames dir")
            .map(|entry| entry.expect("dir entry").file_name())
            .collect();
        assert_eq!(written.len(), 1);
        assert!(
            written
                .first()
                .expect("one Frame should have been written")
                .to_string_lossy()
                .contains("0.000s"),
            "only the non-uniform Frame at 0.0s should survive"
        );

        let _ = fs::remove_dir_all(&bin_dir);
        let _ = fs::remove_dir_all(&work_dir);
    }
}
