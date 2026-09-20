# Recherche : recherche lexicale locale des Fiches de connaissance

Recherche menée le 20 septembre 2026 pour préparer une spécification de
recherche dédiée aux Fiches `knowledge-card`. Elle examine une recherche
lexicale locale, reconstruisible et respectueuse de la provenance. Elle ne
choisit ni bibliothèque, ni format de stockage, ni commande CLI finale.

## Périmètre et conclusion

La bonne unité interne de recherche est l'**Énoncé attribué**, et non la Fiche
entière. Une Fiche reste le document immuable, lisible et vérifiable. L'index
en projette un enregistrement par Énoncé, avec la Référence de la Fiche, le
type de l'Énoncé et ses Ancrages de preuve. Ainsi une requête remonte la
proposition précise qui correspond, sans transformer le résultat en assertion
de Scriptor ou perdre le chemin vers le matériau source.

Cette granularité est cohérente avec le Noyau de connaissance actuel : un
Énoncé est déjà atomique, typé, identifié dans sa Fiche et muni d'Ancrages. À
l'inverse, une Fiche peut contenir plusieurs propositions indépendantes. La
traiter comme un unique document ferait remonter la bonne Fiche, mais sans
dire quel Énoncé est pertinent ni quelle Preuve le soutient.

Le premier contrat devrait rester strictement lexical et mono-Capture : pas
d'embeddings, pas de recherche sémantique, pas de fusion ou de raisonnement
entre Captures. Un résultat appartient à une seule Fiche, elle-même dérivée
d'une seule Capture. Cela respecte explicitement le hors-scope de
`derive-knowledge-card.md` et ne change pas la recherche technique existante
`capture search`.

## Pourquoi séparer la recherche de connaissance de `capture search`

`capture search` répond à « dans quel Artefact ce texte apparaît-il ? ». Son
index actuel projette un document par Artefact texte et mélange volontairement
preuve, extraction, OCR, transcription et Dérivé. C'est le bon outil pour
inspecter le Référentiel ou diagnostiquer une Capture.

Une recherche de connaissance répond à « quelle proposition publiée est utile
sur ce sujet, et d'où vient-elle ? ». Elle ne doit donc pas faire de l'OCR brut
un concurrent d'un Énoncé. Elle peut renvoyer plusieurs Énoncés de plusieurs
Fiches, mais chacun doit rester relié à **une** Capture et à ses propres
preuves.

Illustration non normative :

```text
Recherche de connaissance : "navigation mobile"
Filtre : compte = instagram:deluxewebsite

1. Recommandation attribuée
   « La Source recommande de limiter les destinations principales... »
   Fiche : derive-…-content, énoncé : statement-2
   Preuve : caption de la Capture et slide 3
   Source : https://www.instagram.com/p/… , @deluxewebsite

2. Observation
   « La slide présente une barre inférieure à cinq destinations... »
   Fiche : derive-…-content, énoncé : statement-4
   Preuve : slide 3
```

Le résultat n'affirme pas que la recommandation est universellement correcte.
Son type dit ce que la Fiche attribue à la Source, et les Ancrages permettent
de la contrôler.

## Contrat de projection recommandé

Un index lexical sépare les champs analysés pour la recherche libre de ceux
qui ont une valeur exacte. Cette distinction est structurelle dans les moteurs
plein texte : Tantivy distingue par schéma les champs `TEXT` tokenisés, les
champs `STRING` non tokenisés, les champs indexés, stockés et rapides. Un
champ stocké est récupérable avec le document de résultat, tandis qu'un champ
indexé sert à la recherche. [Schéma Tantivy](https://docs.rs/tantivy/latest/tantivy/schema/)

| Élément projeté par Énoncé | Recherche libre | Filtre ou facette exacte | Renvoyé dans le hit | Raison |
| --- | --- | --- | --- | --- |
| `statement_text` | Oui, champ principal | Non | Oui, texte canonique | C'est l'unité utile lue par l'utilisateur. |
| `statement_kind` | Non | Oui | Oui | Distingue Déclaration, Observation, Recommandation, Interprétation et Incertitude. |
| Référence de la Fiche | Non | Oui, interne | Oui, complète | Pointe vers le Dérivé immuable à relire et à vérifier. |
| `statement_id` | Non | Oui, interne | Oui | Désigne sans ambiguïté l'Énoncé dans cette Fiche. |
| `proof_anchors` | Non | Non | Oui, complets | Permet de passer directement du hit au matériau soutenant l'Énoncé. |
| `recipe_kind` et version de format | Non | Oui | Oui si nécessaire | Évite de mélanger ultérieurement des formats incompatibles. |
| Capture et Source d'origine | Non | `capture_id` si nécessaire | Oui, Locator et identité | Situe le résultat et conserve la remontée vers la Capture. |
| Plateforme et compte source | Non par défaut | Oui | Oui, format d'affichage préservé | Permet de retrouver les connaissances issues d'un même compte sans surclasser artificiellement son texte. |
| Dates | Non | Oui | Oui, avec sémantique explicite | Distingue la date de Capture de la date de publication distante lorsqu'elle est réellement connue. |

La Couverture, les paramètres du Provider, les logs, le JSON brut des
Artefacts et le texte des Preuves ne sont pas des champs de recherche de cette
capacité. Ils demeurent lisibles depuis le Référentiel après ouverture de la
Fiche ou de l'Ancrage. Indexer leur prose créerait exactement le bruit que la
capacité cherche à éviter.

Un compte doit être représenté comme une valeur structurée, au minimum
`plateforme:identifiant-normalisé`, avec une valeur d'affichage capturée telle
que `@deluxewebsite`. Un simple `@handle` n'est pas globalement unique entre
plateformes. Le filtre ne doit jamais prétendre qu'un handle observé est une
identité vérifiée ou l'auteur réel du contenu.

## Contenu d'un résultat utile et vérifiable

Le résultat minimal doit porter le texte canonique de l'Énoncé, son type,
l'identité de la Fiche et ses Ancrages complets. La Référence de la Fiche et
celle de chaque Ancrage conservent `capture_id`, `artifact_id`, hash et Locator
lorsqu'il existe, conformément au contrat actuel de Référence. Un Agent peut
ainsi lire la Fiche ou l'Artefact pointé sans effectuer une nouvelle Capture.

Un extrait surligné est facultatif et ne remplace jamais `statement_text` ni
un Ancrage. Les moteurs peuvent produire ce type d'aperçu en repérant les
termes de la requête et leur contexte, comme le fait le
[générateur de snippets de Tantivy](https://docs.rs/tantivy/latest/tantivy/snippet/).
Dans Scriptor, cet aperçu doit rester une aide de lecture : il n'est ni une
nouvelle preuve, ni un Locator textuel plus précis que ceux effectivement
publiés par la Fiche.

La provenance doit rester une projection, jamais une vérité détenue par
l'index. PROV-DM décrit précisément la provenance comme l'information sur les
entités, activités et agents impliqués dans la production d'une donnée, et
traite une dérivation comme le lien entre entités utilisé par une activité.
[PROV-DM](https://www.w3.org/TR/prov-dm/) conforte donc le chemin : Fiche
produite par Dérivation, Fiche référencée par le hit, Ancrage vers Artefact,
Artefact conservé par Capture. La suppression ou la reconstruction de l'index
ne doit changer aucune de ces relations.

## Recherche, classement et filtres

La requête libre doit porter par défaut sur `statement_text` seulement. Ni
l'URL, ni le handle social, ni un hash ne doivent augmenter la pertinence d'un
Énoncé parce qu'ils contiennent par hasard le même token. Ils restent des
champs de filtre exacts ou de recherche explicite séparée.

Le classement initial peut être le score lexical natif sur ce champ, sans
prime implicite de récence, de compte, de type d'Énoncé ou de Provider. Tantivy
documente que `TopDocs.order_by_score()` trie les résultats par score de
similarité BM25 décroissant. [Collecte et score Tantivy](https://docs.rs/tantivy/latest/tantivy/collector/struct.TopDocs.html)
Ce choix est explicable : le score exprime la correspondance textuelle, pas la
fiabilité externe d'une Source.

La spec devra aussi fixer un départage déterministe après le score, par exemple
Référence de Fiche puis `statement_id`, afin que les pages et curseurs restent
stables pour un même snapshot de l'index. La similarité ne doit pas servir de
substitut à cette règle de stabilité.

Les filtres sont des paramètres structurés du contrat, pas des fragments
fabriqués dans la chaîne de requête libre. Le parseur de Tantivy accepte en
effet des champs ciblés (`field:term`) et sa sémantique dépend de ses champs
par défaut et du mode conjonctif ou disjonctif. [QueryParser Tantivy](https://docs.rs/tantivy/latest/tantivy/query/struct.QueryParser.html)
Les filtres programmatiques doivent donc être séparés de la syntaxe saisie par
l'utilisateur, notamment pour les types, comptes, dates, Recettes et versions.
La documentation d'Apache Lucene formule le même principe : une valeur de
filtre contrôlée par l'application doit être construite comme clause de requête
structurée, non injectée dans une chaîne destinée au parseur.
[Lucene Query Parser Syntax](https://lucene.apache.org/core/3_0_3/queryparsersyntax.pdf)

Les facettes ou compteurs sont utiles seulement pour des vocabulaires bornés,
par exemple les cinq types d'Énoncé ou les plateformes. Tantivy indique que
son collecteur de facettes suppose un nombre de facettes très inférieur au
nombre de documents. [FacetCollector Tantivy](https://docs.rs/tantivy/latest/tantivy/collector/struct.FacetCollector.html)
Le compte social, dont le nombre peut croître sans borne, doit d'abord être un
filtre exact, non une liste de facettes à retourner automatiquement.

## Conséquences pour la future spécification Scriptor

Les décisions de contrat suivantes sont suffisamment étayées pour être fixées
avant toute implémentation :

- capacité distincte de `capture search`, dont le corpus est exclusivement les
  Fiches `knowledge-card` publiées ;
- un hit par Énoncé, qui renvoie la Fiche, l'Énoncé, son type, ses Ancrages et
  le contexte de Source capturé ;
- recherche libre sur le texte de l'Énoncé, filtres typés séparés pour les
  données d'identité et de provenance ;
- classement lexical seulement, suivi d'un départage stable documenté ;
- index entièrement reconstruisible à partir des Dérivés immuables et des
  manifests de Capture, qui reste indisponible ou dégradé sans invalider les
  Fiches ;
- aucune déduction nouvelle, validation externe, agrégation multi-Capture,
  profil de dérivation, embedding ou recherche vectorielle.

Ces choix s'appuient sur les contraintes déjà présentes dans Scriptor : le
Référentiel possède les Captures et Dérivés immuables, l'Index est une
projection reconstructible, une Fiche est mono-Capture et chaque Énoncé porte
déjà un identifiant, un type et des Ancrages validés. La spec devra encore
nommer la capacité, versionner son enveloppe JSON, définir ses erreurs, ses
limites et ses tests de curseur, mais elle n'a pas à réouvrir ces invariants.
