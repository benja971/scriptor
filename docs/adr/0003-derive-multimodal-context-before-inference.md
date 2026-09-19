# Construire un contexte multimodal traçable avant l'inférence

## Status

accepted

Un Dérivé génératif ne reçoit jamais automatiquement une Capture brute. Sa Recette construit un contexte multimodal borné à partir du périmètre explicitement sélectionné : texte extrait, caption, transcription, OCR et frames. Les frames retenues lors de la Capture constituent déjà la couverture visuelle de référence : une seconde réduction n'est admise que si un budget l'exige, avec son motif, les frames transmises et les frames exclues enregistrés dans le Dérivé. La vidéo brute n'est pas transmise à un modèle tant qu'aucun Provider ne déclare cette capacité.

## Considered Options

- Transmettre tous les fichiers de la Capture au Provider : simple, mais coûteux, ambigu et risqué pour une inférence distante.
- Réduire systématiquement une seconde fois les frames avant l'inférence : réduit le contexte, mais peut perdre l'information que la sélection visuelle de Capture a préservée.
- Construire un contexte par Recette et le tracer : retenu. La Recette contrôle les artefacts admissibles, le Provider déclare texte, vision et sortie structurée, et la Policy autorise explicitement tout Provider distant.

## Consequences

`--whole-capture` désigne le périmètre que la Recette peut consulter, non un droit de transmettre indistinctement tous ses fichiers. Le Dérivé conserve les Références effectivement transmises, le Provider, le modèle, les paramètres effectifs et le prompt rendu, sans secret. Les Providers locaux et distants satisfont le même contrat ; leur endpoint, leurs capacités, leurs budgets et leur référence de secret sont configurés hors des Captures et explicitement autorisés par Policy. La recherche multi-Captures et l'inférence vidéo restent des décisions ultérieures.

L'OCR, l'analyse vision et les embeddings restent trois sorties distinctes. L'OCR est une Extraction par image ou Frame, avec texte exact, régions, confiance et timestamp : il sert à la recherche lexicale et à une preuve textuelle localisable. L'analyse vision est un Dérivé : elle exprime uniquement des observations visuelles sourcées par les Frames effectivement transmises, sans prétendre fournir des coordonnées ou du texte exact que le modèle ne peut pas garantir. Un embedding est une projection reconstructible au niveau de l'artefact, utile pour rappeler des candidats mais jamais une Preuve.

Une Recette d'analyse visuelle demande une sortie structurée selon son usage, non une description libre systématique de chaque Frame. Elle peut notamment produire le type de contenu observé, des observations visuelles, des motifs de design et des incertitudes, chaque élément pointant vers une Référence de Frame. Une synthèse combine caption, transcription, OCR et observations visuelles, et distingue ce qui est dit, ce qui est textuellement visible et ce qui est seulement observé.

Une synthèse sans question privilégie la couverture temporelle et les Frames qui apportent du texte OCR nouveau. Une réponse à une question ajoute les candidats remontés par OCR, transcription ou index visuel, tout en gardant des Frames de contexte temporel. L'analyse vision de toutes les Frames n'est pas lancée automatiquement à la Capture : c'est une Recette explicite, bornée et rejouable.

Le Contexte d'inférence est un artefact versionné produit avant l'appel du Provider. Il porte au minimum la Recette, le budget effectif, les Références retenues, le rôle de chaque entrée (`caption`, `transcription`, `visible-text`, `visual-frame`), son motif de sélection (`source-context`, `scene-change`, `coverage`, `retrieval-hit`, `new-ocr-text`) et la règle qui décrit les candidats écartés. Le Provider ne reçoit que ce manifeste et les artefacts retenus. Le Dérivé rend la liste des Références effectivement transmises vérifiable après exécution.

Le premier flux complet cible `structured-summary`, sans créer prématurément une Recette d'analyse visuelle distincte. Sa sortie structurée sépare résumé, idées importantes, texte visible, observations visuelles et incertitudes. Chaque élément généré doit référencer un artefact présent dans le Contexte d'inférence ; Scriptor vérifie cette appartenance avant publication. Une Recette spécialisée ne sera créée que lorsqu'un usage distinct ne peut pas être représenté par cette sortie.
