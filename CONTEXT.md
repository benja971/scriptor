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

**Dérivé**:
L’artefact immuable produit explicitement depuis une Capture par une Recette,
avec ses références de Preuves ou Extractions.

**Recette**:
La forme typée et autorisée d’un Dérivé, appliquée à une Capture entière ou à
des références sélectionnées.

**Capacité**:
L’opération indépendante enregistrée pour une Capture, avec son Provider, son
état et, le cas échéant, son erreur structurée.

**Provider**:
Le programme ou composant qui réalise une capacité de Capture, avec ses
paramètres et dépendances versionnées.

**PageRenderer**:
Le Provider Web qui produit le DOM stabilisé, le Markdown, les découvertes et
les preuves visuelles d’une page publique.

**Découverte**:
Une Source révélée par une Capture Web, avec sa preuve parente, son locator et
son ordre d’observation. Elle peut être poursuivie explicitement par l’Agent.

**Doublon**:
Une Capture existante pour la même identité de Source, réutilisée seulement si
la Policy l’autorise.

**Contrat agent**:
L’interface JSON stable de Scriptor pour créer, suivre, inspecter, lire,
rechercher et dériver des Captures.

**Policy**:
Le document versionné qui autorise les opérations, Providers, appels distants
et budgets d’une Capture.

**Référentiel**:
Le stockage local des Captures, Jobs, index et Dérivés.

**Index de recherche**:
La projection reconstruisible des contenus publiés par le Référentiel. Elle
accélère la recherche sans posséder les Captures ni leurs preuves.

**Job**:
L’exécution persistante d’une Capture ou d’un Dérivé. Il expose son état et
ses erreurs structurées.
