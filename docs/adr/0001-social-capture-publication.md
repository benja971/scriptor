# Isoler la publication sociale ordonnée derrière un seam privé

## Status

accepted

Instagram et LinkedIn sont deux adapters de Source sociale : chacun garde la Policy, l'acquisition et le parsing propres à la plateforme. Un module deep privé de `SocialAcquisition` publie ensuite l'observation normalisée en Capture, avec Preuve de métadonnées, caption, médias ordonnés, Capabilities, provenance, annulation et état `partial`. Le traitement vidéo reste le module commun aux médias locaux et sociaux.

## Consequences

Le Contrat agent et les fichiers de Capture restent inchangés. Aucun adapter social spéculatif ni interface Rust publique n'est créé : un nouveau Provider social fournit seulement son adapter, puis réutilise le même seam de publication.
