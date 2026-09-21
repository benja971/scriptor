# Recherche : import d'une collection Instagram sauvegardée

Recherche menée le 20 septembre 2026 pour cadrer l'issue #79. Elle distingue
l'accès qu'un utilisateur autorise dans son navigateur de l'accès que la
plateforme documente pour une application. Ils ne sont pas équivalents.

## Conclusion

L'API officielle Instagram ne permet pas d'énumérer les publications
sauvegardées ni les collections sauvegardées. Scriptor ne doit donc ni promettre
un import Graph API, ni appeler des endpoints privés observés dans le trafic
du site.

Le premier flux utile est plus étroit : l'utilisateur connecté ouvre sa
collection, copie lui-même les permaliens publics qu'il veut traiter, puis les
fournit au futur lot de Capture. Scriptor ne reçoit que ces URLs publiques et
les traite avec le Provider Instagram public existant. Cette voie ne transmet
ni mot de passe, ni cookie, ni token, ni URL de collection à Scriptor.

Le futur contrat de lot de l'issue #78 est un prérequis. L'import ne doit pas
inventer une seconde orchestration de Jobs.

## API officielle : pas de collection sauvegardée

La [référence Instagram Platform](https://developers.facebook.com/documentation/instagram-platform/reference.md)
ne décrit aucun nœud `Saved` ou `Collection`. La
[référence IG User](https://developers.facebook.com/documentation/instagram-platform/instagram-graph-api/reference/ig-user.md)
liste les edges de compte, dont `media`, `stories`, `tags`, `mentions` et
`insights`, mais aucun edge de publications ou collections sauvegardées. C'est
une inférence forte d'absence dans le contrat public, pas une preuve sur les
services internes de Meta.

Le périmètre documenté vise les comptes Instagram professionnels et leurs
données publiées ou autorisées, pas la bibliothèque privée d'un compte
personnel. Voir [vue d'ensemble Instagram Platform](https://developers.facebook.com/documentation/instagram-platform/overview.md).
Les scopes Instagram Login publiés sont `instagram_business_basic`,
`instagram_business_content_publish`, `instagram_business_manage_messages` et
`instagram_business_manage_comments` : aucun ne couvre les éléments
sauvegardés. Voir [Instagram API with Instagram Login](https://developers.facebook.com/documentation/instagram-platform/instagram-api-with-instagram-login.md).

Conséquences :

- Ne pas créer d'application Meta ni demander OAuth pour cette capacité.
- Ne pas utiliser `i.instagram.com/api/v1/...`, GraphQL interne ou une
  interception réseau. Ces interfaces ne sont pas un contrat public.
- Ne pas traiter l'accès utilisateur au site comme une autorisation de
  réutiliser sa session par programme.

## Observation de navigateur

Dans la session locale autorisée par l'utilisateur, la page de collection
`/saved/.../` exigeait une connexion Instagram et rendait des permaliens de
publications publiques, notamment sous la forme `/p/<shortcode>/`. Cette
observation confirme qu'un humain peut constituer une liste d'URLs sans API.
Elle ne garantit ni structure DOM, ni pagination, ni chargement infini, ni
stabilité de l'interface web.

Une publication vue dans la collection peut ensuite être privée, supprimée,
géobloquée ou login-gated. Chaque URL reste donc une Source indépendante : un
échec ne doit pas invalider les autres éléments du lot.

## Frontière de sécurité

La Policy [`safe-web@1`](../security/safe-web.md) est correctement
cookie-free : elle efface les variables d'environnement hors `PATH`, dont les
identifiants, cookies et tokens. Ne pas l'élargir pour cette issue.

Règles non négociables :

- Scriptor ne lance pas de connexion Instagram et ne lit pas le profil de
  navigateur de l'utilisateur.
- Scriptor ne lit, ne copie, ne journalise et ne persiste ni cookies, ni mots
  de passe, ni token OAuth, ni en-têtes d'authentification.
- Scriptor ne navigue pas une page authentifiée et ne clique pas, ne scrolle
  pas et ne charge pas automatiquement la collection.
- Le nom et l'URL de collection sont des préférences privées. Ils ne sont pas
  de la provenance d'une Capture et ne doivent pas figurer dans un Manifest ou
  dans les logs.
- Les seules entrées acceptées sont des URLs HTTP(S) publiques, validées par
  la Policy existante avant chaque Capture.

## Approche recommandée

### Version initiale

1. L'utilisateur ouvre et parcourt lui-même sa collection dans Instagram.
2. Il copie les permaliens publics voulus dans un fichier texte local ou vers
   l'entrée standard de la commande de lot issue de #78.
3. La commande normalise, déduplique et affiche la liste avant création de
   Jobs.
4. Elle exécute la Capture publique existante pour chaque URL et retourne un
   bilan par URL, avec reprise des seuls échecs.

La provenance enregistrée commence à l'URL publique demandée. Elle ne prétend
pas que la collection est une Source, qu'elle est complète, ou que son nom est
une catégorie fiable.

Critère de sortie : une liste locale de dix permaliens publics produit dix
résultats individuels et aucun secret, cookie ou URL `/saved/` n'apparaît dans
le Référentiel, les manifests ou les logs.

### Helper local à geste explicite

Le helper [`scripts/instagram-visible-permalinks.js`](../../scripts/instagram-visible-permalinks.js)
est déclenché manuellement par l'utilisateur dans l'onglet actif. Il extrait
seulement les permaliens de publications déjà rendus dans le DOM, les normalise
et les copie pour relecture dans le presse-papiers. Il ne demande ni permission
cookies, ni interception réseau, ni chargement automatique de pages, ni
synchronisation périodique.

L'utilisateur colle ensuite ces lignes dans un fichier local UTF-8 et appelle
`scriptor knowledge batch --source-file <chemin>` avec les Policies explicites.
Ce helper n'est pas un import automatisé de collection : l'utilisateur reste
responsable de l'ouverture, du défilement, de la sélection et de la relecture.
L'absence d'API officielle pour Saved rend toute automatisation authentifiée
fragile et potentiellement incompatible avec les règles de la plateforme.

## Inconnues à lever avant toute extension

- Le téléchargement officiel des données Instagram contient-il les éléments
  sauvegardés et l'appartenance aux collections, et dans quel format ? Cette
  couverture n'est pas documentée par les sources API consultées.
- Une collection collaborative expose-t-elle les mêmes permaliens et quel est
  leur statut de confidentialité ?
- Les permaliens visibles dans l'interface web couvrent-ils toutes les formes
  de posts, notamment Reels et contenus partagés ?
- Les conditions Instagram permettent-elles un helper local qui extrait les
  liens visibles d'une page authentifiée ? Il faut une réponse explicite avant
  de livrer ce helper.
