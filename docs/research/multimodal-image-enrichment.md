# Recherche : rendre les images et Frames exploitables sans perdre leur couverture

Recherche menée le 15 septembre 2026 pour préparer l'enrichissement d'un
second brain multimodal. Elle traite le cas Scriptor : une Capture possède
l'original, ses Frames temporelles et par changement de scène, sa caption et sa
transcription ; les traitements ultérieurs doivent rendre ce matériau
interrogeable sans considérer une vue réduite comme la vérité de la Capture.

Les sources sont des publications originales ou documentations des outils et
modèles cités. Elles ne suffisent pas à valider un choix de modèle pour les
contenus réels de l'utilisateur : ce choix exigera un jeu de Captures de
référence et une évaluation locale.

## Conclusion

Ne pas appliquer une deuxième déduplication destructive aux Frames déjà
extraites. La bonne séparation est :

```text
Original et Frames de couverture, immuables
  -> extractions objectives par artefact : OCR, timestamp, hash, dimensions
  -> représentations de recherche, reconstructibles : texte et embeddings
  -> vue de contexte bornée, reconstruite pour une Recette ou une question
  -> Dérivé avec les références réellement transmises
```

La sélection actuelle de Scriptor est déjà une stratégie de **couverture**, non
une simple extraction à une frame par seconde : elle prend une Frame toutes les
dix secondes, ajoute les changements de scène, conserve les timestamps réels,
évite seulement les Frames temporellement proches et enlève les écrans quasi
unis ([`src/frames.rs`](../../src/frames.rs)). Elle préserve donc un échantillon
temporel et les coupures visuelles. Elle ne prouve toutefois pas qu'un texte ou
un détail qui apparaît brièvement entre deux échantillons sera retenu.

La recherche sur la sélection de résumés vidéo formule ce compromis par la
diversité et la représentativité du contenu original
([Mahasseni, Lam et Todorovic, 2018](https://arxiv.org/abs/1801.00054)). Pour
une question donnée, il faut en plus la pertinence à la question : la sélection
query-adaptive vise explicitement une couverture simultanément diverse,
représentative et pertinente ([Sharghi, Gong et Shah, 2017](https://arxiv.org/abs/1705.00581)).
Une vue à budget fixe est donc une décision d'usage réversible, pas une
élimination d'artefacts du référentiel.

## Ce qui doit rester distinct

| Couche | But | Entrée / sortie | Peut-elle modifier ou supprimer les Frames ? |
|---|---|---|---|
| Capture | Conserver ce qui a été acquis et permettre l'audit | original, Frames, caption, transcription, provenance | Non |
| Extraction objective | Rendre un fait observable sans l'interpréter | OCR avec régions et confiances, dimensions, timestamp, hash | Non |
| Index | Retrouver des artefacts candidats | index plein texte et vecteurs, reconstruits depuis le référentiel | Non |
| Sélection de contexte | Respecter un budget d'inférence pour une question ou Recette | liste ordonnée de références d'artefacts | Non |
| Transformation IA | Produire un Dérivé explicite | résumé, note, affirmations et citations | Non |

Cette distinction évite deux erreurs : utiliser un résumé de Frame comme une
preuve, ou considérer l'absence d'un résultat de recherche comme l'absence de
l'information dans la Capture.

## OCR : extraction déterministe, granulaire et réutilisable

L'OCR doit être exécuté séparément sur chaque image et Frame retenue. C'est une
Extraction, pas un résumé visuel et pas un appel LLM : son résultat garde le
lien direct avec le fichier et son timestamp. La documentation officielle de
[Tesseract](https://tesseract-ocr.github.io/tessdoc/Command-Line-Usage.html)
décrit les sorties hOCR et TSV. Le TSV expose notamment mot, boîte
(`left`, `top`, `width`, `height`) et confiance. Ce format permet :

- de chercher du texte visible dans une Frame ;
- de faire remonter au modèle le passage OCR utile avec sa région ;
- de citer ensuite `frame:<artifact-id>#region:<id>`, au lieu de prétendre que
  le texte vient de la transcription ;
- de réexécuter l'OCR avec une autre langue, orientation ou version de
  `traineddata`, sans écraser le premier résultat.

Scriptor possède déjà ce contrat pour une image importée : l'extraction
`image-ocr` conserve des régions typées et leur provenance
([test d'intégration](../../tests/cli.rs)). Il faut l'étendre aux Frames, sans
convertir les régions en simple texte sans localisation. Tesseract distingue
également le moteur et les données de langue installées ; les deux doivent être
enregistrés dans la provenance
([documentation d'installation](https://tesseract-ocr.github.io/tessdoc/Installation.html)).

L'OCR de chaque Frame peut être coûteux. Ce n'est pas une raison de détruire
les Frames : c'est un Job reprenable et borné, avec état partiel par artefact.
Une première politique pragmatique est de l'exécuter sur les Frames déjà
retenues, puis de prévoir ultérieurement une passe visuelle basse résolution
plus dense seulement pour détecter de nouveaux écrans textuels. Cette seconde
passe serait un nouvel artefact de couverture, jamais un remplacement des
Frames actuelles.

## Index visuel et multimodal

Un index texte seul ne retrouve une image que si son OCR, sa caption ou sa
transcription contient les mots de la question. Un encodeur image-texte
contrastif fournit une autre voie : CLIP apprend à rapprocher des paires
image-texte et est évalué sur la recherche image-texte dans sa publication
originale ([Radford et al., 2021](https://cdn.openai.com/papers/Learning_Transferable_Visual_Models_From_Natural_Language_Supervision.pdf)).
Un espace commun peut donc servir à retrouver des Frames à partir d'une requête
textuelle, et inversement.

L'unité indexée doit être **l'artefact**, pas une moyenne opaque de la vidéo :

- une entrée vectorielle par image ou Frame, avec `capture_id`, `artifact_id`,
  timestamp, hash du fichier et version exacte du modèle ;
- une entrée texte par caption, segment de transcription et sortie OCR ;
- les résultats retournent les artefacts sources. Une Capture peut ensuite être
  regroupée au niveau présentation, sans perdre la Frame qui a justifié le hit.

Cette granularité correspond au type de retrieval démontré par CLIP, mais ne
garantit pas l'exactitude sur les interfaces, le texte fin, les comptes ou les
comptages. La carte de modèle officielle signale précisément les limites sur la
classification fine, le comptage et les cas hors distribution
([CLIP model card](https://github.com/openai/CLIP/blob/main/model-card.md)).
Les embeddings sont donc un mécanisme de rappel de candidats, jamais une
preuve et jamais l'unique chemin pour retrouver du texte : le plein texte OCR
et transcription reste nécessaire.

Les encodeurs unifiés peuvent à terme relier texte, image et audio. ImageBind
est un exemple de recherche qui place six modalités dans un même espace et
montre la recherche intermodale ([publication](https://arxiv.org/abs/2305.05665),
[implémentation officielle](https://github.com/facebookresearch/ImageBind)).
Ce n'est pas un candidat par défaut pour Scriptor : ses poids sont sous
CC-BY-NC 4.0 et son exécution exige PyTorch. Il établit seulement qu'un port
`EmbeddingProvider` doit porter les modalités et la version du modèle, pas le
nom d'un fournisseur.

## Construire un contexte d'inférence sans trou silencieux

Les APIs multimodales ont toujours un budget. Par exemple, la documentation
officielle Gemini indique qu'en mode statique vidéo elle échantillonne par
défaut à 1 FPS, que les séquences rapides peuvent perdre du détail et que la
résolution d'un frame consomme elle aussi un budget de tokens
([video understanding](https://ai.google.dev/gemini-api/docs/video-understanding)).
Ce comportement est une illustration de contrainte, pas un provider à adopter.
Scriptor ne doit pas déléguer sans trace son choix de Frames à une API distante.

Pour chaque Recette ou question, le constructeur de contexte doit sélectionner
et enregistrer un paquet explicitement borné :

1. inclure la caption et les segments de transcription pertinents, référencés ;
2. inclure les hits OCR et visuels les plus pertinents pour la requête ;
3. réserver des Frames de couverture temporelle - début, milieu, fin ou une
   Frame par intervalle - afin que les seuls hits ne masquent pas le déroulé ;
4. dédupliquer uniquement **dans le paquet éphémère** des quasi-doublons qui
   concurrencent le même budget, tout en gardant leurs références disponibles ;
5. conserver dans le Dérivé la liste ordonnée, hashes, timestamps, dimensions,
   budget et règle de sélection réellement employés.

Une Recette sans question, comme `structured-summary`, n'a pas de signal de
pertinence externe. Elle sélectionne donc d'abord la couverture temporelle,
puis enrichit avec les Frames dont l'OCR apporte le plus de texte nouveau. Une
Recette qui répond à une question emploie en plus la pertinence de recherche.
Cette différence doit faire partie du contrat de Recette, pas d'un prompt
implicite.

La sélection actuelle de Frames a déjà démontré localement qu'une comparaison
de pixels ou SSIM seule ne séparait pas de façon fiable les changements
sémantiquement utiles des redondances visuelles
([raisonnement et mesures dans `merge_and_write`](../../src/frames.rs)). Cela
renforce la conclusion : ne pas ajouter un seuil visuel global destructif. Un
index multimodal peut aider à ordonner les candidats, mais son score ne doit
jamais faire disparaître les preuves de couverture.

## Provenance et contrat de Dérivé

Une transformation générative doit déclarer ce qui quitte la machine. Pour les
images, la référence minimale est : `capture_id`, `artifact_id`, hash de
l'artefact, timestamp si Frame, et, pour une citation OCR, région et texte
correspondants. Pour chaque exécution, persister également :

- provider, endpoint non secret, modèle et digest/version quand disponible ;
- paramètres effectifs, version du constructeur de contexte et prompt rendu ;
- liste exacte des entrées retenues et raisons de sélection ;
- budget demandé et consommation retournée si le provider la communique ;
- sortie brute, sortie validée et références de preuve validées par Scriptor.

Une affirmation générée qui pointe vers une Frame doit être refusée si cette
Frame ne figure pas dans les entrées déclarées. Une affirmation OCR doit pouvoir
être vérifiée contre la région conservée. C'est cette traçabilité, et non une
description produite par un VLM, qui rend un second brain inspectable.

## Décisions actionnables pour Scriptor

1. Conserver la stratégie actuelle intervalle + changement de scène + retrait
   des écrans unis. Ne pas supprimer les Frames après une seconde passe de
   similarité.
2. Ajouter l'OCR des Frames comme Capability indépendante, avec TSV ou hOCR,
   régions, confiance, langue et version du modèle. Indexer son texte avec les
   captions et transcriptions.
3. Introduire plus tard un `EmbeddingProvider` distinct du provider génératif.
   Son résultat est une projection reconstructible au niveau `artifact_id`.
4. Définir un `ContextBuilder` versionné par Recette. Il produit une liste
   déclarée d'artefacts, jamais un booléen vague `whole-capture` signifiant
   l'envoi de tous les médias.
5. Faire remonter le résultat au niveau Capture pour l'usage, mais toujours
   afficher la Frame, le timestamp et l'extrait OCR ou transcript qui ont servi
   de preuve.
6. Avant de choisir un modèle d'embedding ou de vision, constituer un petit jeu
   de requêtes réelles : retrouver une interface précise, un texte visible,
   une idée dite seulement à l'oral, et une information présente dans une Frame
   non représentative. Mesurer rappel, faux positifs, temps et mémoire. Sans
   cette évaluation, le nom du modèle serait une préférence, pas une décision.
