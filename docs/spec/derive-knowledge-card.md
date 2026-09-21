# Spec : Fiche de connaissance sourcée multimodale

## Problem Statement

Scriptor conserve déjà des Captures locales et Web avec leurs Preuves,
Extractions, Artefacts, hashes et provenance. Un Dérivé peut être publié de
façon immuable, mais aucun moteur sémantique n'est livré : les Recettes
actuelles ne sont que des noms partageant une liste générique de claims.

L'utilisateur accumule notamment une veille UI/UX destinée à améliorer Kodo,
mais ce problème dépasse ce domaine : un post social, une page, un document ou
une vidéo restent difficiles à réemployer lorsqu'ils ne sont qu'archivés. Il
faut pouvoir transformer explicitement une Capture en connaissances
structurées, retrouvables par un Agent et vérifiables par un humain, sans
confondre le contenu d'une Source avec une vérité établie par Scriptor.

Un carrousel social rend le manque concret : sa caption et ses images sont
conservées, mais sans un Dérivé multimodal il faut relire toutes les slides
pour retrouver une pratique, une recommandation ou une nuance. Le premier
produit ne doit ni se limiter à ce type de Source, ni devenir un système de
fact-checking, de réponse à une question ou de recommandation autonome.

## Solution

Scriptor ajoute la Recette locale explicite `knowledge-card`. Elle publie une
Fiche de connaissance sourcée JSON depuis une Capture unique et une sélection
vérifiée de ses Artefacts. La Fiche contient un Noyau de connaissance stable :
sa Couverture du Dérivé et des Énoncés attribués atomiques, typés et reliés à
leurs Ancrages de preuve.

La Recette traite les modalités plutôt que les adaptateurs de Source. Elle
exploite les Extractions textuelles et les Artefacts image disponibles. Une
vidéo contribue par ses Extractions de transcription et ses Frames ; son
binaire brut n'est pas transmis au moteur. Les images statiques, dont les
slides sociales, peuvent recevoir l'OCR local nécessaire pour rendre leur
texte exploitable comme Extraction. Tout Artefact exclu, illisible ou non
traité est rendu visible dans la Couverture du Dérivé ou dans une Incertitude.

Le résultat est consommable directement par un Agent et inspectable par un
humain. Il sert ensuite de matière à une décision ou à une application dans un
projet, comme Kodo, mais ne propose pas lui-même une modification du projet.

## User Stories

1. En tant qu'utilisateur qui capture un post, je veux demander explicitement une Fiche de connaissance sourcée, afin de rendre le post réutilisable plutôt que simplement archivé.
2. En tant qu'Agent, je veux lire une Fiche JSON stable, afin de réemployer ses connaissances sans analyser une prose libre ou tous les Artefacts originaux.
3. En tant qu'humain, je veux relire la même Fiche que l'Agent, afin de contrôler les informations qui seront réemployées.
4. En tant qu'utilisateur de veille UI/UX, je veux retrouver les pratiques extraites d'un post, afin de les examiner lorsque je travaille sur un écran de Kodo.
5. En tant qu'utilisateur, je veux que la même Recette fonctionne à partir d'une page, d'un document, d'un post social ou d'une vidéo, afin que le type de Source ne dicte pas mon organisation de connaissances.
6. En tant qu'utilisateur, je veux que les captions, Markdown, textes PDF, transcriptions et OCR participent à la Fiche, afin que le texte disponible soit exploité sans être confondu avec une Preuve brute.
7. En tant qu'utilisateur, je veux que les slides, screenshots et Frames puissent contribuer à la Fiche, afin que les informations visuelles ne soient pas perdues.
8. En tant qu'utilisateur, je veux que le texte des images statiques puisse être extrait localement, y compris pour des slides sociales, afin de retrouver et réemployer les informations qu'elles portent.
9. En tant qu'utilisateur, je veux qu'une vidéo contribue par sa transcription et ses Frames, afin d'exploiter sa parole et son contenu visuel sans transmettre le fichier vidéo brut.
10. En tant qu'utilisateur, je veux voir quels Artefacts ont été examinés, exclus ou n'ont pas pu être exploités, afin de connaître les limites de la Fiche.
11. En tant qu'Agent, je veux recevoir des Déclarations attribuées, afin de distinguer ce que la Source affirme de ce que Scriptor établit.
12. En tant qu'Agent, je veux recevoir des Observations, afin de distinguer ce qui est directement présent dans un Artefact de toute interprétation.
13. En tant qu'Agent, je veux recevoir des Recommandations attribuées, afin de réemployer les conseils d'une Source sans les présenter comme des conseils de Scriptor.
14. En tant qu'Agent, je veux recevoir des Interprétations avec leurs prémisses, afin de pouvoir séparer un raisonnement du Dérivé des déclarations de la Source.
15. En tant qu'Agent, je veux recevoir des Incertitudes explicites, afin de ne pas transformer un Artefact illisible, absent ou ambigu en information inventée.
16. En tant qu'humain, je veux que chaque Énoncé attribué pointe vers au moins un Artefact pertinent, afin de pouvoir revenir au matériau conservé.
17. En tant qu'utilisateur, je veux que les Frames et les pages qui possèdent déjà un repère naturel conservent leur timestamp ou leur page, afin d'inspecter rapidement le bon matériau.
18. En tant qu'utilisateur local-first, je veux qu'aucune Fiche ne soit produite, ni aucun coût engagé, sans une commande `derive` explicite et une Policy qui l'autorise.
19. En tant qu'utilisateur local-first, je veux que le moteur et ses paramètres effectifs restent versionnés dans le Dérivé, afin de reproduire ou comparer une dérivation.
20. En tant qu'Agent, je veux pouvoir retrouver une Fiche publiée dans l'Index de recherche existant, afin de retrouver une pratique ou un concept au moment de l'utiliser.
21. En tant qu'utilisateur, je veux que plusieurs tentatives créent plusieurs Dérivés immuables, afin de comparer des résultats sans modifier une connaissance déjà publiée.
22. En tant qu'utilisateur, je veux qu'un échec d'OCR ou de moteur reste un état de Job structuré, afin de savoir ce qui manque sans publier une Fiche incomplète comme un succès.
23. En tant qu'utilisateur, je veux que le Noyau de connaissance reste indépendant d'un domaine comme l'UI/UX, afin qu'il serve aussi à l'apprentissage, aux articles techniques ou à d'autres usages.
24. En tant qu'utilisateur, je veux que le Noyau de connaissance puisse plus tard être organisé par un Profil de dérivation local, afin que des besoins métier récurrents n'exigent pas de modifier le coeur de Scriptor.
25. En tant qu'utilisateur de Kodo, je veux qu'un Agent puisse utiliser une Fiche de veille avec le contexte réel du projet, afin de proposer ensuite une amélioration contextualisée sans que le Dérivé n'invente ce contexte.

## Implementation Decisions

- La première capacité s'appelle `knowledge-card`. Elle remplace le rôle de premier flux sémantique précédemment attribué à la synthèse structurée, sans modifier les invariants d'ADR-0003 concernant la sélection de Contexte d'inférence, les budgets, les Artefacts transmis et la publication immuable.
- La sortie de `knowledge-card` est une Fiche de connaissance sourcée JSON versionnée. Elle contient un Noyau de connaissance et ne réutilise pas le conteneur générique de claims des Recettes actuelles.
- Le Noyau de connaissance expose la Couverture du Dérivé et une collection d'Énoncés attribués atomiques. Un Énoncé porte un identifiant stable dans la Fiche, son type, son texte, ses Ancrages de preuve et, pour une Interprétation, ses prémisses par identifiant d'Énoncé.
- Les seuls types du premier jalon sont Déclaration attribuée, Observation, Recommandation attribuée, Interprétation et Incertitude. Une citation directe est une forme d'Ancrage de preuve, pas un type d'Énoncé. Aucun type ne signifie que Scriptor a validé la vérité externe de la Source.
- Tout Énoncé autre qu'une Incertitude doit porter au moins un Ancrage de preuve vers un Artefact effectivement sélectionné dans le Contexte d'inférence. Une Incertitude porte la limite concrète qui empêche d'établir sa proposition et référence l'Artefact concerné lorsqu'il existe.
- L'Ancrage minimal du premier jalon réutilise une Référence d'Artefact et le meilleur Locator déjà conservé, comme une page PDF, une Frame horodatée ou une slide ordonnée. Les extraits textuels avec offsets, les sélecteurs de citation et les régions image par Énoncé restent une extension ultérieure.
- La Couverture du Dérivé déclare les Artefacts réellement examinés, exclus ou non exploitables et le motif correspondant. Une Fiche ne prétend jamais couvrir un Artefact exclu ou illisible.
- Le constructeur de Contexte demeure multimodal et borné : textes et images sélectionnés sont transmis, tandis que les binaires vidéo, audio et document bruts restent exclus. Les Dérivés antérieurs ne deviennent jamais des entrées implicites.
- Les images statiques sont traitées comme une modalité transverse. L'OCR local est appliqué de façon homogène aux images pertinentes, notamment aux médias sociaux et aux Frames, et publie une Extraction avec sa provenance. Une image peut cependant rester utile comme Observation même sans texte OCR fiable.
- Le moteur local `scriptor-local-derive` est livré comme Provider de la Recette. Il ne réalise aucun appel distant et retourne une Fiche conforme au contrat versionné ; Scriptor refuse toute sortie, Référence ou type non conforme avant publication.
- Le Contrat agent conserve le cycle de vie asynchrone existant de `derive`, le snapshot de Policy, la validation des paramètres non secrets, l'immuabilité, les reprises explicites, la lecture bornée et l'indexation des Dérivés publiés.
- Un Profil de dérivation est réservé comme extension : il organisera une projection métier versionnée à partir du même Noyau de connaissance, sans remplacer ce Noyau ni contourner la provenance. Le premier jalon ne charge ni n'exécute de Profil utilisateur.
- Une vérification externe est un flux distinct sur un corpus de Sources supplémentaire. Elle ne fait pas partie de `knowledge-card` et ne modifie pas rétroactivement les Énoncés attribués publiés.

## Testing Decisions

- Le seul seam de test est le Contrat agent CLI JSON, de la création de Capture à `derive`, l'attente du Job, puis l'inspection, la lecture et la recherche du Dérivé. Les tests observent les commandes, JSON et Artefacts publiés, jamais les modules internes.
- Les tests d'intégration utilisent un environnement XDG isolé, des fixtures locales pour le texte, les images, les documents, les médias sociaux et les vidéos, ainsi qu'un double contrôlé du Provider local. Ils réutilisent le prior art déjà présent pour les Jobs asynchrones et la publication de Dérivé.
- Un test de Fiche multimodale vérifie qu'une Capture réunissant caption, OCR de slide, transcription et Frame produit des Énoncés de plusieurs types, tous reliés à des Artefacts sélectionnés, avec une Couverture complète et sans binaire brut dans le Contexte d'inférence.
- Un test de carrousel social vérifie que les médias image ordonnés reçoivent le même chemin OCR local que les images locales, que le texte OCR devient une Extraction recherchable et qu'une Fiche peut l'utiliser sans perdre la provenance de la slide.
- Un test de vidéo vérifie que la Fiche peut s'appuyer sur une transcription et une Frame horodatée, tandis que le média vidéo brut reste exclu du Contexte d'inférence.
- Des tests de contrat refusent une sortie sans type autorisé, un Énoncé sans Ancrage requis, un Ancrage hors sélection, une prémisse d'Interprétation absente, une Couverture incohérente ou un Locator incompatible avec l'Artefact référencé.
- Des tests vérifient qu'un Artefact absent, illisible ou dont l'OCR échoue produit une Incertitude ou une exclusion déclarée, et non une Déclaration, Recommandation ou Observation inventée.
- Des tests vérifient que la Fiche publiée reste immuable sur relance, lisible par Référence, indexée par la recherche existante et non publiée lorsqu'un Job est annulé ou échoue.
- Les tests vérifient que la Recette et le Provider restent locaux et explicitement autorisés par Policy, sans appel distant ni traitement déclenché implicitement.

## Out of Scope

- Réponse à une question particulière, y compris l'actuelle Recette de réponse sourcée.
- Vérification externe, verdict vrai ou faux, résolution de conflit entre Sources, collecte d'un corpus de fact-checking et suivi de pistes de vérification.
- Suggestion directe d'une modification dans Kodo ou dans tout autre projet sans recevoir le contexte de ce projet.
- Chargement, exécution ou partage de Profils de dérivation utilisateur et projections métier personnalisées. Le Noyau de connaissance est seulement conçu pour les supporter ultérieurement.
- JSON de sortie totalement libre qui remplace le Noyau de connaissance ou ses Ancrages de preuve.
- OCR parfait, citation textuelle par offsets, sélection par extrait exact, surlignage de région par Énoncé et interface graphique d'inspection.
- Analyse directe de binaires vidéo, audio ou document dans le moteur de Dérivé.
- Dérivation automatique après Capture, dérivation par lot, orchestration d'une inbox de veille, appels distants, coûts implicites ou action métier implicite.
- Agrégation de plusieurs Captures, raisonnement sur des Dérivés précédents ou recherche sémantique/vectorielle nouvelle.
- Choix d'un modèle particulier au-delà de l'exigence d'un Provider local versionné et explicitement autorisé.

## Further Notes

- La valeur immédiate est de transformer une Capture difficile à relire, par exemple un post UI/UX, en pratiques, observations et limites réutilisables par un Agent ou un humain. La Fiche n'est pas le produit final de veille quotidienne : les Profils et l'orchestration de volume sont les suites prévues, construites sur le même Noyau de connaissance.
- ADR-0003 reste la décision de référence pour le Contexte d'inférence multimodal traçable. Cette spec raffine son premier flux de sortie et n'autorise aucun affaiblissement de ses contraintes de sélection explicite, de budget, de provenance et d'absence de transmission brute.
- Les termes métier normatifs sont ceux de `CONTEXT.md`, notamment Capture, Artefact, Preuve, Extraction, Dérivé, Fiche de connaissance sourcée, Noyau de connaissance, Couverture du Dérivé, Profil de dérivation, Énoncé attribué et Ancrage de preuve.
