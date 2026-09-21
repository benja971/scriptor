# Recherche : PageRenderers locaux alternatifs à Chromium

Recherche menée le 12 septembre 2026 pour le ticket Wayfinder « Comparer les
PageRenderers locaux alternatifs à Chromium ». Elle évalue le moteur chargé de
produire, pour une page publique, le DOM après JavaScript, les ressources
observées et une preuve visuelle. Les affirmations techniques ci-dessous sont
reliées aux documentations ou sources officielles des projets. Les chiffres de
performance ne sont retenus que lorsqu'ils ont une méthode reproductible : ce
critère écarte les promesses marketing non comparables.

## Conclusion

Il ne faut pas remplacer Chromium par un seul autre moteur. La Capacité
`PageRenderer` doit conserver un résultat uniforme et sélectionner un adapter
par besoin :

| Décision | Candidat | Raison | Statut proposé |
|---|---|---|---|
| Pages JavaScript où une image rendue est indispensable | Firefox headless piloté par Playwright | moteur complet, DOM/JS, screenshots, interceptions réseau et téléchargements à travers une API mature | valider en premier, repli non-Chromium de production possible |
| Vue Safari/WebKit ou seconde implémentation de contrôle | Playwright WebKit | mêmes artefacts et API de haut niveau, moteur indépendant de Gecko/Blink | valider après Firefox, pas un remplacement motivé par les ressources |
| Extraction DOM/JS sans pixels | Lightpanda | binaire local à protocole CDP/BiDi et Playwright, conçu pour cette charge | valider comme fast-path, jamais comme unique renderer |
| Embedding applicatif WebKit | WebKitGTK | vrai WebKit, API de snapshot et chargement de ressources | ne pas intégrer : il faudrait écrire et maintenir le client GTK/WebKit dédié |
| Moteur expérimental | Servo | projet actif et empaqueté par Nix, mais se présente lui-même comme prototype | ne pas valider maintenant |
| Moteur historique | PhantomJS | projet suspendu et dernier stable 2.1 | écarter |

La recommandation concrète est de faire du renderer Playwright un adapter qui
peut lancer **Firefox** ou **WebKit**, et de n'introduire Lightpanda qu'après
un benchmark et des fixtures Scriptor. Chromium reste le repli pragmatique
pour les pages que Lightpanda ou un autre moteur ne restitue pas correctement.
Ce n'est pas une préférence de marque : Lightpanda ne produit délibérément pas
de pixels réels, ce qui l'empêche de satisfaire seul le contrat actuel de
Capture.

## Grille de capacités

| Moteur / contrôle | JavaScript et DOM | Client / protocole | Ressources et téléchargements | Artefact visuel | Maturité et Linux/NixOS | Licence, confidentialité, ressources |
|---|---|---|---|---|---|---|
| Firefox + Playwright | oui, vrai navigateur Gecko | WebDriver BiDi natif, WebDriver/Marionette, ou Playwright avec son Firefox patché | événements réseau et `Download` Playwright ; BiDi a `browser.setDownloadBehavior` | screenshots Playwright et WebDriver | Firefox est un paquet nixpkgs ; Playwright ne pilote pas le Firefox de distribution, il utilise un build patché | MPL-2.0 pour Firefox. Aucun chiffre officiel comparable de RAM/CPU retenu. Isoler chaque Capture dans un profil temporaire et désactiver la télémétrie via politiques/préférences documentées |
| WebKit + Playwright | oui, vrai moteur WebKit | API Playwright, pas CDP public équivalent | mêmes APIs Playwright : réseau observé et téléchargements | screenshots Playwright | paquet WebKit Playwright distinct et WebKitGTK dans nixpkgs ; support Linux existe mais les codecs varient selon OS | LGPL-2.1-or-later pour WebKit. Aucun chiffre comparable de RAM/CPU retenu ; le binaire Playwright est téléchargé par défaut depuis le CDN Microsoft, donc le verrouiller/packager avec Nix |
| Lightpanda | oui, V8 + DOM/Web APIs natifs, mais couverture Web API partielle | CDP, WebDriver BiDi, `playwright.chromium.connectOverCDP` | couche réseau, cookies et sous-ressources ; vérifier les événements/téléchargements sur fixtures, pas de promesse générale trouvée | **non** : `Page.captureScreenshot` renvoie un placeholder | nightly Linux glibc, build Zig/Rust avec `flake.nix` officiel ; absent du nixpkgs évalué le 12/09/2026 | AGPL-3.0-only. Une télémétrie anonyme est activée par défaut et doit être désactivée. Les allégations mémoire/CPU du projet doivent être reproduites localement, pas prises comme SLO |
| WebKitGTK direct | oui | API GObject/C, non CDP/Playwright | requêtes, réponse et ressources accessibles par API WebKit, mais orchestration à écrire | `get_snapshot` fournit un snapshot | `webkitgtk_4_1` et `webkitgtk_6_0` existent dans nixpkgs | LGPL-2.1-or-later. La surface GTK rend cet adapter plus coûteux et fragile en headless |
| Servo | moteur, DOM et rendu en évolution | pas de contrat CDP/WebDriver stable documenté pour Scriptor | pas de client d'automatisation prêt à employer trouvé dans les sources officielles | son shell rend, mais pas de contrat de screenshot automatisé retenu | Linux 64-bit annoncé et paquet nixpkgs `servo` | MPL-2.0. Prototype selon son README, pas de mesures officielles comparables retenues |
| PhantomJS | ancien WebKit, DOM/JS et capture à l'époque | API PhantomJS seulement | HAR/network annoncés historiquement | oui historiquement | développement suspendu | BSD-3-Clause, à exclure pour dette de sécurité et compatibilité web moderne |

## Constats sourcés

### Firefox headless : meilleur candidat complet hors Chromium

Firefox peut être lancé avec `-headless` et un profil éphémère par geckodriver
([capabilités officielles](https://developer.mozilla.org/en-US/docs/Web/WebDriver/Reference/Capabilities/firefoxOptions)).
Il expose directement une connexion WebDriver BiDi locale avec
`--remote-debugging-port`, sans wrapper CDP
([guide Mozilla](https://developer.mozilla.org/en-US/docs/Web/WebDriver/How_to/Create_BiDi_connection)).
CDP est possible mais désactivé par défaut depuis Firefox 129 : ne pas bâtir
l'adapter sur cette compatibilité secondaire
([release note Mozilla](https://developer.mozilla.org/en-US/docs/Mozilla/Firefox/Releases/129)).

Pour limiter le code spécifique au protocole, Playwright est le meilleur client
initial : il supporte Firefox et WebKit, accepte les téléchargements, expose
les événements réseau et produit les screenshots dans la même API
([navigateurs](https://playwright.dev/docs/browsers),
[téléchargements](https://playwright.dev/docs/downloads),
[réseau](https://playwright.dev/docs/network)). Attention : Playwright indique
que son Firefox dépend de patches et ne fonctionne pas avec le Firefox de
distribution. Il faut donc choisir explicitement entre le binaire Playwright
reproductible dans le flake et un client BiDi/Marionette pour `pkgs.firefox`.

Firefox 149 documente `browser.setDownloadBehavior` et les screenshots sous
WebDriver et BiDi : les deux capacités demandées sont donc présentes côté
moteur ([release note](https://developer.mozilla.org/en-US/docs/Mozilla/Firefox/Releases/149)).
Nixpkgs fournit Firefox et Firefox ESR. Le paquet s'évalue localement ici ; ce
n'est toutefois pas une validation de l'intégration Playwright sur NixOS.

Firefox est sous MPL-2.0
([NOTICE officiel](https://searchfox.org/mozilla-central/source/toolkit/content/license.html)).
Pour une Capture sans état, lancer un profil neuf sous un répertoire de Job,
effacer le profil après conservation des artefacts, et appliquer les politiques
Mozilla de désactivation de télémétrie plutôt que compter sur des valeurs de
préférences non testées. Aucun benchmark officiel ne permet de soutenir une
affirmation de mémoire ou CPU supérieure à Chromium.

### WebKit : second vrai moteur, mais pas spécialement léger

Playwright exécute Firefox et WebKit sur Linux et prend lui-même des
screenshots WebKit ([documentation de bibliothèque](https://playwright.dev/docs/library)).
Son WebKit dérive de la branche principale de WebKit, pas de Safari, et les
capacités dépendantes de la plate-forme, notamment codecs média, diffèrent sur
Linux ([documentation des navigateurs](https://playwright.dev/docs/browsers)).
Les appels Playwright de téléchargement, réseau et screenshot sont transverses
aux trois moteurs, donc le test doit vérifier la parité au lieu de l'inférer.

WebKitGTK est une alternative d'embedding plus basse couche. Son API
[`get_snapshot`](https://webkitgtk.org/reference/webkit2gtk/stable/method.WebView.get_snapshot.html)
produit une image du contenu d'une WebView. C'est viable techniquement, mais
ne partage pas le client Playwright ni la gestion de jobs : adopter cette voie
revient à maintenir un programme GObject qui gère loop GTK, WebKit process,
réseau, téléchargement et isolation. Sans exigence d'embedding, elle est moins
maintenable que Playwright WebKit.

Playwright télécharge par défaut ses navigateurs depuis le CDN Microsoft et
les stocke sous le cache utilisateur Linux ; les binaires occupent « quelques
centaines de Mo » selon sa documentation
([installation et stockage](https://playwright.dev/docs/browsers)). Pour
NixOS/privacy, les fournir à partir d'un input verrouillé ou d'un paquet Nix
est préférable à un téléchargement implicite pendant une Capture.

### Lightpanda : fast-path DOM, pas renderer visuel

Lightpanda est exactement orienté automatisation headless : son architecture
déclare un réseau, V8, DOM et Web APIs, une exposition CDP/BiDi, et une
connexion Playwright via CDP
([architecture](https://lightpanda.io/docs/core-concepts/architecture-overview),
[usage Playwright](https://lightpanda.io/docs/open-source/getting-started/usage)).
Il peut donc servir aux pages où Scriptor a besoin du DOM final, de scripts et
de l'inventaire des URL de ressources, sans lancer un moteur de rendu complet.

Mais l'absence de renderer est une limite structurelle, non un bug temporaire :
le projet dit que les positions sont simulées et que `Page.captureScreenshot`
renvoie un placeholder. Il faut impérativement envoyer toute Capture qui exige
une preuve visuelle vers Firefox/WebKit/Chromium. La même documentation dit que
la couverture Web API est partielle et renvoie vers son tableau de tests : ne
pas présumer qu'une page JavaScript arbitraire fonctionnera.

La documentation expose bien une couche réseau, les sous-ressources, les
cookies et `robots.txt`, mais aucune garantie retenue ici pour les événements
de téléchargement équivalents à Playwright. Le test doit couvrir : modules JS,
XHR/fetch, lazy-load, iframe, vidéo/image, réponse `Content-Disposition` et
export du DOM stabilisé. Ses comparaisons vitesse/mémoire sont publiées par le
projet lui-même : elles motivent le benchmark, elles ne démontrent pas un gain
sur les Sources de Scriptor.

Lightpanda est AGPL-3.0-only
([licence source](https://github.com/lightpanda-io/browser/blob/main/LICENSE))
et documente une télémétrie anonyme activée par défaut, désactivable : le
provider local devra forcer `LIGHTPANDA_DISABLE_TELEMETRY=true` et la tester au niveau réseau
avant emploi ([local vs cloud](https://lightpanda.io/docs/core-concepts/local-vs-cloud)).

Son benchmark officiel annonce, sur son corpus de démonstration et avec une
méthode publiée, 123 MB contre 2 GB et 4,81 s contre 46,70 s pour 25 tâches
parallèles. Il compare des processus Lightpanda à des onglets Chrome car
Lightpanda ne sait pas ouvrir plusieurs onglets dans un processus. C'est une
raison crédible de mesurer ce fast-path, pas une mesure transférable à des
Captures Scriptor ([benchmark officiel](https://lightpanda.io/docs/core-concepts/benchmarks)).
Les binaires nightly officiels sont liés à glibc et la liste de systèmes
supportés ne cite pas NixOS ; le projet fournit néanmoins un `flake.nix` et
documente `nix develop` pour compiler ([nightly](https://lightpanda.io/docs/run-locally/installation/nightly-builds),
[prérequis](https://lightpanda.io/docs/run-locally/installation/system-requirements),
[build source](https://lightpanda.io/docs/run-locally/installation/build-from-sources)).

### Servo et PhantomJS : ne pas investir

Servo se décrit officiellement comme un « prototype web browser engine »,
malgré ses cibles Linux 64-bit et son packaging nixpkgs. C'est un projet de
moteur et d'embedding, pas un renderer d'automatisation doté d'une interface
de client stable comparable à Playwright ou BiDi
([README Servo](https://github.com/servo/servo)). Son MPL-2.0 est favorable,
mais ne compense pas le coût produit du client à construire.

PhantomJS annonçait DOM, JavaScript, capture d'écran et export HAR, mais son
README dit explicitement que le développement est suspendu et que le dernier
stable est 2.1 ([README PhantomJS](https://github.com/ariya/phantomjs)). Le
BSD-3-Clause et le pur headless Linux ne rendent pas acceptable son moteur WebKit
figé : l'exclure, sans POC.

## Validation à ajouter au ticket suivant

Créer une suite locale de pages contrôlées et lancer chaque candidat avec les
mêmes budgets : 1 page HTML statique, SPA avec `fetch`, lazy-load fini,
iframe, image/audio/vidéo, document en téléchargement, CSS/canvas et un cas
échec. Pour chaque run, mesurer wall time, pic RSS, CPU-seconds, nombre et
octets de requêtes, DOM sérialisé, inventaire des ressources, fichier téléchargé
et screenshot hashé. Les budgets Scriptor décideront alors quel adapter utiliser
et quand basculer, sans transformer une promesse de benchmark fournisseur en
contrat produit.

Les critères de sortie sont :

- Firefox via Playwright ou BiDi produit toutes les preuves demandées sur
  NixOS avec un profil et une configuration sans télémétrie ;
- WebKit est testé sur le même corpus pour préserver une alternative complète ;
- Lightpanda est accepté uniquement pour les fixtures dont le DOM et les
  ressources sont équivalents, sans demander de screenshot réel ;
- tout écart déclenche un repli explicite, enregistré dans la provenance de la
  Capture avec le moteur, sa version, ses arguments et la raison du basculement.
