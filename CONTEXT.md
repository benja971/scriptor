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
Le dossier produit par le traitement d'une Source, contenant `transcription.txt`
(le texte transcrit, précédé d'un en-tête `Source : <chemin ou URL>`) et, si la
Source contenait un flux vidéo, un sous-dossier `frames/` (les Frames
extraites). Pour une Source distante, contient aussi les artefacts
intermédiaires dont la conservation a été demandée (`source.<ext>` : vidéo
brute téléchargée, `video-muted.<ext>` : vidéo sans son, `audio.mka` : audio
d'origine) — absents par défaut, activés via la configuration ou les options
`--keep-*`. Emplacement selon le type de Source : à côté du fichier (Source
locale), ou dans le dossier de sortie configuré (Source distante, qui n'a pas
d'emplacement local à côté duquel écrire).
_Avoid_: résultat, output, transcript, fichier de sortie

**Modèle**:
Le modèle ggml whisper utilisé par le Worker pour la transcription, désigné
par son nom (ex. `small`) dans la configuration.
_Avoid_: modèle whisper, checkpoint

**Frame**:
Une image extraite par le Worker d'une Source vidéo, à intervalle fixe et aux
changements de scène (détection ffmpeg), dédoublonnées entre elles, puis
écrites dans le sous-dossier `frames/` de la Sortie. Absentes si la Source est
un fichier audio sans flux vidéo.
_Avoid_: image, capture, screenshot, thumbnail
