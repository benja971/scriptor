## Développement

### Environnement

`nix develop` (ou `direnv allow`) fournit la toolchain Rust (nixpkgs stable) et les dépendances
runtime (`ffmpeg`, `whisper-cpp`, `yt-dlp`). Toujours lancer `cargo` à travers ce shell.

### Branches

`main` (prod) ← `develop` (intermédiaire) ← branches de travail (`feature/*`, `ticket/*`,
`fix/*`). Jamais de commit direct sur `main` ou `develop` : une branche, une PR, un merge.
`develop` est la branche par défaut du repo GitHub. `develop` → `main` se fait périodiquement,
pas à chaque PR.

### CI/CD (`.github/workflows/ci.yml`)

- **`check`** : sur chaque push/PR vers `main`/`develop`. `cargo fmt --check`, `cargo clippy
  --all-targets -- -D warnings`, `cargo build`, `cargo test`. Doit passer avant tout merge.
- **`release`** : seulement sur un tag `v*` (ex. `v1.0.0`) qui passe `check`. Build en `--release`
  et publie le binaire (`scriptor-linux-x86_64`) comme asset d'une **release GitHub**. Aucune
  release n'est créée par un simple push ou merge de PR.

### Conventions de code

- Lints clippy stricts dans `Cargo.toml` (`[lints.clippy]`) : `pedantic`/`nursery` en `deny`, plus
  anti-panic explicite (`unwrap_used`, `expect_used`, `panic`, `panic_in_result_fn`, etc.).
  `anyhow` + `.context(...)` à chaque point d'échec, jamais de raccourci qui paniquerait.
- Un fichier par module (`src/foo.rs`), jamais `src/foo/mod.rs`. Pas de `lib.rs` sauf besoin futur
  (second binaire, tests appelant directement les fonctions).
- Vocabulaire du domaine dans `CONTEXT.md` (Source, Pipeline, Worker, Sortie, Modèle) : à utiliser
  de façon cohérente dans le code et les tickets, pas de synonymes.

### Doc du projet

- `docs/spec/` : specs fonctionnelles publiées (une par effort de wayfinder/to-spec).
- `docs/research/rust-tooling.md` : choix de crates et décisions techniques, avec justification.
- `CONTEXT.md` : glossaire du domaine.

## Agent skills

### Issue tracker

Issues tracked as GitHub issues (github.com/benja971/scriptor) via `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Default label vocabulary (needs-triage, needs-info, ready-for-agent, ready-for-human, wontfix). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context layout (CONTEXT.md + docs/adr/ at repo root). See `docs/agents/domain.md`.
