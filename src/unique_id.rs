//! Génération d'identifiants uniques (pid du process courant + horodatage
//! nanoseconde) pour les fichiers temporaires et objets du Référentiel.

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
