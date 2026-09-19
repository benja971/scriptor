# Concentrer les Captures derrière un seam de Référentiel privé

## Status

accepted

Un module privé de Référentiel possède les Captures et leurs artefacts vérifiés : chemins, verrous, Manifest, ledger, Dérivés, résolution d'artefact, hash et lecture bornée. Read, Dérivé et Publication traversent ce même seam d'intégrité.

## Consequences

Jobs et Policy restent hors du Référentiel. L'Index de recherche ne possède aucune Capture : il ne consomme que des vues de Capture et demeure une projection reconstruisible. Le format sur disque, le Contrat agent, l'unique adapter filesystem et l'absence de trait Rust public restent inchangés.
