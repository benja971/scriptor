# scriptor

CLI de capture vérifiable de sources locales et web. Une capture conserve sa
preuve, ses médias, ses extractions et leur provenance dans un Référentiel
local.

## Usage

```bash
scriptor capture <fichier-ou-url> --policy safe-local@1
scriptor capture <url> --policy safe-web@1
scriptor job wait <job_id>
scriptor capture inspect <capture_id>
scriptor capture search "une expression"
```

`safe-web@1` capture les pages publiques et les publications Instagram ou
LinkedIn. Pour ces publications, Scriptor récupère la caption et les médias
dans leur ordre de publication. Chaque vidéo capturée produit aussi une
transcription locale via Whisper et des keyframes via ffmpeg.

La recherche parcourt les preuves textuelles, captions et transcriptions
publiées. Elle effectue actuellement une correspondance textuelle
insensible à la casse.

## Développement

```bash
nix develop
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build
cargo test
```

Le devShell fournit `ffmpeg`, `whisper-cpp`, `yt-dlp`, les outils Rust et les
providers web emballés par le projet.
