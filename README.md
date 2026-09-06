# scriptor

CLI de transcription audio/vidéo vers texte, 100% local (CPU-only, via
[whisper.cpp](https://github.com/ggml-org/whisper.cpp)).

Prend en entrée un fichier local (audio ou vidéo) ou une URL (Instagram, TikTok,
YouTube, lien direct...), et produit un fichier `.txt` de la transcription.

## Dépendances système

Non fournies par Cargo, requises au runtime :

- [`ffmpeg`](https://ffmpeg.org/) : extraction audio (16kHz mono PCM16)
- [`whisper-cpp`](https://github.com/ggml-org/whisper.cpp) (paquet nixpkgs `whisper-cpp`,
  binaire `whisper-cli`) : transcription
- [`yt-dlp`](https://github.com/yt-dlp/yt-dlp) : téléchargement si l'entrée est une URL

Sur NixOS/Nix, le `flake.nix` du projet fournit ces trois dépendances plus la
toolchain Rust dans un devShell (voir [Développement](#développement)).

Modèle whisper à télécharger séparément : `whisper-cpp-download-ggml-model small`.

## Configuration

`~/.config/scriptor/config.toml` :

```toml
output_dir = "~/Downloads/Transcriptions"
model = "small"
threads = 16
```

- `output_dir` : dossier de sortie des `.txt` quand l'entrée est une URL (pas de
  fichier source local à côté duquel écrire).
- `model` : nom du modèle ggml whisper à utiliser.
- `threads` : nombre de threads passés à `whisper-cli`.

## Usage

```
scriptor <fichier-local-ou-url>
```

- Entrée = fichier local : le `.txt` est écrit à côté du fichier source (même nom,
  extension `.txt`).
- Entrée = URL : téléchargement via `yt-dlp`, puis `.txt` écrit dans `output_dir`.

La commande rend la main immédiatement ; le traitement tourne en arrière-plan en
process détaché. Une notification est envoyée en fin de traitement (`ntfy`).

## Développement

Environnement reproductible via Nix :

```
nix develop
# ou, avec direnv installé :
direnv allow
```

Fournit `cargo`, `rustc`, `rust-analyzer`, `clippy`, `rustfmt`, `ffmpeg`,
`whisper-cpp`, `yt-dlp`.

```
cargo build
cargo test
cargo clippy
cargo fmt
```

Choix d'outils et recherche documentés dans
[`docs/research/rust-tooling.md`](docs/research/rust-tooling.md).
