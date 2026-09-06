//! Génération d'identifiants uniques (pid du process courant + horodatage
//! nanoseconde), utilisés pour nommer sans collision le fichier de log du
//! Worker (`main.rs`) et son dossier temporaire (`worker.rs`).

use std::time::{SystemTime, UNIX_EPOCH};

/// Identifiant unique au format `<pid>-<nanos>`, basé sur le pid du process
/// courant et l'horodatage nanoseconde actuel.
#[must_use]
pub fn unique_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("{}-{nanos}", std::process::id())
}
