# scriptor

CLI de capture vérifiable de sources locales et web.

## Language

**Capture**:
L’enregistrement immuable publié pour une Source, ses preuves, extractions,
artefacts, capacités et provenance.

**Source**:
Le fichier local ou Locator web demandé à `scriptor capture`.

**Preuve**:
L’artefact source conservé afin de vérifier une Capture. Une publication
sociale conserve ses métadonnées et ses médias comme preuves.

**Extraction**:
Le contenu produit à partir d’une Preuve, comme une caption, une
transcription Whisper ou une keyframe.

**Provider**:
Le programme ou composant qui réalise une capacité de Capture, avec ses
paramètres et dépendances versionnées.

**Policy**:
Le document versionné qui autorise les opérations, Providers, appels distants
et budgets d’une Capture.

**Job**:
L’exécution persistante d’une Capture ou d’un Dérivé. Il expose son état et
ses erreurs structurées.

**Référentiel**:
Le stockage local des Captures, Jobs, index et Dérivés.
