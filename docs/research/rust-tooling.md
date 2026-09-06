# Recherche : tooling Rust pour scriptor (2025-2026)

Recherche menée avant l'implémentation, pour choisir les outils/crates et le setup Nix.
Contexte : premier projet Rust, usage 100% local et perso, pas de contrainte de perf sur le
code Rust (le goulot d'étranglement est dans les binaires externes : ffmpeg, whisper-cli, yt-dlp).

## 1. Environnement Nix (flake.nix)

**Toolchain Rust** : `nixpkgs` stable seul suffit pour un devShell simple (pas de build Nix du
binaire final). Alternative si besoin d'une version plus récente : `rust-overlay` (oxalica),
recommandation communautaire la plus mûre en 2025-2026, plutôt que `fenix` (plus léger mais
nécessite d'épingler soi-même les révisions).

**Structure du flake** : un seul `devShells.default` avec `mkShell`, mélangeant toolchain Rust
et paquets runtime (ffmpeg, whisper-cpp, yt-dlp) dans `buildInputs`. Pas besoin de `flake-utils`
pour un flake mono-système (perso, une seule machine) : `system` codé en dur, cf. l'article
["1000 Instances of flake-utils"](https://nixcademy.com/posts/1000-instances-of-flake-utils/)
qui documente le bloat causé par cette dépendance quasi-systématique mais évitable.

`whisper-cpp` sur nixpkgs (branche master, vérifié) fournit toujours le binaire `whisper-cli`
comme `meta.mainProgram`, ainsi que `whisper-cpp-download-ggml-model`. Pas de renommage récent.

`direnv` + `.envrc` (`use flake`) + `nix-direnv` (cache persistant) reste la pratique standard
pour l'auto-activation du shell.

Exemple minimal :

```nix
{
  description = "scriptor dev shell";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
    in {
      devShells.${system}.default = pkgs.mkShell {
        buildInputs = [
          pkgs.cargo pkgs.rustc pkgs.rust-analyzer pkgs.clippy pkgs.rustfmt
          pkgs.ffmpeg pkgs.whisper-cpp pkgs.yt-dlp
        ];
      };
    };
}
```

Sources : [oxalica/rust-overlay](https://github.com/oxalica/rust-overlay),
[nix-community/fenix](https://github.com/nix-community/fenix),
[Rust - NixOS Wiki](https://nixos.wiki/wiki/Rust),
[nixpkgs whisper-cpp package.nix](https://github.com/NixOS/nixpkgs/blob/master/pkgs/by-name/wh/whisper-cpp/package.nix),
[nix-community/nix-direnv](https://github.com/nix-community/nix-direnv).

## 2. Crates CLI

| Besoin | Choix retenu | Alternatives écartées | Pourquoi |
|---|---|---|---|
| Parsing d'arguments | `clap` v4 (derive) | `bpaf`, `pico-args` | Standard de facto, garde la porte ouverte à des flags futurs (`--model`, `--threads`) ; le surcoût (taille binaire, compilation) est négligeable ici |
| Erreurs | `anyhow` seul | `thiserror`, `eyre` | Bin unique, pas de lib exposée à matcher ailleurs ; `thiserror` inutile sans enum d'erreur consommé par un autre crate |
| Config | `serde` + `toml` + `dirs` | `config-rs`, `figment`, `directories` | Un seul fichier, pas de fusion multi-source ; `dirs::config_dir()` suffit pour le chemin XDG |
| Subprocess | `std::process::Command` | `duct`, `subprocess` | Pipeline strictement séquentiel, pas de pipe OS entre commandes ; `duct` n'apporte rien ici |
| Détachement de process | `Command::process_group(0)` + stdio redirigé vers fichier de log, ré-appel du binaire en mode "worker" interne | `daemonize`, double-fork manuel (`nix`) | Stable depuis Rust 1.64, suffisant pour "survit à la fermeture du terminal" sur un usage perso ; à valider empiriquement (lancer, fermer le terminal, vérifier `ps`), sinon ajouter un `setsid()` explicite via `nix` en secours |
| Logging | `tracing` + `tracing-subscriber` vers fichier | `log` + `env_logger` | Le process détaché n'a plus de terminal attaché : `tracing` est le chemin le mieux documenté pour écrire proprement dans un fichier depuis un process de fond |

Nuance sur le détachement : `process_group(0)` change le groupe de processus, pas la session.
Le `SIGHUP` de fermeture de terminal cible généralement la session. Comportement à vérifier
empiriquement dans Ghostty/fish avant de considérer que c'est suffisant.

## 3. Structure de projet

```
src/
  main.rs        # orchestration fine : parse args, charge config, appelle le pipeline
  cli.rs         # struct Args (clap derive)
  config.rs      # struct Config, load/save, defaults
  download.rs    # wrapper yt-dlp
  audio.rs       # wrapper ffmpeg
  transcribe.rs  # wrapper whisper-cli
  notify.rs      # wrapper ntfy
```

Pas de `lib.rs` sauf besoin futur (second binaire, ou tests d'intégration appelant directement
les fonctions sans passer par le process). Un fichier par module (`src/foo.rs`), pas
`src/foo/mod.rs` (style pré-2018, déprécié).

## 4. Tests

`assert_cmd` (exécute le binaire compilé, vérifie stdout/stderr/exit code) + `predicates`
(assertions lisibles) + `assert_fs` (répertoire de travail temporaire isolé). Pour mocker
yt-dlp/ffmpeg/whisper-cli : injecter un `PATH` de test pointant vers de faux scripts shell.
Tests d'intégration "réels" (avec les vrais binaires) marqués `#[ignore]`, lancés manuellement.

Sources : [rust-cli-recommendations (Rain)](https://rust-cli-recommendations.sunshowers.io/cli-parser.html),
[oneuptime.com - error types 2026](https://oneuptime.com/blog/post/2026-01-25-error-types-thiserror-anyhow-rust/view),
[Rustify - tracing vs log 2026](https://rustify.rs/articles/rust-tracing-vs-log-crates-2026),
[rust-cli.github.io book - testing](https://rust-cli.github.io/book/tutorial/testing.html),
[RFC 3228 - process_group](https://rust-lang.github.io/rfcs/3228-process-process_group.html).

## Décisions restant à valider avec l'utilisateur

- Flake : nixpkgs stable seul (pas de rust-overlay), sauf si la version de Rust s'avère trop
  ancienne à l'usage.
- Détachement de process : `process_group(0)` d'abord, `nix`/`setsid` en secours si insuffisant
  empiriquement.

## 5. Lints clippy (qualité de code)

`[lints.clippy]` dans `Cargo.toml` (pas `[workspace.lints.clippy]` : un seul crate, pas de
workspace multi-paquets ici) : `pedantic` + `nursery` en `deny`, plus des lints anti-panic
explicites (`unwrap_used`, `expect_used`, `indexing_slicing`, `arithmetic_side_effects`,
`unreachable`, `unimplemented`, `panic`, `panic_in_result_fn`, `exit`, `as_conversions`,
`string_slice`) et quelques lints d'idiome (`clone_on_ref_ptr`, `clone_on_copy`,
`undocumented_unsafe_blocks = "forbid"`).

Conséquence directe sur le choix d'erreurs (section 2) : `unwrap_used`/`expect_used`/`panic`
étant interdits, `anyhow` avec `.context(...)` systématique à chaque point d'échec devient la
voie normale de gestion d'erreur, pas une option parmi d'autres. `eyre` (fork d'anyhow, meilleur
reporting/coloration) reste une alternative valable mais sans bénéfice concret ici (pas de
terminal interactif pour la majorité de l'exécution, cf. process détaché) ; `thiserror` reste
hors scope tant que scriptor n'expose pas de lib consommée par un autre crate.

## 6. Parallélisme

Pas de runtime async (`tokio`) : le pipeline est séquentiel par fichier (download → extraction →
transcription), et les binaires externes sont déjà le goulot d'étranglement, pas le code Rust.
Si un besoin de parallélisme apparaît plus tard (ex : traiter plusieurs fichiers/URLs en une
invocation), privilégier **`rayon`** (parallel iterators, synchrone) plutôt qu'un runtime async :
plus simple, pas de coloration `async`/`await` à propager dans tout le code pour un cas d'usage
qui reste fondamentalement du travail CPU/IO bloquant par nature (spawn de process). Pas encore
ajouté en dépendance : aucun besoin actuel dans le scaffold.
