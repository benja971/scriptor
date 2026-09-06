# Spec : scriptor v1 - CLI de transcription audio/vidéo vers texte

Publié comme issue GitHub : [benja971/scriptor#1](https://github.com/benja971/scriptor/issues/1).

Découpée en tickets d'implémentation (task graph) : [config.rs (#2)](https://github.com/benja971/scriptor/issues/2), [cli.rs (#3)](https://github.com/benja971/scriptor/issues/3), [wrappers subprocess (#4)](https://github.com/benja971/scriptor/issues/4), [notify.rs (#5)](https://github.com/benja971/scriptor/issues/5), [orchestration main.rs (#6, bloqué par #2-#5)](https://github.com/benja971/scriptor/issues/6), [tests + validation réelle (#7, bloqué par #6)](https://github.com/benja971/scriptor/issues/7).

## Problem Statement

L'utilisateur veut transcrire en texte l'audio de vidéos ou fichiers audio, locaux ou en ligne (Instagram, TikTok, YouTube, lien direct), sans dépendre d'un service cloud, en réutilisant les outils déjà validés manuellement sur son laptop NixOS (whisper.cpp, ffmpeg, yt-dlp). Il veut lancer une seule commande, retrouver immédiatement la main sur son terminal, et être averti sur son bureau quand la transcription est prête (ou a échoué).

## Solution

Un CLI Rust (`scriptor`) qui prend une Source (chemin de fichier local ou URL) en argument unique. Il valide la présence des binaires externes requis, puis lance un Worker détaché qui exécute le Pipeline (téléchargement si Source distante → extraction audio → transcription → écriture de la Sortie → nettoyage des temporaires) et notifie la fin via `notify-send`. Le process CLI initial rend la main dès que le Worker est lancé.

## User Stories

1. En tant qu'utilisateur, je veux lancer `scriptor <fichier-local>` et retrouver la main sur mon terminal immédiatement, pour continuer à travailler pendant la transcription.
2. En tant qu'utilisateur, je veux que la transcription d'un fichier local produise un `.txt` à côté du fichier source (même nom, extension `.txt`), pour retrouver facilement le résultat.
3. En tant qu'utilisateur, je veux lancer `scriptor <url>` (Instagram, TikTok, YouTube, lien direct) et que la vidéo soit téléchargée automatiquement avant transcription, pour ne pas avoir à télécharger moi-même.
4. En tant qu'utilisateur, je veux que la transcription d'une URL produise un `.txt` dans mon dossier de sortie configuré (`output_dir`), nommé d'après le titre de la vidéo, puisqu'il n'y a pas de fichier source local à côté duquel écrire.
5. En tant qu'utilisateur, je veux recevoir une notification desktop native (`notify-send`) quand la transcription est terminée avec succès, indiquant le chemin du fichier produit.
6. En tant qu'utilisateur, je veux recevoir une notification desktop native quand la transcription échoue, m'indiquant où trouver le détail de l'erreur (fichier de log), pour pouvoir diagnostiquer sans surveiller le terminal.
7. En tant qu'utilisateur au premier lancement, je veux que `~/.config/scriptor/config.toml` soit créé automatiquement avec des valeurs par défaut raisonnables si absent, pour pouvoir l'éditer ensuite sans devoir le créer moi-même.
8. En tant qu'utilisateur, je veux que le chemin du fichier `config.toml` créé me soit annoncé au premier lancement, pour savoir où l'éditer.
9. En tant qu'utilisateur, je veux pouvoir changer le modèle whisper utilisé (`model` dans la config), pour arbitrer moi-même vitesse/précision.
10. En tant qu'utilisateur, je veux pouvoir changer l'emplacement où `scriptor` cherche les fichiers de modèle (`models_dir`), pour ne pas être contraint par un chemin fixe imposé.
11. En tant qu'utilisateur, je veux pouvoir changer le dossier de sortie des transcriptions issues d'une URL (`output_dir`), pour organiser mes fichiers comme je veux.
12. En tant qu'utilisateur, je veux pouvoir changer le nombre de threads utilisés par la transcription (`threads`), pour arbitrer moi-même vitesse/charge CPU.
13. En tant qu'utilisateur, je veux pouvoir changer la langue de transcription (`language`), avec une détection automatique par défaut, pour transcrire du contenu non-anglais sans configuration supplémentaire.
14. En tant qu'utilisateur, si le fichier `.txt` de sortie existe déjà, je veux qu'un nouveau fichier soit créé avec un suffixe numérique (`-1`, `-2`...) plutôt que d'écraser ou d'échouer, pour ne jamais perdre une transcription existante.
15. En tant qu'utilisateur, si `ffmpeg`, `whisper-cli` ou `yt-dlp` (pour une Source distante) est absent de mon système, je veux un message d'erreur clair et un code de sortie non nul avant tout détachement, pour comprendre immédiatement le problème sans attendre une notification.
16. En tant qu'utilisateur, je veux que les fichiers temporaires (audio extrait, vidéo téléchargée) soient nettoyés après le traitement, que celui-ci réussisse ou échoue, pour ne pas accumuler de fichiers inutiles.
17. En tant qu'utilisateur, je veux que le fichier de log du Worker soit conservé après un échec, pour pouvoir diagnostiquer sans avoir à relancer le traitement.
18. En tant qu'utilisateur, je veux que la distinction Source locale / Source distante se fasse automatiquement à partir de l'argument fourni (préfixe `http://`/`https://`), sans option supplémentaire à préciser.

## Implementation Decisions

- **Modules** (fichier par module, `src/foo.rs`, pas de `mod.rs`, pas de `lib.rs` sauf besoin futur) : `cli.rs` (clap derive, un seul argument positionnel `source`), `config.rs` (struct `Config` + chargement, défauts, création automatique), `download.rs` (wrapper `yt-dlp`), `audio.rs` (wrapper `ffmpeg`), `transcribe.rs` (wrapper `whisper-cli` + résolution du chemin du Modèle), `notify.rs` (wrapper `notify-send`), orchestration du détachement dans `main.rs`.
- **Détection Source locale/distante** : préfixe `http://` ou `https://` sur l'argument ⇒ Source distante ; sinon chemin de fichier local.
- **Vérification des binaires** : avant tout détachement, vérifier la présence de `ffmpeg` et `whisper-cli` (toujours), et `yt-dlp` (seulement si Source distante). Échec immédiat (stderr + code de sortie non nul) si un binaire requis manque.
- **Détachement du Worker** : le CLI initial se relance lui-même (`std::env::current_exe()`) avec un mode interne (ex. flag `--worker`), via `Command::process_group(0)`, stdin `Stdio::null()`, stdout/stderr redirigés vers le fichier de log, `.spawn()` sans `.wait()`, puis le process initial se termine (exit 0). Point à valider empiriquement pendant l'implémentation : si `process_group(0)` seul ne suffit pas à survivre à la fermeture du terminal (Ghostty/fish), ajouter un `setsid()` explicite via la crate `nix`.
- **Résolution du Modèle** : chemin construit comme `models_dir/ggml-<model>.bin`.
- **Nom de la Sortie (Source distante)** : titre de la vidéo obtenu via `yt-dlp --print filename` (gabarit sur le titre), slugifié, extension `.txt`, écrit dans `output_dir`.
- **Nom de la Sortie (Source locale)** : même basename que le fichier Source, extension `.txt`, à côté du fichier.
- **Collision de Sortie** : recherche du premier suffixe `-N` libre à partir de 1 (`video.txt` existe → `video-1.txt`, etc.), jamais d'écrasement ni d'erreur.
- **Défauts de configuration** (utilisés si `config.toml` absent ou champ manquant) : `output_dir = "~/Downloads/Transcriptions"`, `model = "small"`, `models_dir = dirs::data_dir()/scriptor/models`, `language = "auto"`, `threads = std::thread::available_parallelism()`.
- **`config.toml` absent** : créé automatiquement avec les défauts ci-dessus au premier lancement ; le chemin créé est annoncé sur stdout avant le détachement.
- **Fichiers temporaires** : `dirs::cache_dir()/scriptor/tmp/<id-unique>/`, supprimés après le Pipeline (succès ou échec).
- **Log du Worker** : `dirs::cache_dir()/scriptor/logs/<horodatage>.log` (via `tracing` + `tracing-subscriber`), conservé après un échec (et un succès).
- **Notification** : `notify-send`, succès `"Transcription terminée : <chemin Sortie>"`, échec `"Échec transcription <Source> : voir <chemin log>"`.
- **Gestion d'erreurs** : `anyhow` avec `.context(...)` à chaque point d'échec ; lints clippy stricts déjà en place dans `Cargo.toml` (`unwrap_used`, `expect_used`, `panic`, `panic_in_result_fn`, etc. en `deny`) interdisent tout raccourci qui paniquerait.
- **Langue et threads** : passés directement à `whisper-cli` via `-l <language>` et `-t <threads>`.

## Testing Decisions

- **Seam unique** (validé avec l'utilisateur) : le binaire `scriptor` compilé, exécuté via `assert_cmd`. Pas de tests unitaires fins sur chaque module en plus de ce seam, sauf logique pure isolable (ex. calcul du nom de Sortie avec suffixe, résolution du chemin du Modèle) qui peut être testée directement en unit test sans passer par le process.
- **Mocks** : `yt-dlp`, `ffmpeg`, `whisper-cli`, `notify-send` remplacés par de faux scripts shell injectés en tête de `PATH` pour la durée du test, simulant succès/échec et écrivant les fichiers attendus (ex. le faux `whisper-cli` écrit directement un `.txt` de test).
- **Isolation** : `HOME`/`XDG_CONFIG_HOME`/`XDG_CACHE_HOME`/`XDG_DATA_HOME` pointés vers un répertoire temporaire par test (`assert_fs`), pour ne jamais toucher la config ou le cache réels de la machine.
- **Détachement** : comme le Worker se détache et que le CLI rend la main immédiatement, les tests ne font pas de `.wait()` sur le process ; ils poll (avec timeout court) l'apparition du fichier Sortie ou du fichier de log.
- **Couverture attendue** : happy path Source locale, happy path Source distante, binaire manquant (échec immédiat, pas de détachement), collision de Sortie (suffixe `-N`), notification de succès, notification d'échec, création automatique de `config.toml` au premier lancement.
- **Tests avec vrais binaires** : marqués `#[ignore]`, lancés manuellement uniquement (dépendent de `ffmpeg`/`whisper-cli`/`yt-dlp` réellement installés et d'un vrai modèle téléchargé).

## Out of Scope

- Traitement batch de plusieurs fichiers/URLs en une seule invocation (pas de parallélisme `rayon` pour cette v1).
- Portabilité au-delà de Linux/NixOS (pas de support Windows/macOS).
- Flags CLI au-delà de l'argument positionnel unique (pas d'override `--model`/`--threads` en ligne de commande ; seule la config `config.toml` pilote ces valeurs).
- Notifications distantes (`ntfy`) : remplacées par `notify-send` natif, tranché explicitement par l'utilisateur.
- Gestion de plusieurs modèles simultanés ou changement de modèle par appel.

## Further Notes

- Le modèle whisper "small" déjà testé manuellement se trouve actuellement dans `/tmp/whisper-test/ggml-small.bin` (emplacement éphémère, `/tmp`). À déplacer/retélécharger vers `models_dir` (`~/.local/share/scriptor/models` par défaut) avant le premier test réel end-to-end, via `whisper-cpp-download-ggml-model small ~/.local/share/scriptor/models`.
- `whisper-cpp-download-ggml-model` ne respecte aucune convention XDG : il télécharge toujours dans le répertoire courant d'exécution, sauf second argument positionnel explicite (fait vérifié empiriquement sur ce système).
- Recherche technique (choix de crates, structure de flake Nix) tracée dans `docs/research/rust-tooling.md`.
- Vocabulaire du domaine (Source, Pipeline, Worker, Sortie, Modèle) fixé dans `CONTEXT.md`.
- Critère de complétion de cette spec : `cargo build`/`cargo test`/`cargo clippy`/`cargo fmt --check` passent, ET une transcription réelle a été validée manuellement sur un fichier local réel et une URL réelle avant de considérer le travail terminé.
