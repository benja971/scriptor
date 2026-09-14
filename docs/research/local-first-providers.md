# Recherche : providers locaux initiaux pour les Captures v2

Recherche menée le 12 septembre 2026 pour le ticket Wayfinder « Choisir les
providers locaux initiaux et leurs frontières ». Le but n'est pas de figer les
interfaces de Scriptor sur des outils actuels, mais de choisir une première
implémentation locale réaliste par Capacité. Toutes les sources sont les dépôts
ou la documentation officielle des projets cités.

## Conclusion

La première version peut rester entièrement locale avec les providers suivants :

| Capacité | Provider initial recommandé | Pourquoi ce choix | Limite à garder hors contrat |
|---|---|---|---|
| Acquisition HTTP directe | client Rust dédié, initialement `reqwest` | téléchargement déterministe des URL de fichier, redirections et en-têtes | ne sait pas extraire une plateforme média ni rendre JavaScript |
| Acquisition média web | `yt-dlp` | téléchargeur CLI couvrant des milliers de sites, déjà présent dans le shell du projet | dépend des extracteurs et des changements externes de sites |
| Inspection, audio et Frames vidéo | `ffprobe` et `ffmpeg` | gère conteneurs, codecs, audio, sous-titres et métadonnées, déjà présent | ne fournit aucune sémantique métier ni transcription |
| Transcription | `whisper-cli` de `whisper.cpp` | exécution locale CPU possible, modèle et accélération échangeables | ASR seulement : pas une interprétation de la Capture |
| Texte PDF et ressources PDF | `pdftotext` et `pdfimages` de Poppler | texte natif et images embarquées sont deux extractions objectives séparées | un scan sans couche texte exige aussi l'OCR |
| OCR image et page PDF rendue | Tesseract | moteur CLI local, langues choisies explicitement, sortie structurée possible | la qualité dépend du modèle de langue et de l'image, pas du contrat |
| Page web rendue et ressources non textuelles | Playwright + Chromium | le navigateur donne le DOM après JavaScript et permet de lister/capturer les ressources observées | pas de promesse d'authentification, de paywall ou de contournement anti-bot |
| HTML/DOM vers Markdown | Trafilatura | extrait texte, métadonnées, images, tableaux et produit du Markdown | c'est une vue lisible, jamais le substitut du DOM ou des ressources conservées |
| Transformations textuelles et visuelles | Ollama, via son API locale | API stablement séparée du runtime, texte et images possibles selon le modèle | le modèle, ses poids et sa licence ne sont pas ceux d'Ollama |
| Recherche plein texte | Tantivy | crate Rust embarquée, index incrémental, BM25 et offsets/positions | l'index est une projection reconstructible, pas le référentiel de Captures |

Cette sélection ne justifie **aucun** couplage des ports Scriptor à des processus
ou crates précis. Les ports portent les Capabilities et leurs résultats :
`MediaAcquirer`, `PageRenderer`, `ObjectiveExtractor`, `TransformationProvider`
et `SearchIndex`. Le manifest enregistre l'implémentation concrète qui a tourné.
Un provider distant ultérieur implémente le même `TransformationProvider`, sans
devenir le chemin obligatoire.

## Contraintes transverses de provenance

Chaque exécution doit enregistrer, à côté de son résultat ou de son échec :

- l'identifiant stable du provider, par exemple `ffmpeg`, `whisper.cpp` ou
  `ollama` ;
- version exacte du binaire ou du crate, et révision `nixpkgs` qui l'a fourni ;
- ligne de commande ou paramètres effectifs, sans secrets ;
- modèle, digest/empreinte du fichier de poids, langue et paramètres de modèle
  lorsque pertinents ;
- hash de chaque entrée et sortie matérielle, ainsi que les références de preuve
  utilisées par un Dérivé.

Ainsi, la version des poids Whisper, Tesseract ou d'un modèle Ollama reste
distincte de la version du runtime qui les exécute. Le fichier `flake.lock` est
la provenance de l'environnement Nix, pas celle d'un modèle téléchargé après
installation.

## Acquisition web et média

### URL directe : client HTTP Scriptor

Une URL de fichier, d'image ou de PDF n'a pas besoin de navigateur ni de
`yt-dlp`. Une implémentation Rust HTTP, initialement `reqwest`, permet de
préserver les redirections, en-têtes significatifs et octets reçus avec leur
hash. `reqwest` est sous licence MIT ou Apache-2.0 et nécessite Rust 1.64 au
minimum selon son manifeste officiel [Cargo.toml de
reqwest](https://github.com/seanmonstar/reqwest/blob/master/Cargo.toml).

Cette Capability ne doit pas promettre que l'URL est encore consultable plus
tard : la preuve durable est l'original téléchargé. Elle doit conserver l'URL
demandée, l'URL finale, date/heure et statut HTTP dans la provenance.

### Plateformes audio/vidéo : yt-dlp

[`yt-dlp`](https://github.com/yt-dlp/yt-dlp) est le premier `MediaAcquirer` :
son projet le présente comme un téléchargeur audio/vidéo CLI prenant en charge
des milliers de sites. Il propose ses binaires Linux x86_64 et musl et exige
Python 3.10+ pour son installation Python ; le projet recommande aussi
`ffmpeg`/`ffprobe` pour fusion et post-traitement
([installation et dépendances](https://github.com/yt-dlp/yt-dlp#installation)).

Le code source et les distributions PyPI sont sous Unlicense. Les binaires
packagés peuvent incorporer des dépendances sous d'autres licences, notamment
GPLv3+ pour PyInstaller : sur NixOS, préférer donc le paquet `nixpkgs` plutôt
qu'un binaire téléchargé opaque
([licences officielles](https://github.com/yt-dlp/yt-dlp#licensing)).

Le provider doit relever `yt-dlp --version`, les formats réellement choisis et
les métadonnées JSON de la Source. Les extracteurs évoluent vite : le projet
indique que sa branche stable peut devenir obsolète quand les sites changent et
publie stable/nightly/master. La version enregistrée est donc indispensable,
mais l'interface ne doit jamais exposer ses options directement.

### Audio, vidéo et Frames : FFmpeg

[`FFmpeg`](https://github.com/FFmpeg/FFmpeg) traite audio, vidéo, sous-titres
et métadonnées ; `ffprobe` inspecte le média et `ffmpeg` convertit ou extrait
audio et Frames. Il est déjà dans le `devShell` de Scriptor. Le projet précise
que son code est principalement LGPL, avec des composants GPL optionnels : la
licence réelle dépend de la configuration du build
([README officiel](https://github.com/FFmpeg/FFmpeg#license)).

Le provider doit persister `ffmpeg -version` et `ffprobe -version`, puis les
propriétés observées (durée, streams, codec, dimensions) et la stratégie de
sélection des Frames. FFmpeg accepte une très grande variété de formats : la
spec ne doit pas énumérer des extensions, mais exprimer des résultats
normalisés - original, stream audio normalisé, sous-titres, Frames et
métadonnées - ou des échecs par Capacité.

## Extractions objectives

### Transcription : whisper.cpp

[`whisper.cpp`](https://github.com/ggml-org/whisper.cpp) est sous MIT, fait de
l'inférence locale de Whisper en C/C++, supporte l'inférence CPU et propose
aussi Vulkan, CUDA et ROCm. Les images officielles couvrent Linux amd64/arm64,
dont une image Vulkan ; le projet fournit `whisper-cli` et ses scripts de
téléchargement de modèles
([README, capacités et usage](https://github.com/ggml-org/whisper.cpp)).

Il reste le meilleur premier `Transcriber` : il est déjà empaqueté dans le
shell Nix et fonctionne sans service résident. Le provider reçoit un audio
normalisé plutôt qu'une URL, puis produit segments, texte et timestamps. Le
manifest doit enregistrer le modèle exact, son hash, langue, options de
décodage et version de `whisper-cli`. Les poids doivent être vérifiés sous leur
propre licence avant distribution, même si le runtime est MIT.

### PDF : Poppler

Les utilitaires [Poppler](https://poppler.freedesktop.org/) séparent bien les
extractions à conserver : `pdftotext` exporte la couche texte et `pdfimages`
extrait les images embarquées. La documentation officielle décrit `pdftotext`
comme un convertisseur PDF vers texte et `pdfimages` comme un extracteur
d'images ([manuel Poppler](https://poppler.freedesktop.org/)). Poppler est GPL
2.0 ou GPL 3.0 selon les composants, à vérifier sur la version Nix retenue.

La stratégie initiale est donc : conserver le PDF original, extraire texte par
page avec la référence de page, extraire les images originales lorsqu'elles
existent, puis rendre les pages qui n'ont pas de couche texte en images pour
l'OCR. La disposition du dossier distinguera clairement texte natif,
texte OCR et ressources image : l'OCR ne doit pas masquer que le PDF était
déjà sélectionnable.

### OCR : Tesseract

[`Tesseract`](https://github.com/tesseract-ocr/tesseract) est un moteur OCR
Apache-2.0 ; ses sources officielles documentent le CLI, les données de langues
`tessdata` et plusieurs formats de sortie. Il s'appuie sur Leptonica pour lire
les images ([README officiel](https://github.com/tesseract-ocr/tesseract)).

Tesseract est le `OcrProvider` initial pour une image importée, un screenshot
et les pages PDF rendues. Utiliser TSV ou hOCR quand disponible afin de
conserver mots, boîtes et confiance, plutôt qu'un simple `.txt`. Cela permet à
une assertion future de référencer une région d'image. Enregistrer version du
binaire, langues/modèles `traineddata`, hash, paramètres et orientation.

## Pages web : DOM, ressources et Markdown

La demande produit impose deux résultats distincts pour une page : le texte
lisible en Markdown **et** les ressources non textuelles. Aucun extracteur
Markdown ne peut remplacer l'acquisition de la page rendue.

[`Playwright`](https://playwright.dev/docs/intro) est le `PageRenderer` initial.
Il supporte Chromium, WebKit et Firefox sur Linux et fonctionne en mode headless
ou headed. Sa documentation d'installation exige actuellement Node 22, 24 ou
26 et ne liste officiellement que Debian/Ubuntu parmi ses distributions Linux
testées. NixOS n'est donc pas une plateforme de support annoncée : empaqueter
le navigateur et ses dépendances dans Nix, et ajouter un test d'intégration
réel avant de déclarer le provider disponible sur la machine.

[Lightpanda](https://lightpanda.io/docs/) est un candidat local à comparer à
Chromium pour les Captures lancées par un Agent. Il exécute JavaScript et expose
CDP, auquel `playwright-core` peut se connecter via
`chromium.connectOverCDP` ([quickstart officiel](https://github.com/lightpanda-io/docs/blob/main/src/content/quickstart.mdx)). Il vise précisément les
charges headless à faible mémoire, mais sa prise en charge des Web APIs reste
partielle et en cours. Il ne peut donc devenir le provider par défaut qu'après
des fixtures réelles couvrant le DOM, les ressources, les téléchargements et
les artefacts visuels requis par Scriptor, avec Chromium comme repli. Son mode
local émet aussi de la télémétrie anonyme par défaut ; la configuration doit
positionner `LIGHTPANDA_DISABLE_TELEMETRY` pour respecter le local-first
([documentation locale ou cloud](https://lightpanda.io/docs/core-concepts/local-vs-cloud)).

Le résultat du renderer doit inclure l'URL finale, le DOM sérialisé après une
politique de stabilisation explicite, une capture d'écran et un inventaire des
ressources effectivement observées. Scriptor télécharge et hashe les ressources
retenues par sa politique, par exemple images, `video`, `audio`, `source`,
posters et documents liés. Il ne promet ni connexion, ni paywall, ni capture
infinie de tout le trafic réseau.

[`Trafilatura`](https://github.com/adbar/trafilatura) est le premier
`MarkdownExtractor` sur ce DOM sauvegardé : il est Apache-2.0 à partir de 1.8.0,
produit notamment Markdown et extrait métadonnées, liens, images et tableaux
([fonctionnalités et licence](https://github.com/adbar/trafilatura#features)).
Il requiert Python, donc doit être fourni par le `devShell`, pas appelé depuis
l'environnement utilisateur implicite. Conserver son HTML/DOM d'entrée et sa
version : Markdown est une extraction dérivée qui peut être régénérée avec un
autre provider.

## Transformations sémantiques locales

[`Ollama`](https://github.com/ollama/ollama) est le `TransformationProvider`
initial, servi localement via son API HTTP. Son runtime est MIT, il fournit une
installation Linux officielle et documente son API locale
([dépôt](https://github.com/ollama/ollama), [API](https://docs.ollama.com/api)).
L'API accepte des prompts et, pour les modèles compatibles, des images : elle
permet donc les recettes de synthèse, assertions, checklist, note, réponse
sourcée et des transformations de Capture visuelle.

Ollama n'est pas un choix de modèle. Les modèles disponibles peuvent avoir des
capacités, exigences matérielles et licences radicalement différentes. La
configuration doit sélectionner explicitement modèle/digest et conserver le
prompt rendu, température et options effectives. Une implémentation cloud ou
un processus local différent pourra satisfaire le même port, à condition de
retourner le Dérivé, la provenance et les références de preuve demandées par
Scriptor.

## Index local

[`Tantivy`](https://github.com/quickwit-oss/tantivy) est une crate Rust MIT,
pas un serveur prêt à déployer. Elle compile sur Rust stable et fournit index
plein texte, BM25, phrases, positions, indexation incrémentale, mmap et champs
structurés ([README officiel](https://github.com/quickwit-oss/tantivy)). Son
démarrage très court la rend cohérente avec un CLI.

Le premier `SearchIndex` peut indexer titre, intention, texte extrait et
Dérivés, avec `capture_id`, `artifact_id` et offsets de preuve stockés comme
champs. Il doit être supprimable et entièrement reconstruisible à partir des
dossiers de Captures. Tantivy ne doit pas devenir un `CaptureRepository` : ses
documents sont immuables et une mise à jour signifie suppression puis
réindexation, ce qui confirme son rôle de projection.

## Décisions reportées

- L'interface exacte de chaque port et le manifest de Capture : ticket dédié.
- Politique précise de ressources d'une page rendue et limites de volume.
- Modèles Whisper, OCR et vision par défaut : décision matérielle et qualité à
  tester sur cette machine, pas une propriété du port.
- Recherche vectorielle, embeddings et graphe : hors premier index plein texte.
- Authentification, cookies connectés, paywalls et contournement des mesures de
  protection : hors périmètre de l'acquisition initiale.
