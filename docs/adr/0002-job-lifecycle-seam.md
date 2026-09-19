# Concentrer le cycle de vie des Jobs derrière un seam privé

## Status

accepted

Un module privé `job` possède l'état persistant, les verrous, l'admission de concurrence, les transitions, la réconciliation et la consultation d'annulation des Jobs. Capture, Continue et Dérivé gardent leur orchestration; toutes leurs transitions terminales passent par ce module.

## Consequences

La publication d'un Dérivé reste propriétaire de son staging, de sa promotion et de son ledger de Capture. Sa coordination avec le module Job conserve l'ordre de verrous `Job` puis ledger de Capture afin que l'annulation reste atomique vis-à-vis de la publication. Le Contrat agent, ses états publics, l'annulation et le retry restent inchangés.
