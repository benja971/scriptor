# Recherche : persistance et index des Captures

Recherche menée le 12 septembre 2026 pour éclairer la décision Wayfinder sur le
contrat de persistance, l'index de recherche et les providers de Scriptor. Les
sources ci-dessous sont les dépôts officiels des projets étudiés.

## Conclusion

Conserver une séparation nette : le **référentiel de Captures** possède la
Capture, sa provenance, son original et ses artefacts dérivés ; un **index de
recherche** est une projection dérivée, reconstructible, qui ne possède pas la
vérité métier. Les providers de traitement (OCR, transcription, LLM,
embeddings) constituent un troisième seam.

Ainsi, la première implémentation peut être un référentiel sur dossier et aucun
index, ou un index local à côté. Plus tard, un référentiel SQLite/PostgreSQL et
un index plein texte ou vectoriel peuvent évoluer indépendamment. Ne pas faire
d'une abstraction de blobs ou d'un moteur RAG le contrat de persistance de la
Capture.

## RagForge Core

[RagForge](https://github.com/LuciformResearch/ragforge-core/blob/88f0853988958fd98d9825101011ca1c716dc2fe/README.md)
est un serveur MCP de mémoire pour Claude, centré sur un graphe Neo4j obligatoire.
Il ingère notamment PDF, DOCX, XLSX, Markdown, images OCR et pages web, puis
propose recherche vectorielle, entités et relations. C'est une référence utile
pour les capacités envisageables, pas un socle léger à reprendre pour Scriptor :
son installation requiert Node, Docker et Neo4j.

Sa principale leçon pour Scriptor est la valeur des métadonnées transverses :
le contrat de ses parseurs distingue identité, état de traitement, provenance,
version de contenu, hash, provider et modèle d'embedding
([`parser-types.ts`](https://github.com/LuciformResearch/ragforge-core/blob/88f0853988958fd98d9825101011ca1c716dc2fe/src/ingestion/parser-types.ts)).
Un manifest de Capture devrait conserver les équivalents pertinents, sans
adopter son schéma de graphe.

RagForge expose bien des interfaces de provider :

- [`OCRProvider`](https://github.com/LuciformResearch/ragforge-core/blob/88f0853988958fd98d9825101011ca1c716dc2fe/src/runtime/ocr/types.ts) sépare l'extraction de texte image de Gemini, Claude, Replicate et Tesseract.
- [`EmbeddingProviderInterface`](https://github.com/LuciformResearch/ragforge-core/blob/88f0853988958fd98d9825101011ca1c716dc2fe/src/runtime/embedding/embedding-provider.ts) offre le même principe pour les embeddings, dont une implémentation locale Ollama.

En revanche, son abstraction de stockage
[`INeo4jClient`](https://github.com/LuciformResearch/ragforge-core/blob/88f0853988958fd98d9825101011ca1c716dc2fe/src/database/neo4j-client.ts)
reste un client Cypher : les autres modules et la recherche vectorielle sont
fortement couplés à Neo4j. Ce n'est pas un contrat réutilisable pour alterner
dossier, SQLite et PostgreSQL.

## Lucivy

[Lucivy](https://github.com/L-Defraiteur/lucivy/blob/main/README.md) est une
bibliothèque d'index plein texte BM25, en processus, pour Rust et plusieurs
bindings. Elle vise en particulier le code et la documentation technique, et se
présente comme le complément lexical d'une base vectorielle. Ce n'est ni un
référentiel de contenus ni une interface utilisateur commune.

Son contrat [`BlobStore`](https://github.com/L-Defraiteur/lucivy/blob/main/lucistore/src/blob_store.rs#L21-L64)
est une bonne inspiration de découplage physique : il sait charger, sauver,
supprimer et lister des blobs d'index nommés. Les adaptateurs de stockage de
shards confirment que les blobs sont la vérité de l'index et qu'un cache mmap
local peut être jetable
([`shard_storage.rs`](https://github.com/L-Defraiteur/lucivy/blob/main/lucistore/src/shard_storage.rs#L12-L113),
[architecture](https://github.com/L-Defraiteur/lucivy/blob/main/ARCHITECTURE.md#sharding-storage-formats)).

Lucivy offre ultérieurement une recherche lexicale utile à Scriptor : champs
structurés, substring, fuzzy, regex, booléens, filtres non textuels et
highlights avec offsets
([référence de requêtes](https://github.com/L-Defraiteur/lucivy/blob/main/README.md#query-reference)).
Ces offsets peuvent renvoyer vers le texte extrait d'une Capture, donc préserver
la traçabilité vers la preuve.

Mais `BlobStore` ne porte que des octets nommés : ni identité de Capture, ni
provenance, ni original, ni version sémantique, ni écriture atomique de
l'agrégat. Il ne doit donc pas devenir le contrat de persistance Scriptor.
Lucivy est un candidat d'implémentation de `SearchIndex` plus tard, jamais de
`CaptureRepository`.

## Contrats à décider dans la spec

La spec devrait fixer des responsabilités, sans figer une base de données :

- `CaptureRepository` : créer, lire, lister et versionner une Capture et son
  manifest de provenance.
- `ArtifactStore` : conserver et ouvrir l'original, les extractions objectives
  et les transformations explicites. Le répertoire de Capture est sa première
  implémentation.
- `SearchIndex` : indexer les contenus publiés par le référentiel, rechercher,
  puis pouvoir être intégralement reconstruit depuis lui.
- providers de traitement : exécuter OCR, transcription, LLM et embeddings,
  avec identité du provider, du modèle et des paramètres enregistrés dans le
  manifest pour rendre chaque dérivé explicable et reproductible.

Le référentiel doit rester la frontière de cohérence : une Capture publiée doit
être complète même si l'index échoue ou est absent. Si une synchronisation
atomique référentiel-index devient nécessaire, elle se résout au niveau d'une
unité de travail ou d'une file d'indexation, pas en confondant les deux contrats.
