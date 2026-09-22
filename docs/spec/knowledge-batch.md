# Spec : lot Capture vers Fiche de connaissance

## Problem Statement

Une veille de plusieurs Sources publiques exige aujourd'hui un enchaînement
manuel de Captures, attentes, inspections et Dérivés. Cette procédure reste
vérifiable mais devient fragile dès qu'une Source échoue ou qu'une limite de
concurrence est atteinte.

## Solution

`scriptor knowledge batch` crée un Job parent persistant depuis une liste
explicite de Sources. Pour chaque Source, il lance une Capture avec une Policy
de Capture explicite, attend son état terminal, puis lance `knowledge-card`
avec une Policy locale explicite lorsque la Capture a réussi. Une Capture
Instagram `partial` reste dérivable uniquement si sa caption non vide est une
Extraction vérifiable. Le bilan du parent conserve chaque Job enfant, Capture,
Dérivé, Fiche et erreur.

## User Stories

1. En tant qu'utilisateur, je fournis plusieurs URLs publiques choisies afin
   de les transformer en Fiches sans répéter la procédure manuelle.
2. En tant qu'Agent, je lis un bilan par Source afin de reprendre seulement les
   échecs sans confondre un lot partiel avec un succès complet.
3. En tant qu'utilisateur local-first, je vois les Policies effectives et les
   Références de chaque résultat afin de contrôler le lot.
4. En tant qu'utilisateur, j'annule le parent afin qu'aucune nouvelle Capture
   ou Dérivé enfant ne démarre.

## Implementation Decisions

- Commande : `knowledge batch --source <url>` répétable ou `--source-file
  <chemin>` pour une liste UTF-8 locale, `--capture-policy`,
  `--derive-policy`, `--provider scriptor-local-derive` et `--recipe
  knowledge-card` obligatoires. Les lignes vides et celles qui commencent par
  `#` sont ignorées.
- Le parent est un Job distinct. Il termine `partial` dès qu'une Source échoue,
  est refusée ou ne publie pas de Capture utilisable. Une annulation du parent
  le laisse `cancelled`.
- Le parent lance les enfants séquentiellement. La file de Jobs existante porte
  seule la concurrence, les budgets et l'annulation de chaque enfant.
- Une Capture partielle avec `capture_id` ne déclenche pas de Dérivé, sauf une
  Capture Instagram dont `extraction-caption` est non vide et vérifiable. Ce
  cas préserve une information textuelle sourcée lorsque les médias publics
  sont refusés. Toute autre Capture partielle reste visible sans Dérivé.
- Le bilan contient, dans ordre d'entrée, Source, Job Capture, `capture_id`,
  Job Dérivé, `derive_id`, Référence Fiche et erreur structurée éventuelle.
- Les Sources sont dédupliquées avant création du parent puis validées une à
  une. Les permaliens Instagram publics reconnus (`/p/`, `/reel/`, `/tv/`,
  avec ou sans segment de compte) sont ramenés à leur forme canonique avant
  cette déduplication. Les autres Sources restent inchangées. Aucune
  découverte, URL Saved, cookie ou session de navigateur n'est acceptée.
- Une reprise explicite est un nouveau lot avec seulement Sources choisies par
  l'utilisateur. Le premier jalon ne relance jamais automatiquement un échec.

## Testing Decisions

- Tests CLI JSON couvrent succès complet, Capture refusée ou partielle, Dérivé
  échoué, annulation parent, ordre de bilan, exception Instagram à caption et
  absence de Dérivé après les autres Captures partielles.
- Les tests utilisent Sources locales et Providers contrôlés dans un XDG isolé.
- Un test lit la Fiche par Référence retournée dans le bilan et vérifie son
  appartenance à la Capture correspondante.
- Un test vérifie que deux variantes de permalien Instagram du même post ne
  lancent qu'une seule Capture dans le lot.

## Out of Scope

- Import automatisé d'une collection Instagram connectée, lecture de cookies,
  sélection navigateur, sync périodique et découverte de Sources.
- Agrégation de Fiches, réponse IA, vecteurs, interface graphique, priorité de
  lot et reprise automatique.

## Further Notes

- Cette commande est le seul prérequis produit pour importer manuellement des
  permaliens publics issus d'une collection Instagram, conformément à la
  recherche `instagram-saved-collection-import.md`.
- Elle réutilise Capture, Dérivé, Fiche, Job et Policy du glossaire existant.
