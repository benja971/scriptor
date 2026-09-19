# scriptor

CLI de capture vérifiable de sources locales et web.

## Language

**Capture**:
L’enregistrement immuable publié pour une Source, ses preuves, extractions,
artefacts, capacités et provenance.

**Source**:
Le fichier local ou Locator web demandé à `scriptor capture`.

**Artefact**:
Le fichier immuable publié par une Capture ou un Dérivé, identifié par son
hash et, lorsqu’il existe, son Locator. Une Preuve et une Extraction sont des
Artefacts.

**Preuve**:
L’artefact source conservé afin de vérifier une Capture. Une publication
sociale conserve ses métadonnées et ses médias comme preuves.

**Extraction**:
Le contenu produit à partir d’une Preuve, comme une caption, une
transcription Whisper ou une keyframe.

**Dérivé**:
L’artefact immuable produit explicitement depuis une Capture par une Recette,
avec ses références de Preuves ou Extractions. Il est lisible par un Agent
selon un contrat stable et inspectable par un humain à partir des mêmes
éléments publiés.

**Fiche de connaissance sourcée**:
Le premier Dérivé explicitement demandé pour rendre une Capture réutilisable
et contrôlable, sans répondre à une question particulière. Elle expose des
éléments attribués à cette Capture et leurs preuves, plutôt que des vérités
établies par Scriptor.

**Noyau de connaissance**:
La structure stable partagée par les Dérivés de connaissance : les Énoncés
attribués et leurs preuves. Il reste indépendant de l’usage métier qui les
organise ou les met en avant.

**Couverture du Dérivé**:
L’état des Artefacts textuels et visuels qu’un Dérivé a examinés, exclus ou n’a
pas pu exploiter. Elle borne explicitement ce que sa Fiche peut représenter.

**Profil de dérivation**:
La configuration JSON versionnée qui adapte un Dérivé de connaissance à un
usage métier, sans modifier son Noyau de connaissance ni contourner la
provenance de ses Énoncés attribués.

**Énoncé attribué**:
L’élément atomique d’une Fiche de connaissance sourcée. Il relie une seule
proposition à un ou plusieurs fragments de Preuves ou d’Extractions, sans
présenter cette proposition comme une vérité externe établie.

**Ancrage de preuve**:
Le lien d’un Énoncé attribué vers l’Artefact qui le soutient, éventuellement
réduit à un fragment localisable. Il permet de remonter de la Fiche au matériau
conservé.

**Déclaration attribuée**:
L’Énoncé attribué qui reformule une affirmation de la Source. Elle ne constitue
ni une citation directe ni une validation de cette affirmation.

**Observation**:
L’Énoncé attribué qui décrit directement ce qui est présent dans une Preuve ou
une Extraction, sans en déduire intention, causalité ou validité.

**Recommandation attribuée**:
L’Énoncé attribué qui rapporte ce que la Source conseille de faire. Elle n’est
pas un conseil donné par Scriptor.

**Interprétation**:
L’Énoncé attribué qui conclut une proposition à partir de prémisses référencées.
Elle reste distincte de ce que la Source a déclaré ou montré directement.

**Incertitude**:
L’Énoncé attribué qui exprime qu’une proposition ne peut pas être établie à
partir de la Capture, avec la limite concrète qui l’explique.

**Vérification externe**:
La démarche explicite et distincte qui évalue une proposition au moyen d’un
corpus de Sources supplémentaire. Elle ne modifie pas l’attribution publiée
par une Fiche de connaissance sourcée.

**Recette**:
La forme typée et autorisée d’un Dérivé, appliquée à une Capture entière ou à
des références sélectionnées.

**Contexte d’inférence**:
Le paquet borné et déclaré de Références d’une Capture qu’une Recette transmet
réellement à un Provider pour produire un Dérivé.

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
