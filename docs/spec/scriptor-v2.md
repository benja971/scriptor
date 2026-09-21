# Spec : Scriptor v2 - Captures multimodales exploitables par Agent

Publiée comme issue GitHub : [benja971/scriptor#30](https://github.com/benja971/scriptor/issues/30).

## Problem Statement

Un Agent ne peut pas exploiter de manière sûre, portable et vérifiable une
vidéo, une page Web dynamique, un PDF, une image ou un document avec le CLI de
transcription v1. Les traitements ne partagent pas un contrat de provenance,
les résultats ne peuvent pas être inspectés ni recherchés de façon bornée, et
les actions asynchrones ne sont pas pilotables par un contrat JSON stable.

L'utilisateur veut une base local-first qui transforme des Sources locales ou
Web en Captures autoportantes. Chaque contenu conservé, extrait ou interprété
doit rester traçable jusqu'à une Preuve. Un Agent doit pouvoir demander,
attendre, vérifier et réutiliser ces résultats sans parser de logs, sans
exécuter les instructions contenues dans les Sources et sans lancer de coûts ou
d'appels distants implicites.

## Solution

Scriptor v2 fournit un Contrat agent JSON pour créer, contrôler, consulter et
transformer des Captures. Une Capture est un dossier portable avec une identité
immuable, son observation initiale, ses Preuves, Extractions, Dérivés et un
ledger append-only. Les Jobs asynchrones sont persistants, versionnent leur
Policy effective et rapportent des états et erreurs structurés.

Les opérations de Capture et de Dérivé exigent une Policy explicite. La Policy
initiale `safe-local@1` limite les ressources, autorise seulement des Providers
locaux et interdit les appels distants par défaut. Le skill agent versionné
guide une séquence explicite de Capture, attente, inspection, sélection de
Recette, Dérivé et vérification bornée.

## User Stories

1. En tant qu'Agent, je veux créer une Capture depuis une Source locale, pour conserver son contenu dans un format portable et vérifiable.
2. En tant qu'Agent, je veux créer une Capture depuis une Source Web publique, pour exploiter une page rendue côté client sans perdre sa provenance.
3. En tant qu'Agent, je veux que toute Capture ait un identifiant local immuable, pour pouvoir la référencer sans ambiguïté.
4. En tant qu'Agent, je veux distinguer l'identité d'une Source de celle d'une Capture, pour conserver des observations différentes d'une même URL.
5. En tant qu'Agent, je veux conserver une Preuve canonique et les hashes des fichiers, pour vérifier les contenus sans index externe.
6. En tant qu'Agent, je veux recevoir des Extractions séparées des Preuves, pour distinguer les faits observés des transformations ultérieures.
7. En tant qu'Agent, je veux que chaque Dérivé porte sa Recette, son Provider et ses entrées, pour rendre une synthèse ou une réponse sourcée vérifiable.
8. En tant qu'Agent, je veux que les Dérivés soient immuables, pour comparer plusieurs tentatives sans perdre les résultats précédents.
9. En tant qu'Agent, je veux qu'une page Web conserve le DOM rendu et une Extraction Markdown sémantique, pour accéder à la fois à la preuve technique et au contenu lisible.
10. En tant qu'Agent, je veux capturer les médias et documents incorporés pertinents, pour ne pas perdre les contenus non textuels d'une page.
11. En tant qu'Agent, je veux que les Sources découvertes soient reliées à leur parent et à leur emplacement, pour préserver le contexte de découverte.
12. En tant qu'Agent, je veux voir les Sources ignorées, les cycles et les échecs de découverte, pour savoir précisément ce qui manque à une Capture.
13. En tant qu'Agent, je veux reprendre explicitement des Découvertes ignorées par budget, pour compléter une Capture sans réécrire son historique.
14. En tant qu'Agent, je veux fournir une Policy explicite à chaque Capture ou Dérivé, pour rendre les choix opérationnels reproductibles.
15. En tant qu'Agent, je veux qu'un Job persistant me donne un état structuré, pour attendre ou diagnostiquer une opération sans parser des logs.
16. En tant qu'Agent, je veux pouvoir annuler un Job et retrouver un Job interrompu, pour maîtriser les traitements longs et les redémarrages.
17. En tant qu'Agent, je veux lister les Captures avec une pagination stable, pour explorer un Référentiel sans charger un volume arbitraire.
18. En tant qu'Agent, je veux rechercher des Captures via un Index reconstruisible, pour trouver du contenu sans rendre l'Index propriétaire des données.
19. En tant qu'Agent, je veux lire un artefact par Référence et plage bornée, pour inspecter un contenu sans afficher accidentellement un binaire ou un fichier énorme.
20. En tant qu'Agent, je veux recevoir des Références vérifiables dans toute réponse, pour les réutiliser dans une Recette ou une réponse sourcée.
21. En tant qu'Agent, je veux que les Sources et leurs instructions soient traitées comme du Contenu non fiable, pour qu'une page ne puisse jamais choisir des actions à ma place.
22. En tant qu'Agent, je veux que les appels réseau et Providers distants soient refusés sans Policy qui les autorise, pour empêcher une exfiltration ou un coût implicite.
23. En tant qu'Agent, je veux que le skill me force à inspecter une Capture avant de choisir une Recette, pour éviter les transformations inutiles ou non demandées.
24. En tant qu'Agent, je veux une restitution finale des identifiants, états, Providers, Références et limites, pour savoir ce qui est réellement acquis et vérifié.
25. En tant qu'opérateur local-first, je veux que Firefox fournisse le premier rendu Web complet et Chromium un repli, pour disposer d'un comportement validé sur NixOS.
26. En tant qu'opérateur local-first, je veux que Lightpanda reste limité au contenu textuel quand cette capacité est suffisante, pour profiter de son fast-path sans prétendre fournir de Preuve visuelle.

## Implementation Decisions

- Une Capture est un dossier autoportant identifié par `capture_id`. Son manifest canonique et versionné décrit l'observation initiale ; un ledger append-only ajoute les résultats ultérieurs. Les Preuves, Extractions et Dérivés sont séparés et chaque fichier déclaré porte son identité, chemin relatif, MIME, hash, taille, date et Provider éventuel.
- Chaque contenu interprété référence la Preuve ou l'Extraction utilisée et un Locator typé quand le support le permet. L'absence de Locator est elle-même déclarée.
- Le Référentiel de Captures sur dossier est la vérité métier. L'Index de recherche local est une projection reconstruisible, déclenchée après publication, qui peut être absent ou en erreur sans invalider une Capture.
- Les transformations explicites utilisent des Recettes typées : synthèse structurée, assertions prouvées, checklist, note Markdown et réponse sourcée. Une Recette cible une Capture entière ou une sélection explicite de Preuves ou Extractions.
- Un Provider reçoit une demande structurée et retourne un Dérivé ou une erreur structurée. Le Dérivé consigne Provider, version, paramètres effectifs non secrets et Références d'entrée. La première pile locale s'appuie sur les outils validés par capacité ; les interfaces fonctionnelles ne deviennent pas des abstractions Rust publiques avant une seconde implémentation réelle.
- Le rendu Web conserve un DOM rendu comme Preuve et du Markdown sémantique comme Extraction. Il conserve les images, audio, vidéo, PDF et documents incorporés pertinents, sans archiver scripts, CSS ou ressources de mesure. Il n'accomplit pas d'interaction métier.
- Firefox via Playwright est le premier PageRenderer complet. Chromium est le repli complet. WebKit n'est pas éligible au chemin complet tant que son téléchargement ne fonctionne pas. Lightpanda est limité au DOM et contenu textuel sans preuve visuelle ni inventaire complet de ressources.
- Une Source et une Capture ont des identités distinctes. URL demandée, URL finale et hash de Preuve sont conservés. Une nouvelle observation produit une nouvelle Capture et une Version de Capture ; les Liens de découverte forment un Graphe de provenance explicite.
- Une Découverte est une observation durable d'une Source avec son parent, Locator, ordre, statut et motif. La découverte suit un parcours breadth-first, par profondeur puis ordre DOM. Les statuts couvrent au minimum capture, réutilisation, cycle, budget, Policy, support et échec.
- `safe-local@1` borne une découverte à la profondeur 2, 50 Sources, 2 GiB téléchargés, 10 GiB sur disque, 30 minutes par Job et une concurrence de 2. Au dépassement, les éléments restants sont inventoriés sans acquisition et la Capture finit partielle. `capture continue` reprend explicitement les Découvertes ignorées dans un nouveau Job.
- Toute opération `capture` ou `derive` exige une Policy JSON versionnée et validée. Le Job en conserve l'identité, version, hash et snapshot canonique. Les Doublons sont uniquement `reuse`, `create` ou `fail`. Les Providers et appels distants sont interdits sauf autorisation explicite de la Policy.
- Les Jobs vivent dans un Référentiel séparé avec état courant mutable et événements append-only. Ils suivent les états `queued`, `running`, `succeeded`, `partial`, `failed`, `cancelled` et `interrupted`. Un Worker absent rend un Job interrompu ; une relance crée un nouveau Job lié.
- Le Contrat agent retourne du JSON stable. Les commandes de création retournent immédiatement le Job ; les opérations de contrôle permettent de consulter, attendre avec timeout et annuler de façon idempotente.
- Le Contrat agent sépare liste directe du Référentiel, recherche dans l'Index et lecture d'artefact. La pagination est stable, avec curseur opaque et limites de 20 par défaut et 100 au maximum. Une lecture retourne métadonnées et extrait texte de 8 KiB au plus ; une plage explicite ne dépasse pas 1 MiB. Les binaires ne sont jamais écrits sur stdout.
- Une Référence contient au minimum `capture_id`, `artifact_id`, `sha256` et un Locator éventuel. Les réponses, Recettes et Dérivés les réutilisent, avec l'extrait effectivement lu.
- Tout contenu issu d'une Source est non fiable. Une Source ne peut déclencher ni commande ni Provider ni acquisition secondaire. L'acquisition Web refuse les schémas non HTTP(S), les accès locaux et réseaux privés, y compris après redirection et résolution DNS. Aucun secret ou contenu non sélectionné n'est transmis à un Provider distant.
- Le skill versionné impose : validation de la demande et de la Policy, Capture, attente, inspection, choix explicite de Recette, Dérivé, attente et lecture bornée de vérification. Il s'arrête sur tout état partiel, échec, garde-fou, Doublon non résolu ou action non demandée. Il ne crée jamais spontanément une Capture, reprise, Recette, appel distant ou lecture coûteuse.

## Testing Decisions

- Le seam unique est le binaire `scriptor` et son Contrat agent JSON. Les tests vérifient les commandes et fichiers observables, pas les modules internes ni les détails de processus.
- Les tests d'intégration existants du binaire servent de prior art : environnement XDG isolé, Providers et binaires externes remplacés par des doubles contrôlés, et attente bornée des Jobs asynchrones.
- Les doubles de Provider doivent exposer la version, les paramètres, les réponses, les échecs et les artefacts produits afin de vérifier la provenance et les états par Capacité.
- Les tests doivent couvrir : publication atomique d'une Capture portable, hashes et ledger append-only, Locators, Dérivés immuables, Doublons sous chaque mode de Policy, tous les états de Job, interruption et annulation idempotente.
- Les tests de découverte couvrent l'ordre breadth-first, les plafonds de `safe-local@1`, les cycles, les Découvertes ignorées, la reprise explicite et l'absence d'acquisition après épuisement du budget.
- Les tests de Contrat agent couvrent le JSON de succès et d'erreur, pagination stable, Index absent, lecture bornée, Références et refus de binaires sur stdout.
- Les tests de sécurité couvrent instructions injectées dans une Source, redirections vers des cibles refusées, accès local ou privé refusé, absence de transmission de secrets et appel distant refusé sans Policy.
- Les tests d'intégration de PageRenderer utilisent les mêmes fixtures locales : page statique, SPA, lazy-load fini, iframe, média, document téléchargeable, CSS/canvas et échec. Firefox et Chromium doivent satisfaire le contrat complet ; WebKit et Lightpanda ne peuvent être annoncés que dans leur périmètre validé.
- Une validation réelle sur NixOS reste requise avant livraison d'un Provider : Firefox, Chromium et Lightpanda selon leurs périmètres, avec mesure de durée, mémoire, CPU, ressources observées et artefact visuel lorsque requis.

## Out of Scope

- Sources derrière authentification, paywall ou contrôle d'accès, y compris conservation ou transmission de cookies et secrets.
- Interface graphique de consultation, gestion ou orchestration.
- PageRenderer WebKit complet tant que son contrat de téléchargement n'est pas validé.
- Lightpanda comme renderer de Preuve visuelle, de téléchargement ou d'inventaire exhaustif des ressources.
- Tout comportement implicite d'autorisation, de sélection de Provider, de Doublon, de Recette, de reprise ou d'appel distant.
- Migration détaillée des Sorties v1 vers des Captures v2 et compatibilité de commandes v1, qui nécessitent une décision de livraison séparée.

## Further Notes

- Cette spec remplace le modèle fonctionnel de Sortie v1 pour le périmètre v2, sans modifier rétroactivement l'usage de transcription existant.
- Les termes métier normatifs sont ceux du glossaire : Source, Capture, Preuve, Extraction, Dérivé, Provider, Policy, Job, Référence, Découverte, Contenu non fiable et Contrat agent.
- La suite du travail est un découpage en tickets d'implémentation dépendants, sans réouvrir les décisions déjà indexées par la carte Wayfinder.
