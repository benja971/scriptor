# Spec : Recherche de connaissance locale

## Problem Statement

Scriptor sait désormais publier une Fiche de connaissance sourcée avec des
Énoncés attribués, mais l'Index de recherche existant répond à une question
technique : dans quels Artefacts un texte apparaît-il ? Il mélange Preuves,
OCR, Frames, captions, transcriptions et Dérivés. Un Agent qui cherche une
information utile doit donc trier du matériau brut avant de pouvoir réemployer
une pratique ou une limite.

L'utilisateur veut retrouver toutes les informations déjà extraites sur un
sujet, sans se limiter à une réponse générée ni à une Fiche entière. Il doit
recevoir les Énoncés pertinents, ordonnés de façon compréhensible, puis pouvoir
remonter à leur Fiche et à leur Source seulement si cela devient nécessaire.
La recherche doit rester locale, vérifiable, paginée et fidèle à
l'immuabilité du Référentiel.

## Solution

Scriptor ajoute la capacité `knowledge search`. Elle recherche dans une
projection locale dédiée des Énoncés attribués issus des Fiches
`knowledge-card` actives. Un Résultat de connaissance expose l'Énoncé entier,
son type, une Référence vers sa Fiche et un résumé minimal de sa Source. La
lecture bornée existante de la Fiche donne ensuite accès à la Couverture et aux
Ancrages de preuve complets.

Une Requête de connaissance est un texte littéral. Tous ses mots doivent
correspondre après normalisation de la casse et des accents. Le classement
utilise une Pertinence lexicale locale, avec un bonus lorsque les mots de la
requête apparaissent comme une phrase exacte ; la date ne départage que les
résultats de même pertinence. La capacité retourne toutes les informations
actives correspondantes par pages stables, sans exposer de score numérique ni
prétendre répondre à la question de l'utilisateur.

## User Stories

1. En tant qu'utilisateur, je veux rechercher une information dans mes Fiches de connaissance, afin de ne pas parcourir des fichiers bruts pour la retrouver.
2. En tant qu'Agent, je veux appeler une capacité distincte de `capture search`, afin de ne pas confondre recherche d'information et recherche d'Artefact.
3. En tant qu'Agent, je veux recevoir un Énoncé attribué par Résultat, afin de savoir quelle information correspond à ma recherche.
4. En tant qu'utilisateur, je veux recevoir toutes les informations correspondantes par pagination, afin qu'une première page ne limite pas ce que je peux apprendre.
5. En tant qu'Agent, je veux que les mots d'une Requête à plusieurs mots soient tous présents, afin d'éviter les résultats ne traitant que partiellement mon sujet.
6. En tant qu'utilisateur, je veux que les mots voisins dans le même ordre fassent mieux classer un Résultat, afin que les formulations directement liées à ma recherche apparaissent d'abord.
7. En tant qu'utilisateur francophone, je veux que `ecole`, `école` et `ÉCOLE` retrouvent la même information, afin que la saisie d'accents ou de majuscules ne masque pas une connaissance.
8. En tant qu'Agent, je veux que la ponctuation sépare les mots recherchés, afin que des formulations comme `navigation-mobile` restent retrouvables avec `navigation mobile`.
9. En tant qu'utilisateur, je veux que le texte saisi reste littéral, afin qu'aucun opérateur de requête caché ne modifie silencieusement les résultats.
10. En tant qu'Agent, je veux que le classement s'appuie sur la correspondance textuelle, afin que la recherche ne prétende pas mesurer la vérité, l'autorité ou la qualité générale d'une Source.
11. En tant qu'utilisateur, je veux que la récence ne domine pas une information mieux liée aux mots recherchés, afin qu'une information ancienne mais pertinente reste accessible avant une information récente moins liée.
12. En tant qu'utilisateur, je veux voir le type de chaque Énoncé, afin de distinguer une déclaration, une observation, une recommandation, une interprétation ou une incertitude.
13. En tant qu'Agent, je veux recevoir les Incertitudes parmi les Résultats, afin de savoir qu'une information ne peut pas être établie depuis une Capture.
14. En tant qu'utilisateur, je veux lire le texte entier de l'Énoncé sans score ni extrait artificiel, afin que le résultat reste directement réemployable.
15. En tant qu'Agent, je veux recevoir la Référence de la Fiche correspondante, afin de demander plus de détails sans nouvelle Capture ni recherche implicite.
16. En tant qu'utilisateur, je veux recevoir seulement un résumé minimal de la Source, afin que la provenance reste disponible sans encombrer les Résultats.
17. En tant qu'Agent, je veux lire ensuite les Ancrages de preuve et la Couverture depuis la Fiche, afin de contrôler un Résultat lorsque cela importe.
18. En tant qu'utilisateur, je veux qu'une Requête vide soit refusée explicitement, afin de détecter une erreur d'appel plutôt que de confondre une exploration globale avec une recherche.
19. En tant qu'Agent, je veux qu'une Requête valide sans correspondance retourne une page vide, afin de distinguer l'absence d'information d'une erreur de contrat.
20. En tant qu'utilisateur, je veux reprendre une recherche avec un curseur opaque, afin de lire un grand ensemble de Résultats sans chargement arbitraire.
21. En tant qu'Agent, je veux que les pages d'une même recherche restent stables, afin de ne pas répéter ou manquer des informations entre deux appels.
22. En tant qu'utilisateur, je veux que la dernière Fiche régénérée depuis le même État observé de Source devienne active, afin que la recherche ne répète pas une même connaissance issue d'une dérivation identique.
23. En tant qu'utilisateur, je veux conserver et pouvoir lire une Fiche supersédée, afin de conserver l'historique, les Références et les preuves publiés.
24. En tant qu'utilisateur, je veux que deux États observés différents d'un même post restent tous deux recherchables, afin qu'une modification réelle de la Source ne soit pas cachée.
25. En tant qu'utilisateur, je veux que deux posts différents restent deux Sources distinctes même lorsqu'ils se ressemblent, afin de conserver les informations et désaccords entre Sources.
26. En tant qu'utilisateur de réseau social, je veux que des variantes de Locator d'un même post soient reconnues comme le même post lorsque la plateforme fournit son identifiant, afin qu'une variante d'URL ne crée pas une connaissance active dupliquée.
27. En tant qu'utilisateur local-first, je veux que la Recherche de connaissance n'effectue aucun appel réseau ni appel à un Provider, afin que la consultation de connaissances n'engage aucun coût ou traitement implicite.
28. En tant qu'Agent, je veux recevoir une erreur structurée lorsque la projection de recherche est absente ou dégradée, afin de ne pas prendre un sous-ensemble silencieux pour une réponse exhaustive.
29. En tant qu'utilisateur, je veux pouvoir reconstruire la projection de recherche depuis les Captures et Dérivés publiés, afin que l'Index reste reconstructible et ne possède jamais mes connaissances.
30. En tant qu'utilisateur, je veux que la recherche reste utilisable par un harness d'IA, afin que celui-ci puisse sélectionner des informations, répondre avec elles et rechercher de nouveau si nécessaire sans que Scriptor génère lui-même une réponse.

## Implementation Decisions

- Le Contrat agent ajoute `knowledge search <query>` avec les options `--cursor` et `--limit`. Cette capacité est distincte de `capture search`, qui conserve son rôle de recherche technique dans les Artefacts publiés.
- Une Requête de connaissance doit produire au moins un token après analyse. Une Requête vide ou ne produisant aucun token retourne une erreur structurée `invalid_request`. Une Requête valide sans Énoncé correspondant retourne une page de Résultats vide.
- Le premier jalon accepte seulement une Requête littérale. Il ne reconnaît ni opérateurs booléens, ni guillemets de phrase, ni exclusions, ni recherche préfixe, ni syntaxe de champ. Un harness d'IA reformule une question en termes de recherche avant l'appel.
- Chaque token de Requête est obligatoire. La recherche compare uniquement le texte canonique des Énoncés attribués, jamais les URLs, comptes, hashes, Couverture, Ancrages, paramètres de Provider ou texte des Preuves.
- L'analyseur `knowledge_v1` applique une segmentation Unicode sur ponctuation et espaces, la normalisation en minuscules et le repli des accents vers leur équivalent non accentué. Le même analyseur est utilisé lors de l'indexation et de l'analyse de Requête. Stemming français, suppression de mots vides, correction orthographique, synonymes, fuzzy matching et n-grams ne font pas partie du premier jalon.
- La Pertinence lexicale est le score BM25 natif sur le seul champ de texte des Énoncés. Une phrase exacte formée de plusieurs tokens ajoute un bonus de classement sans enlever les Résultats où les mêmes mots sont séparés. Aucun score numérique n'est retourné par le Contrat agent.
- L'ordre total est : Pertinence lexicale décroissante, date de publication distante décroissante lorsqu'elle existe, date de publication de Capture décroissante sinon, puis Référence de Fiche et identifiant d'Énoncé par ordre stable. La date ne constitue pas une prime de pertinence.
- Une page contient vingt Résultats par défaut et au plus cent. Le curseur est opaque, lié à la Requête normalisée et à un snapshot de la projection. Un curseur d'une autre Requête ou d'un autre snapshot est refusé par erreur structurée. La capacité ne tronque jamais silencieusement l'ensemble correspondant.
- Un Résultat de connaissance contient le texte complet de l'Énoncé, son type, sa Référence de Fiche, son identifiant stable dans cette Fiche et un résumé minimal de Source, notamment son Locator. Le score, les Ancrages, la Couverture et les Preuves n'apparaissent pas par défaut ; l'Agent les obtient avec la lecture bornée existante de la Fiche référencée.
- Les cinq types d'Énoncé existants sont tous indexés et retournés. Le type est toujours visible, y compris pour une Incertitude. Aucun type n'obtient de bonus, de malus ou de filtre au premier jalon.
- La projection dédiée contient un enregistrement par Énoncé de Fiche `knowledge-card` active et de version de format supportée. Elle exclut seulement un Énoncé attribué composé uniquement de hashtags ou de mentions, car ce marqueur social n'est pas une information recherchable. La Fiche et son Ancrage restent lisibles. Elle stocke les données strictement nécessaires à la pagination et au Résultat, sans dupliquer la Fiche complète ni les binaires.
- Une Fiche active est déterminée par la Supersession de Fiche d'ADR-0007. La publication d'une nouvelle Fiche de même Recette et même État observé de Source rend la Fiche la plus récente active et laisse la précédente lisible hors recherche par défaut. Deux États observés distincts restent actifs.
- L'État observé de Source compare les preuves de contenu conservées, sans tenir compte de la date d'acquisition, de la version de Provider ou du texte produit par une Fiche. Pour une publication sociale, l'Identité logique de Source est `plateforme + post_id` lorsque ces métadonnées sont présentes. Son état compare les Extractions et médias conservés, pas la preuve de métadonnées qui peut varier avec le Locator demandé. Sinon il réutilise l'identité de Source et les Preuves existantes.
- La publication d'une Fiche met à jour la projection de façon atomique. Une reconstruction explicite recrée la projection depuis les Captures, leurs manifests et leurs Dérivés publiés. Une version de format ou d'analyseur inconnue rend l'Index dégradé par erreur structurée, sans rendre les Fiches illisibles ni tenter une conversion implicite.
- La capacité ne réalise aucun scan de secours du Référentiel lorsque l'Index est indisponible. Elle retourne une erreur structurée compatible avec les états existants `index_unavailable` et `index_degraded` et laisse l'Agent demander une reconstruction explicite.
- La capacité reste locale. Elle ne crée ni Capture, ni Job, ni Dérivé, n'appelle ni réseau ni Provider, et ne modifie pas les Fiches, Ancrages, Preuves ou Extractions existants.

## Testing Decisions

- Le seul seam de test est le Contrat agent CLI JSON. Les tests créent des Captures et Dérivés avec des fixtures locales dans un environnement XDG isolé, attendent les Jobs, appellent `knowledge search`, puis lisent la Fiche avec la Référence retournée. Ils n'observent jamais les modules, structures ou requêtes du moteur d'Index.
- Les tests réutilisent le prior art des tests CLI de Capture, Dérivé, index, curseurs et lecture bornée. Les doubles du Provider local permettent de publier des Fiches déterministes contenant les cinq types d'Énoncé, des Ancrages et des États observés contrôlés.
- Un test vérifie qu'une recherche ne retourne que des Énoncés de Fiches `knowledge-card` actives, jamais les captions, OCR, Frames, transcriptions ou autres Artefacts que `capture search` peut encore retrouver.
- Un test vérifie qu'un Résultat contient le texte entier, son type, sa Référence de Fiche, son identifiant dans cette Fiche et un Locator de Source, sans score ni Ancrages complets, puis que la lecture de la Fiche fournit ces détails.
- Des tests vérifient que les cinq types, dont Incertitude, sont recherchables et que leur type visible empêche de les confondre avec une vérité externe validée par Scriptor.
- Des tests vérifient les Requêtes à un et plusieurs mots, l'exigence de tous les mots, la normalisation de casse et d'accents, la segmentation par ponctuation et le bonus de phrase exacte par rapport à des mots séparés comparables.
- Des tests vérifient que synonymes, faute voisine, opérateur booléen, exclusion, guillemets et syntaxe de champ ne reçoivent aucun sens spécial au premier jalon.
- Des tests vérifient qu'une Requête vide ou sans token est rejetée, tandis qu'une Requête valide sans correspondance retourne une page vide.
- Des tests vérifient la pagination à plus de vingt et plus de cent Résultats, l'absence de saut ou de répétition, la stabilité d'un snapshot et le refus d'un curseur associé à une autre Requête ou à une projection reconstruite.
- Des tests vérifient l'ordre par Pertinence lexicale, puis les départages par date et identifiants stables, sans exposer la valeur du score dans la sortie JSON.
- Des tests vérifient qu'une redérivation de même État observé et même Recette conserve les deux Fiches lisibles mais ne recherche que la plus récente, alors qu'un changement de Preuve de contenu garde les deux Fiches actives.
- Des tests de Capture sociale vérifient que deux Locators variants d'un même `plateforme + post_id` et même État observé n'ajoutent pas deux Fiches actives, tandis que deux posts distincts ou un post dont les preuves changent restent deux résultats distincts.
- Des tests vérifient qu'une publication et une reconstruction indexent les mêmes Fiches actives, que la projection est atomique et qu'un Index absent, dégradé ou de format inconnu retourne une erreur structurée sans acquisition, Provider, scan silencieux ni altération de Fiche.

## Out of Scope

- Répondre à une question, générer une synthèse, fusionner des Fiches, résoudre un conflit entre Sources ou déduire une connaissance inter-Capture.
- Recherche sémantique, embeddings, vecteurs, reranking par modèle, évaluation de pertinence par IA, score normalisé ou pourcentage de qualité.
- Recherche de Preuves, OCR, Frames, captions, transcriptions ou fichiers bruts à la place des Énoncés. `capture search` conserve ce rôle technique.
- Filtres par compte, plateforme, type d'Énoncé, date, Recette, Provider ou version de format, facettes, suggestions de requêtes, recherche par compte social et browsing de comptes.
- Syntaxe avancée de Requête, synonymes, stemming, mots vides, correction de fautes, fuzzy matching, préfixes, proximité relâchée ou expansion de Requête.
- Interface graphique, feed de connaissances récentes, résultats groupés par Fiche, résultat de score visible ou gestion manuelle de ranking.
- Nouvelle Capture automatique, vérification d'un post distant, nettoyage destructif des Fiches supersédées, fusion d'historique ou correction rétroactive des Preuves.
- Profils de dérivation exécutables, nouveaux formats métier de Fiche et prise en charge automatique de versions de format inconnues.

## Further Notes

- Cette capacité est une mémoire recherchable pour un harness d'IA, non un moteur de question-réponse. Le harness choisit les Résultats, lit les Fiches nécessaires et peut relancer une Recherche plus précise.
- ADR-0007 fixe la conservation immuable des Fiches supersédées et leur visibilité par défaut. Cette spec la rend exploitable dans la projection de Recherche de connaissance.
- La recherche préalable sur le schéma lexical, la provenance et les heuristiques de classement est conservée dans `docs/research/knowledge-card-lexical-search.md` et `docs/research/knowledge-search-ranking-heuristics.md`.
- Les termes métier normatifs sont ceux de `CONTEXT.md`, notamment Recherche de connaissance, Requête de connaissance, Pertinence lexicale, Résultat de connaissance, Identité logique de Source, État observé de Source et Supersession de Fiche.
