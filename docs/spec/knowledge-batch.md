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
avec une Policy locale explicite lorsque la Capture a réussi. Le bilan du parent
conserve chaque Job enfant, Capture, Dérivé, Fiche et erreur.

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

- Commande : `knowledge batch --source <url>` répétable, `--capture-policy`,
  `--derive-policy`, `--provider scriptor-local-derive` et `--recipe
  knowledge-card` obligatoires.
- Le parent est un Job distinct. Il termine `partial` dès qu'une Source échoue,
  est refusée ou ne publie pas de Capture utilisable. Une annulation du parent
  le laisse `cancelled`.
- Le parent lance les enfants séquentiellement. La file de Jobs existante porte
  seule la concurrence, les budgets et l'annulation de chaque enfant.
- Une Capture partielle avec `capture_id` ne déclenche pas de Dérivé dans ce
  premier jalon. Son état reste visible dans le bilan.
- Le bilan contient, dans ordre d'entrée, Source, Job Capture, `capture_id`,
  Job Dérivé, `derive_id`, Référence Fiche et erreur structurée éventuelle.
- Les Sources sont dédupliquées avant création du parent puis validées une à
  une. Aucune découverte, URL Saved, cookie ou session de navigateur n'est
  acceptée.
- Une reprise explicite est un nouveau lot avec seulement Sources choisies par
  l'utilisateur. Le premier jalon ne relance jamais automatiquement un échec.

## Testing Decisions

- Tests CLI JSON couvrent succès complet, Capture refusée ou partielle, Dérivé
  échoué, annulation parent, ordre de bilan et absence de Dérivé après Capture
  partielle.
- Les tests utilisent Sources locales et Providers contrôlés dans un XDG isolé.
- Un test lit la Fiche par Référence retournée dans le bilan et vérifie son
  appartenance à la Capture correspondante.

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
