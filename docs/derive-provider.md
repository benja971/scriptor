# Provider local de Dérivés

Le Contrat agent crée un Dérivé avec une sélection explicite :

```console
scriptor derive <capture_id> \
  --recipe markdown-note \
  --provider scriptor-local-derive \
  --policy safe-local-derive@1 \
  --whole-capture
```

`--whole-capture` sélectionne les Preuves et Extractions de la Capture initiale.
Il peut être remplacé par un ou plusieurs `--reference '<json>'`. Les Dérivés
antérieurs ne sont jamais ajoutés implicitement aux entrées.

La Policy `safe-local-derive@1` interdit les appels distants, autorise uniquement
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

Le moteur local derrière `scriptor-local-derive` n'est pas distribué par ce
projet. Il doit respecter cette interface et être présent dans `PATH`.
