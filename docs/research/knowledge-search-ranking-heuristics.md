# Recherche : heuristiques de classement de connaissance

Recherche menée le 20 septembre 2026. Périmètre : recherche locale dans des
Énoncés sourcés individuels. Hors périmètre : vecteurs, embeddings, LLM,
réponse synthétique, fusion multi-Capture et évaluation d'une Source.

## Frontière

Cette capacité retrouve des Énoncés publiés. Elle ne produit pas une réponse
nouvelle. Sa similarité répond seulement à « ce texte correspond-il aux termes
cherchés ? ». Un Agent peut ensuite lire Fiche et Preuves, puis répondre ou
relancer une recherche.

## Heuristiques examinées

### BM25 : oui, base native

BM25 est une fonction établie de recherche documentaire, présentée dans le
papier original *Okapi at TREC-3* de Robertson et al. [Publication originale,
Microsoft Research](https://www.microsoft.com/en-us/research/publication/okapi-at-trec-3/).
Lucene documente ses paramètres `k1` et `b`, par défaut `1.2` et `0.75`.
[BM25Similarity, Lucene](https://lucene.apache.org/core/9_12_2/core/org/apache/lucene/search/similarities/BM25Similarity.html)
Tantivy expose `TopDocs::order_by_score()` comme un ordre décroissant de
similarité BM25. [TopDocs,
Tantivy](https://docs.rs/tantivy/latest/src/tantivy/collector/top_score_collector.rs.html#225-228)

Recommandation : BM25 natif sur `statement_text`. Ne pas réécrire formule ni
régler `k1` ou `b` au premier jalon. Pas de données de jugement pour le faire.

### Tous mots : oui, condition d'éligibilité

Tantivy peut interpréter `happy tax payer` comme `happy AND tax AND payer`
avec `set_conjunction_by_default()`. [QueryParser,
Tantivy](https://docs.rs/tantivy/latest/tantivy/query/struct.QueryParser.html)

Recommandation : chaque token analysé devient `MUST`. Un Énoncé avec un seul
mot ne devient pas résultat pour une requête à plusieurs mots.

### Phrase exacte : oui, bonus simple

Tantivy fournit `PhraseQuery`, qui recherche une séquence de mots et demande
des positions indexées. [PhraseQuery,
Tantivy](https://docs.rs/tantivy/latest/tantivy/query/struct.PhraseQuery.html)
Lucene indique que les correspondances plus exactes sont mieux classées que les
plus lâches. [PhraseQuery,
Lucene](https://lucene.apache.org/core/8_1_1/core/org/apache/lucene/search/PhraseQuery.html)
Une étude sur cinq collections TREC trouve une corrélation forte entre
proximité et pertinence, avec amélioration significative au-dessus de BM25.
[Tao et Zhai, SIGIR 2007](https://timan.cs.illinois.edu/czhai/pub/sigir07-prox.pdf)

Recommandation : pour au moins deux tokens, ajouter phrase exacte comme clause
`SHOULD`, boostée par facteur constant. Tokens restent `MUST`, donc les mots
séparés restent récupérés. Phrase collée gagne avantage. `BoostQuery` ne change
pas corpus trouvé, seulement le score. [BoostQuery,
Tantivy](https://docs.rs/tantivy/latest/tantivy/query/struct.BoostQuery.html)

Commencer avec boost `2.0`, mais le vérifier contre quelques requêtes et Fiches
réelles avant le figer. Pas de `slop`, fenêtre glissante ou proximité non
ordonnée au premier jalon.

### Boost de champ : non

Tantivy sait booster un champ. [QueryParser,
Tantivy](https://docs.rs/tantivy/latest/tantivy/query/struct.QueryParser.html)
Mais le corpus n'a qu'un champ métier recherché, `statement_text`. Booster
URL, compte, type, Ancrages ou Couverture remettrait bruit technique dans
classement.

### Récence : départage, pas prime

Date ne mesure pas similarité textuelle. Une prime ferait monter information
récente mais moins liée aux mots. Tantivy sait ordonner par BM25 ou champ
rapide, mais ne définit pas formule hybride produit. [TopDocs,
Tantivy](https://docs.rs/tantivy/latest/src/tantivy/collector/top_score_collector.rs.html#225-240)

Recommandation : score lexical puis phrase. En égalité seulement : publication
Source décroissante, sinon Capture décroissante. Si produit veut vraiment
« récent d'abord », score ne peut plus être ordre principal.

### Source, compte, type : non

Ils servent lecture détaillée ou debug. Ils ne mesurent pas similarité. Pas de
boost autorité, compte, plateforme, Provider, Recette ou type d'Énoncé. Un
type peut devenir filtre explicite plus tard.

### Normalisation : non

Tantivy définit `Score` comme une `f32` de pertinence relative à requête.
[Score, Tantivy](https://docs.rs/tantivy/latest/tantivy/type.Score.html) BM25
dépend de statistiques de collection, dont fréquence de termes et longueur
moyenne. [Similarity,
Lucene](https://lucene.apache.org/core/9_12_2/core/org/apache/lucene/search/similarities/Similarity.html)

Donc : pas de pourcentage de pertinence. Pas de comparaison de scores entre
requêtes ou après reconstruction Index. Score peut être rendu pour debug, sans
signification de probabilité.

### Stabilité : clé métier

Tantivy départage scores égaux par `DocAddress`. Cette adresse dépend Index,
pas identité métier durable. [Top score collector,
Tantivy](https://docs.rs/tantivy/latest/src/tantivy/collector/top_score_collector.rs.html#26-29)

Recommandation : après score et date, trier par Référence Fiche puis
`statement_id`. Curseur lié snapshot Index et dernière clé. Pagination ne
répète ni saute Énoncé quand scores égaux.

## Règle étroite recommandée

```text
Corpus : statement_text des Fiches knowledge-card publiées.
Éligibilité : tous tokens de requête requis.
Classement : BM25 natif + phrase exacte SHOULD boostée.
Départage : publication Source desc, Capture desc si inconnue,
             Référence Fiche asc, statement_id asc.
Résultat : tous Énoncés éligibles, paginés.
```

Tester facteur de boost avec requêtes réelles : mots séparés, phrase exacte,
terme rare, terme fréquent, accent et pagination. Ce test juge classement,
pas réponse à question.

## Reporté ou rejeté

- vecteurs, embeddings, similarité sémantique, synonymes et expansion requête ;
- réponse générée, fusion Fiches et raisonnement inter-Capture ;
- score autorité, confiance externe, compte ou plateforme ;
- prime de récence mélangée score lexical ;
- fuzzy matching, stemming personnalisé, `slop` et proximité non ordonnée ;
- score normalisé ou pourcentage présenté comme qualité de réponse.
