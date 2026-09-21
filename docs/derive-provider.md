# Provider local de Dérivés

Le Contrat agent crée un Dérivé avec une sélection explicite :

```console
scriptor derive <capture_id> \
  --recipe knowledge-card \
  --provider scriptor-local-derive \
  --policy safe-local@1 \
  --whole-capture
```

`--whole-capture` définit un périmètre de consultation, pas une transmission
automatique de tous les fichiers. Scriptor y retient les artefacts texte
(transcription, OCR, texte extrait ou Preuve textuelle) et les images (Frames
ou Preuves image). Les binaires vidéo, audio et document bruts sont exclus :
aucun Provider local ne les déclare supportés. Il peut être remplacé par un ou
plusieurs `--reference '<json>'` : une Référence explicite vérifiée est retenue
comme telle, y compris lorsqu'elle ne ferait pas partie de la sélection
automatique. Les Dérivés antérieurs ne sont jamais ajoutés implicitement aux
entrées.

Une relance explicite ajoute `--retry-of <job_id>`. Scriptor vérifie que le Job
précédent concerne la même Capture et la même Recette, puis conserve ce lien
dans le nouveau Job. Chaque tentative garde un `job_id` distinct et chaque
succès publie un nouveau `derive_id`.

La Policy `safe-local@1` interdit les appels distants, autorise le Provider
local intégré `scriptor-local-derive` et les Recettes suivantes :

- `structured-summary`
- `proven-claims`
- `checklist`
- `markdown-note`
- `sourced-answer`
- `knowledge-card`

## Interface du Provider

Scriptor lance son Provider local intégré dans un processus séparé ainsi :

```console
scriptor _local-derive --request <request.json> --output <artifact>
```

Avant l'appel, Scriptor écrit `context.json` dans le répertoire du Dérivé. Ce
manifeste versionné contient la Recette, le budget effectif (`duration_limit_secs`
et `disk_byte_limit`), les Références retenues avec leur rôle et motif de
sélection, ainsi que les candidats exclus et leur motif. Les rôles actuels sont
`caption`, `transcription`, `visible-text` et `visual-frame`. Les motifs de
sélection sont notamment `source-context`, `coverage`, `new-ocr-text` et
`explicit-selection`; les exclusions de binaire brut sont tracées comme
`raw-video-unsupported`, `raw-audio-unsupported` ou
`raw-document-unsupported`.

La demande JSON contient `version`, un objet `context` (Référence et chemin du
manifeste), la `recipe` typée, les `parameters` demandés et les `inputs`. Chaque
entrée fournit sa Référence vérifiée, son MIME et le chemin local de l'artefact.
Le Provider ne reçoit que les chemins des artefacts retenus. Le Provider écrit
au chemin `--output` une sortie Recipe JSON versionnée, puis écrit sur stdout
une réponse JSON. La sortie contient `format_version: 1`, le nom `recipe` et
une liste `claims`. Un claim porte son `kind` (`summary`, `important-idea`,
`visible-text`, `visual-observation` ou `uncertainty`), son texte et une liste
non vide de `citations`. Chaque Citation est une Référence complète : son
`capture_id` doit appartenir au corpus transmis et sa Référence doit correspondre
exactement à un artefact retenu. Une sortie sans claim reste valide, mais aucun
claim ne peut être non sourcé.

Pour `knowledge-card`, le Provider produit à la place un `knowledge_core` et
aucun `claims`. Son `coverage` liste exactement chaque Artefact transmis ou
exclu du Contexte, avec son état (`examined`, `unusable` ou `excluded`) et son
motif. Ses `statements` sont des Énoncés atomiques `attributed-declaration`,
`observation`, `attributed-recommendation`, `interpretation` ou `uncertainty`.
Chaque Énoncé non-incertitude porte un Ancrage de preuve vers une Référence
transmise. Une Incertitude déclare une `limitation` concrète et peut être
ancrée lorsqu'un Artefact concerné existe. Une `interpretation` liste les
identifiants de ses `premises`.

```json
{
  "format_version": 1,
  "recipe": "knowledge-card",
  "knowledge_core": {
    "coverage": [{
      "reference": {
        "capture_id": "capture-example",
        "artifact_id": "extraction-transcription",
        "sha256": "...",
        "locator": null
      },
      "state": "examined",
      "reason": "local-text-extraction"
    }],
    "statements": [{
      "id": "statement-1",
      "kind": "attributed-declaration",
      "text": "La Source indique : L'entretien présente le projet.",
      "anchors": [{ "reference": { "capture_id": "capture-example", "artifact_id": "extraction-transcription", "sha256": "...", "locator": null } }]
    }]
  }
}
```

```json
{
  "format_version": 1,
  "recipe": "structured-summary",
  "claims": [
    {
      "kind": "summary",
      "text": "L'entretien présente le projet.",
      "citations": [
        {
          "capture_id": "capture-example",
          "artifact_id": "extraction-transcription",
          "sha256": "...",
          "locator": null
        }
      ]
    }
  ]
}
```

La réponse stdout doit annoncer `mime: "application/json"` :

```json
{
  "mime": "application/json",
  "effective_parameters": {
    "style": "concise"
  }
}
```

Un code de sortie non nul échoue le Job avec la Capacité correspondant à la
Recette. Scriptor borne la durée, l'espace disque et les sorties diagnostiques,
calcule la version du Provider depuis le hash de son binaire, puis publie le
Dérivé dans `captures/<capture_id>/derivatives/<derive_id>/`. Le manifeste du
Dérivé et l'événement de publication référencent aussi `context.json`, qui est
lisible comme tout autre artefact par sa Référence.

Une sortie Recipe mal formée ou d'une version/Recette incohérente échoue avec
`invalid_recipe_output`. Une Citation vide, hors corpus ou absente de la
sélection échoue avec `invalid_recipe_citation`; aucun Dérivé n'est alors
publié.

Les paramètres demandés et effectifs suivent un schéma fermé. Les champs texte
autorisés sont `language`, `model`, `model_sha256` et `style`. `max_tokens` et
`seed` sont des entiers positifs ; `temperature` et `top_p` sont des nombres.
Tout autre champ, objet imbriqué ou type est refusé avant persistance. Ce schéma
exclut notamment secrets, jetons, mots de passe, cookies et autorisations.

Le binaire livré `scriptor` contient le Provider local
`scriptor-local-derive`. Il produit `knowledge-card` sans modèle ni appel
distant. Il transforme le texte local sélectionné en Déclarations attribuées
extractives et publie une Incertitude explicite pour une entrée sans texte
exploitable. Pour préserver le contrat générique des autres Recettes, il
retourne une liste de claims vide et versionnée.
