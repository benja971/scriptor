# Provider local de Dérivés

Le Contrat agent crée un Dérivé avec une sélection explicite :

```console
scriptor derive <capture_id> \
  --recipe markdown-note \
  --provider scriptor-local-derive \
  --policy safe-local@1 \
  --whole-capture
```

`--whole-capture` sélectionne les Preuves et Extractions de la Capture initiale.
Il peut être remplacé par un ou plusieurs `--reference '<json>'`. Les Dérivés
antérieurs ne sont jamais ajoutés implicitement aux entrées.

Une relance explicite ajoute `--retry-of <job_id>`. Scriptor vérifie que le Job
précédent concerne la même Capture et la même Recette, puis conserve ce lien
dans le nouveau Job. Chaque tentative garde un `job_id` distinct et chaque
succès publie un nouveau `derive_id`.

La Policy `safe-local@1` interdit les appels distants, autorise
le binaire `scriptor-local-derive` et les Recettes suivantes :

- `structured-summary`
- `proven-claims`
- `checklist`
- `markdown-note`
- `sourced-answer`

## Interface du Provider

Scriptor appelle le Provider ainsi :

```console
scriptor-local-derive --request <request.json> --output <artifact>
```

La demande JSON contient `version`, la `recipe` typée, les `parameters` demandés
et les `inputs`. Chaque entrée fournit sa Référence vérifiée, son MIME et le
chemin local de l'artefact. Le Provider écrit le contenu au chemin `--output`,
puis écrit sur stdout une réponse JSON :

```json
{
  "mime": "text/markdown",
  "effective_parameters": {
    "style": "concise"
  }
}
```

Un code de sortie non nul échoue le Job avec la Capacité correspondant à la
Recette. Scriptor borne la durée, l'espace disque et les sorties diagnostiques,
calcule la version du Provider depuis le hash de son binaire, puis publie le
Dérivé dans `captures/<capture_id>/derivatives/<derive_id>/`.

Les paramètres demandés et effectifs suivent un schéma fermé. Les champs texte
autorisés sont `language`, `model`, `model_sha256` et `style`. `max_tokens` et
`seed` sont des entiers positifs ; `temperature` et `top_p` sont des nombres.
Tout autre champ, objet imbriqué ou type est refusé avant persistance. Ce schéma
exclut notamment secrets, jetons, mots de passe, cookies et autorisations.

Le moteur local derrière `scriptor-local-derive` n'est pas distribué par ce
projet. Il doit respecter cette interface et être présent dans `PATH`.
