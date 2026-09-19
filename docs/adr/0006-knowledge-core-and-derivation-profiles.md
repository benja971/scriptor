# Séparer le Noyau de connaissance des Profils de dérivation

## Status

accepted

Les Dérivés de connaissance publient toujours un Noyau de connaissance stable composé d'Énoncés attribués, de leur Couverture et de leurs Ancrages de preuve. Des Profils de dérivation JSON versionnés pourront ultérieurement organiser ce Noyau pour un usage métier, comme la veille UI/UX, sans le remplacer ni contourner sa provenance. Ce choix évite à la fois un unique format figé impropre aux usages récurrents et des JSON totalement libres qui rendraient les Dérivés incompatibles entre Agents et incontrôlables par un humain.

## Consequences

Le premier jalon livre le Noyau de connaissance mais pas le moteur de Profils. Une projection spécialisée future doit rester rattachée à la version du Profil qui l'a produite et préserver les Énoncés attribués et Ancrages de preuve publiés.
