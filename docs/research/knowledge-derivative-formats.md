# Dérivé de connaissance vérifiable : formats et cadrage de tâche

## Question examinée

Pour une Capture unique, locale et figée, quel objet structuré aide à la fois un
Agent à réemployer l'information et un humain à contrôler ce réemploi ? Cette
note ne choisit ni moteur ni implémentation. Les contenus capturés sont traités
comme des **allégations non fiables**, pas comme des faits sur le monde.

## Ce que les sources établissent

- Le modèle W3C Web Annotation sépare une annotation de sa ressource ciblée et
  définit des sélecteurs pour désigner un fragment précis. Un `TextQuoteSelector`
  porte le texte exact, et peut porter un préfixe et un suffixe ; un
  `TextPositionSelector` porte des positions début/fin comptées en points de code
  Unicode. Le standard note que la position seule est fragile quand la ressource
  évolue. [Web Annotation Data Model](https://www.w3.org/TR/annotation-model/)
- Le standard W3C Media Fragments adresse un intervalle temporel demi-ouvert et
  une région rectangulaire `xywh` d'un média, en pixels ou en pourcentage.
  [Media Fragments URI 1.0](https://www.w3.org/TR/media-frags/)
- PROV-DM distingue explicitement les entités, activités, agents, l'attribution
  et la dérivation ; il recommande d'exprimer une relation de provenance
  spécifique plutôt qu'une influence générique quand elle est connue.
  [PROV-DM](https://www.w3.org/TR/prov-dm/)
- Dans FEVER, une allégation est évaluée comme `Supported`, `Refuted` ou
  `NotEnoughInfo`, et les jugements supportés ou réfutés sont accompagnés des
  phrases nécessaires comme preuves. Cela établit l'intérêt d'un état
  « information insuffisante » distinct de vrai/faux dans un cadre de
  vérification textuelle, pas une taxonomie universelle pour Scriptor.
  [Thorne et al., 2018](https://aclanthology.org/N18-1074/)
- ERASER qualifie les extraits de preuve de « rationales » et évalue à la fois
  leur alignement avec des annotations humaines et leur fidélité à la sortie.
  C'est un argument expérimental pour conserver les preuves avec le résultat,
  non une preuve qu'un extrait seul rend une interprétation correcte.
  [DeYoung et al., 2020](https://aclanthology.org/2020.acl-main.408/)
- Les recommandations de nanopublications séparent une assertion, sa
  provenance et ses informations de publication. Elles admettent notamment des
  hypothèses, résultats négatifs et opinions : une unité publiable ne se réduit
  donc pas à un fait validé. [Nanopublication Guidelines](https://nanopub.net/guidelines/working_draft/)
- Les directives FactBank distinguent la polarité d'un événement et le degré
  d'engagement épistémique qu'une Source lui attribue. Cette distinction ne
  vérifie pas l'événement : elle décrit ce que la Source présente comme certain,
  probable, possible ou indéterminé. [FactBank 1.0 Annotation Guidelines](https://catalog.ldc.upenn.edu/docs/LDC2009T23/annotationGuidelines.pdf)
- FActScore décompose une génération en faits atomiques, puis évalue séparément
  leur soutien par une source de connaissance. C'est un précédent de recherche
  pour éviter un jugement binaire sur une fiche entière, non une taxonomie
  métier imposée. [Min et al., 2023](https://aclanthology.org/2023.emnlp-main.741/)
- JSON Schema permet de déclarer les propriétés requises d'un objet plutôt que
  de laisser la présence des champs implicite. C'est une option standard pour
  valider une sortie structurée, sans imposer le contenu métier de cette sortie.
  [JSON Schema](https://json-schema.org/understanding-json-schema/reference/object)

## Recommandation de produit déduite

La première fiche ne devrait pas prétendre produire des « faits ». Elle devrait
produire des **items atomiques attribués à une Capture**, chacun avec un type
épistémique visible et au moins un ancrage de preuve si l'item affirme quelque
chose. « Atomique » signifie une seule proposition contrôlable : ne pas mêler
dans le même item ce que l'auteur dit, ce qui est visible et ce que le Dérivé en
déduit.

Une forme de dossier, et non un texte libre, paraît la plus robuste : les champs
requis rendent l'absence de preuve détectable et chaque item peut être filtré par
un Agent ou relu indépendamment. C'est une recommandation de conception,
inspirée des mécanismes ci-dessus, pas une norme ni un schéma prêt à implémenter.

| Partie du dossier | Rôle recommandé | Règle de contrôle humain |
| --- | --- | --- |
| Identité et portée | Identifiant de publication, Capture/artefacts sélectionnés, versions, langue, couverture et limites connues. | Dire précisément quels artefacts ont été lus et ceux qui ne l'ont pas été. |
| Item | `id`, proposition courte, type épistémique, attribution, preuves et état de support. | Un item important sans preuve est soit refusé, soit marqué insuffisant. |
| Preuve | Identifiant de l'artefact immuable, hash/révision disponible, type d'ancre et sélecteur. | Un lien doit ouvrir le même artefact et le même fragment, pas seulement la publication entière. |
| Limite ou incertitude | Cause concrète : artefact manquant, texte illisible, ambiguïté, contradiction interne ou information hors Capture. | Ne pas transformer une absence de preuve en réfutation. |
| Question ouverte | Question que la Capture ne permet pas de trancher, avec éventuellement les items concernés. | Ne jamais inventer une réponse pour compléter la fiche. |

### Types d'items à ne pas confondre

Cette séparation est une recommandation, aucune des sources ci-dessus ne la
prescrit telle quelle.

| Type | Formulation admise | Ce que cela ne signifie pas |
| --- | --- | --- |
| `citation_directe` | Le texte exact de la Source, sous forme de citation. | Que le propos cité est vrai. |
| `reformulation_attribuée` | « L'auteur affirme que X. » | Une citation littérale ou que X est vrai. |
| `observation` | « La slide 4 montre/contient visiblement X. » | L'intention, la causalité ou la validité de X. |
| `recommandation_attribuée` | « La Source recommande de faire X, sous la condition Y. » | Une recommandation de Scriptor ou une instruction à exécuter. |
| `interprétation` | « À partir de A et B, le Dérivé interprète X. » | Une déclaration directe de la Source. Les items A et B doivent être reliés. |
| `incertitude` | « La Capture ne permet pas d'établir X, parce que Y. » | Que non-X est vrai. |
| `question_ouverte` | « Quelle est la preuve externe de X ? » | Une lacune que le moteur peut silencieusement combler. |

L'item `interprétation` est le plus risqué. Il doit être rare, explicitement
marqué et porter ses prémisses citées. Une reformulation, même fidèle, n'est pas
une citation directe : la catégorie séparée évite de présenter la compression
comme les mots de l'auteur. Une recommandation issue d'un post reste toujours
attribuée à son auteur.

### Ancrages de preuve recommandés

Conserver le type d'artefact et une ancre adaptée, plutôt qu'un unique champ
« citation » :

| Artefact | Ancre minimale | Renforcement utile |
| --- | --- | --- |
| Caption, transcription, OCR ou autre texte d'Extraction | `artifact_id`, intervalle `[start,end)` sur le texte normalisé, extrait exact. | Préfixe/suffixe et règle de normalisation, pour vérifier que l'intervalle vise toujours le bon passage. |
| Image ou slide | `artifact_id` ou hash du média, indice de slide, rectangle `xywh` dans les dimensions originales. | Image dérivée ou transcription/OCR explicitement identifiée si elle existe. |
| Audio ou vidéo | `artifact_id`, intervalle temporel `[start,end)`. | Lien vers la transcription correspondante ou frame sélectionnée, sans confondre les deux preuves. |
| Plusieurs artefacts | Plusieurs ancres nommées avec leur relation à l'item. | Distinguer « soutient directement », « montre », et « prémisse d'interprétation ». |

L'intervalle texte et l'extrait exact servent des objectifs différents : le
premier permet une adresse mécanique, le second une relecture et une résistance
aux décalages. L'approche reprend les deux sélecteurs W3C, sans obliger Scriptor
à sérialiser du JSON-LD.

## Cadrage de la demande de dérivation

Un bon cadrage n'est pas « résume cette Source ». Il force le résultat à rester
dans le périmètre des artefacts et à exprimer son statut. Formulation de travail
à discuter :

> À partir uniquement des artefacts sélectionnés de cette Capture, produis une
> fiche structurée. Découpe les propositions vérifiables. Pour chacune, indique
> si elle est déclarée par la Source, observée dans un artefact,
> recommandée par la Source, ou interprétée à partir d'items cités. Attache les
> ancres exactes. Ne juge pas la vérité externe de la Source, ne donne aucun
> conseil propre, et marque explicitement ce qui n'est pas établi ou ne peut pas
> être lu. Retourne seulement les champs prévus.

Trois garde-fous méritent d'être explicites :

1. « Seulement les artefacts sélectionnés » empêche une connaissance externe
   non traçable de devenir une pseudo-preuve.
2. « Ne juge pas la vérité externe » évite de confondre attribution et
   fact-checking. Celui-ci demanderait un corpus de vérification distinct.
3. « Marque l'incertitude » ne veut pas dire énumérer tout ce qui manque. Ne
   créer un item `non_établi` que pour une proposition examinée et utile, avec la
   limite précise qui empêche de l'établir.

## Cas mental : carrousel Instagram de 14 images et caption

Le dossier peut ancrer une déclaration dans la caption et une observation dans
la slide 7. Sans OCR, une phrase seulement visible dans une image ne possède pas
d'ancre textuelle vérifiable : la preuve honnête est alors le média de la slide,
éventuellement sa région rectangulaire. La fiche doit annoncer que les 14 images
ont été examinées visuellement, mais que le texte inclus dans les images n'est
ni recherché ni cité comme texte. Elle ne doit pas simuler une précision de
citation que la Capture ne possède pas.

Cette limite rend le premier résultat encore utile : il peut attribuer la
caption, inventorier les observations clés par slide et exposer ce qui reste
inaccessible. En revanche, il ne permet pas encore de promettre une extraction
exhaustive des consignes inscrites dans le carrousel. Ajouter plus tard une OCR
locale créerait un nouvel artefact de preuve, pas une raison de réécrire la
fiche déjà publiée.

## Limites de cette recherche

Les standards W3C décrivent comment cibler et provenancer des ressources, pas
comment décider qu'une proposition est atomique ou vraie. FEVER et ERASER sont
des travaux d'évaluation sur des corpus textuels, pas des prescriptions produit
ni des validations pour images sociales. Les catégories proposées, la forme de
la fiche et le cadrage de tâche sont donc des recommandations à arbitrer avec le
produit, notamment avant de les inscrire dans le vocabulaire du domaine.
