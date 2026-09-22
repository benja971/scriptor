# Validation Instagram réel - Knowledge Card

Exécution du 2026-09-22, depuis la collection `saas` explicitement ouverte par
son propriétaire. Les 36 permaliens publics visibles ont été extraits dans le
DOM rendu, sans cookie, endpoint privé ni URL de collection persistée.

## Commande

```console
scriptor knowledge batch --source-file /tmp/instagram-saas-links.txt \
  --capture-policy safe-web@1 \
  --derive-policy safe-local@1 \
  --recipe knowledge-card \
  --provider scriptor-local-derive
```

Job parent : `job-3170862-1790035728307829927`.

## Mesure

| Mesure | Résultat |
| --- | --- |
| Sources publiques visibles | 36 |
| État du lot | `partial` |
| Captures et Fiches disponibles | 2 |
| Échecs de Capture | 34 |
| Cause observée | `capture_failed`: `Instagram metadata media acquisition failed` |

Les deux Fiches publiées sont issues de Captures déjà présentes et réutilisées
par la Policy `reuse` :

- `https://www.instagram.com/deluxewebsite/p/DdZFTqltIoU/`
  - Capture `capture-591341-1789917360682467868`
  - Fiche `derive-3172143-1790035757096391346`
- `https://www.instagram.com/noahelhadedy/p/DdJzEakF7Ux/`
  - Capture `capture-591834-1789917375112731992`
  - Fiche `derive-3173734-1790035789456469209`

## Recherche

Les requêtes suivantes ont toutes retourné des Énoncés actifs avec leur
Référence de Fiche et une Source minimale :

| Requête | Résultat utile |
| --- | --- |
| `navigation` | navigation et conversion, source `deluxewebsite` |
| `design tools` | 7 outils de design IA, source `noahelhadedy` |
| `conversion` | teardown CRO, source `deluxewebsite` |

Les deux Fiches ont été lues par leur Référence exacte, avec `offset: 0` et
lecture bornée à 8192 octets. Elles déclarent les Preuves, la caption examinée
et les limites de couverture visuelle/OCR.

## Correction et rerun

Le Provider conservait déjà les métadonnées et la caption mais annulait la
Capture lorsque les médias publics étaient indisponibles. Le correctif publie
alors la Capture `partial` et le lot dérive sa caption non vide et vérifiable.
Les autres Captures partielles restent non dérivables.

Le rerun du 2026-09-22, `job-3215913-1790036607614966731`, atteint :

| Mesure | Résultat |
| --- | --- |
| Sources | 36 |
| État du lot | `succeeded` |
| Fiches publiées ou réutilisées | 36 |
| Échecs | 0 |

Les requêtes `responsive design` et `database tables` retournent des Énoncés
attribués de Fiches nouvelles, respectivement les sources `developer_lokesh`
et `roadsidecoder`.

## Verdict

Le contrat de lot, la Capture publique Instagram, la Fiche et la recherche
locale sont opérationnels sur toute la collection visible. La collection ne
contenait que 36 sources, en dessous de l'objectif initial de 50 à 100, mais la
validation réelle du flux complet est positive sur son corpus disponible.
