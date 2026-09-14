# Outils runtime

Le binaire s’appuie sur des Providers locaux, isolés par budget de durée et
d’espace disque.

| Besoin | Outil |
| --- | --- |
| Métadonnées Instagram | `yt-dlp` |
| Acquisition HTTP bornée | `scriptor-binary-acquirer` |
| Pages rendues | `scriptor-page-renderer` et Playwright |
| Audio et keyframes | `ffmpeg` et `ffprobe` |
| Transcription locale | `whisper-cli` de `whisper-cpp` |

Les commandes sont lancées via `ResourceBudget`, qui les place dans un groupe
de processus, borne leurs diagnostics en mémoire et les interrompt quand une
Policy arrive à échéance.
