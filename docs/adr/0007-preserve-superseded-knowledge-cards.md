# Préserver les Fiches de connaissance supersédées

## Status

accepted

Une nouvelle Fiche `knowledge-card` issue du même État observé de Source et de la même Recette ne supprime ni n'écrase la Fiche publiée auparavant. Elle la supersède automatiquement pour la Recherche de connaissance : la recherche retourne alors la Fiche active la plus récente par défaut, tandis que la Fiche supersédée reste lisible et vérifiable par sa Référence. Une modification du contenu observé crée un nouvel État et laisse les deux Fiches actives. Ce choix préserve l'historique, les preuves et les Références déjà utilisées, sans faire remonter deux fois la même information après une redérivation identique.

## Considered Options

- Écraser la Fiche antérieure : rejeté, car cela détruirait un Dérivé publié et ses Références.
- Retourner systématiquement toutes les Fiches : rejeté pour la recherche par défaut, car une redérivation ou une recapture identique ferait dupliquer les Résultats sans apporter une nouvelle information.

## Consequences

La Supersession de Fiche est déterminée à la publication par même État observé de Source et même Recette, jamais à partir du texte produit par une Fiche ou d'un `statement_id`. La comparaison porte sur les preuves de contenu conservées, pas sur la date d'acquisition, la version du Provider ou le texte dérivé. Pour une publication sociale, l'Identité logique de Source est `plateforme + post_id` plutôt que la variante de Locator saisie. Deux États observés distincts restent actifs, même lorsqu'ils proviennent d'un même post ou que leurs Énoncés se ressemblent. Les capacités de lecture et d'inspection conservent l'accès aux Fiches supersédées.
