# scriptor

CLI de transcription audio/vidéo vers texte, 100% local via whisper.cpp.

## Language

**Source**:
L'entrée fournie en argument au CLI : soit un chemin de fichier local (audio ou
vidéo), soit une URL (Instagram, TikTok, YouTube, lien direct...). Une Source
distante doit d'abord être téléchargée (via yt-dlp) avant de rejoindre le même
Pipeline qu'une Source locale.
_Avoid_: entrée, input, cible, média

**Pipeline**:
La séquence de traitement appliquée à une Source jusqu'à produire sa Sortie :
téléchargement (si Source distante) → extraction audio → transcription →
écriture de la Sortie → nettoyage des fichiers temporaires.
_Avoid_: traitement, job, workflow

**Worker**:
Le process qui exécute réellement le Pipeline en arrière-plan, détaché du
process CLI initial. Le CLI lance le Worker puis rend la main immédiatement ;
le Worker notifie la fin du Pipeline via notify-send.
_Avoid_: daemon, process de fond, tâche de fond

**Sortie**:
Le fichier `.txt` produit par la transcription d'une Source. Emplacement selon
le type de Source : à côté du fichier (Source locale), ou dans le dossier de
sortie configuré (Source distante, qui n'a pas d'emplacement local à côté
duquel écrire).
_Avoid_: résultat, output, transcript

**Modèle**:
Le modèle ggml whisper utilisé par le Worker pour la transcription, désigné
par son nom (ex. `small`) dans la configuration.
_Avoid_: modèle whisper, checkpoint
