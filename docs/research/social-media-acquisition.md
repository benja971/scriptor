# Acquisition de publications sociales

État de la recherche au 14 septembre 2026. Ce document distingue les faits publiés par les
éditeurs, les observations reproductibles et les inférences techniques. Les affirmations
marketing ne sont pas considérées comme une preuve d'implémentation.

## Conclusion

Les services web étudiés ne montrent pas qu'un navigateur graphique est nécessaire. Leur
architecture observable ou déclarée est généralement la même : le client envoie une URL de
publication à un serveur, le serveur résout les métadonnées et les URL de médias Instagram,
puis le navigateur télécharge un fichier depuis Instagram ou via le proxy du service. Inflact
décrit explicitement ce proxy. FastDL expose en plus une offre d'API capable de retourner
médias et captions.

Cette famille de services valide le routage suivant pour Scriptor : un extracteur spécialisé
doit être essayé avant le rendu de page. Le navigateur reste un fallback pour les pages dont
le texte n'est pas disponible autrement. Aucun des services propriétaires examinés ne fournit
toutefois un composant intégrable, inspectable et suffisamment stable pour devenir une
dépendance de Scriptor.

Les captions sont le point faible de ces comparaisons. FastDL les annonce dans son API, 4K
Stogram et Insget les sauvegardent explicitement, mais SnapInsta, Toolzu et le Downloader
Inflact ne documentent pas leur restitution. Télécharger le média et restituer la caption
doivent donc être deux assertions de contrat séparées.

## Synthèse

| Outil | Exécution probable | Médias publics | Caption | Lot | Authentification | État et intérêt pour Scriptor |
|---|---|---|---|---|---|---|
| SnapInsta | Service serveur, puis lien ou flux de téléchargement | Photos, carrousels, vidéos, Reels, Stories selon les sites qui portent ce nom | Non documentée | Carrousel, capacités de profil variables selon le domaine | Aucune pour le public | Marque et domaine non fiables, aucune API publique |
| FastDL | Résolution serveur vers les médias Instagram, téléchargement sans stockage annoncé | JPG, MP4, Reels, Stories, carrousels | Oui dans l'API annoncée, non prouvé dans l'UI gratuite | Carrousels multi-vidéos, pas de lot multi-URL documenté | Aucune pour le public | API et MCP annoncés, mais accès sur contact et aucun contrat public |
| Toolzu | Serveurs Toolzu, puis téléchargement navigateur | Photos, vidéos, Reels, Stories, carrousels, profil | Non documentée | Carrousel et jusqu'à 12 posts d'un profil annoncés | Aucune pour le public | Service propriétaire, politique de logs incompatible avec une dépendance privacy-first |
| Inflact | Proxy serveur explicite, stockage cloud pour le premium | JPG, MP4, photos, carrousels, vidéos, Reels, Stories, profils | Non documentée pour le Downloader | Profil complet et téléchargement en masse payants | Compte Inflact pour le lot, pas de compte Instagram annoncé pour le public | Mécanisme le mieux décrit, mais SaaS propriétaire, quotas et rétention cloud |
| 4K Stogram | Application locale utilisant une session Instagram et éventuellement un proxy | Photos, vidéos, Reels, Stories, carrousels, profils | Oui, metadata et export | Profils, hashtags, lieux, suivi et mise à jour automatique | Login Instagram obligatoire depuis la 4.2 | Discontinué, dernière version 4.9 de mai 2024, risque de blocage reconnu |
| Insget | Application Android locale, session Instagram locale probable | Photos, vidéos, Reels, Stories, carrousels, public et privé autorisé | Oui, caption et auteur | Carrousel, pas de lot multi-URL documenté | Aucune pour le public, login Instagram pour privé et Stories | Maintenu en 2026, mais SDK publicitaires et code fermé |

## SnapInsta

### Identité et statut

Le nom ne désigne plus un service identifiable sans ambiguïté. Le domaine historiquement cité,
`snapinsta.app`, ne résout pas au 14 septembre 2026 lors d'un test DNS/HTTPS local. Plusieurs
sites actifs utilisent ensuite le même nom (`snapinsta.ai`, `.co`, `.lc`, `.im`, `.vn`) avec
des auteurs, politiques et fonctionnalités différentes. Aucune source primaire ne permet de
prouver qu'un de ces domaines est le successeur officiel de `snapinsta.app`. Il ne faut donc
pas écrire « SnapInsta officiel » dans une décision d'architecture.

### Fonctionnement

Le site actif `snapinsta.ai` affirme traiter chaque URL en temps réel, récupérer le fichier
original depuis les serveurs Instagram, ne pas réencoder et ne conserver ni URL ni historique.
Il annonce photos unitaires, carrousels jusqu'à 20 éléments, MP4, Reels, Stories et IGTV, sans
login ni API publique. Ce texte prouve le contrat revendiqué, pas son implémentation interne ni
sa filiation avec l'ancien domaine ([présentation](https://snapinsta.ai/),
[guide](https://snapinsta.ai/pages/how-to-use),
[confidentialité](https://snapinsta.ai/pages/privacy-policy)).

L'inférence raisonnable est un extracteur serveur qui transforme l'URL de publication en URL
CDN ou en flux temporaire. Le serveur doit intervenir puisque l'URL est soumise au service et
que le site dit la traiter en temps réel. Rien ne prouve l'emploi de cookies, d'une API privée
précise ou d'un navigateur headless. La caption n'est pas annoncée comme sortie.

### Évaluation

La confusion de marque, la disparition du domaine historique, l'absence d'API et l'absence de
code source rendent ce service impropre comme Provider. Il reste une preuve de marché qu'une
acquisition publique sans navigateur visible est possible.

## FastDL

### Fonctionnement prouvé et inféré

FastDL est actif sur `fastdl.app`. Il annonce une application web sans compte, avec JPG pour
les images et MP4 pour les vidéos, couvrant photos, vidéos, Reels, Stories et carrousels. Sa FAQ
dit que les fichiers restent hébergés par Instagram et ne sont ni stockés ni publiés par
FastDL ([présentation](https://fastdl.app/fastdl), [FAQ](https://fastdl.app/faq)).

FastDL annonce surtout une API qui retourne les publications publiques, les Reels, leurs
captions et leurs URL de médias. Il annonce la même donnée via MCP. Ni endpoint, ni schéma,
ni tarification ne sont publics : il faut contacter l'éditeur
([API](https://fastdl.app/api), [MCP](https://fastdl.app/mcp)).

L'inférence la plus parcimonieuse est une extraction côté serveur, suivie de liens directs vers
le CDN Instagram. L'existence d'une API structurée rend un simple screenshot ou rendu navigateur
peu probable comme mécanisme principal. Les cookies, endpoints Instagram et éventuel headless
utilisés côté serveur restent inconnus.

### Confidentialité et maintenance

La politique de confidentialité collecte l'adresse IP, les caractéristiques du terminal et les
données d'usage, utilise Google Analytics, Google Ads et reCAPTCHA. Elle date de février 2021 et
nomme encore le service « iGram » et une société `iGram IO` en Alaska. Les conditions nomment
également `iGram`, alors que le site porte FastDL. Cette incohérence affaiblit les garanties de
gouvernance et de maintenance ([confidentialité](https://fastdl.app/privacy-policy),
[conditions](https://fastdl.app/terms-and-conditions)).

Le site, l'API et la page MCP sont actifs et portent un copyright 2026, mais aucun historique de
versions public ne permet d'évaluer la stabilité. Une API privée accessible sur contact ne doit
pas être confondue avec une dépendance documentée.

## Toolzu

### Fonctionnement

Toolzu décrit un service web sans installation ni login Instagram pour les comptes publics. Il
annonce JPG/vidéo en qualité originale, photos, carrousels, vidéos, Reels, Stories, IGTV et un
téléchargement de profil limité à 12 posts. Sa page indique que « nos serveurs » traitent les
téléchargements. C'est une preuve de traitement serveur, sans détail sur l'extracteur, les
cookies ou les endpoints Instagram
([Downloader](https://toolzu.com/downloader/instagram/),
[guide photo](https://toolzu.com/blog/how-to-use-toolzu-instagram-photo-downloader/)).

La caption n'est pas documentée comme résultat. Le lot documenté concerne un carrousel ou les
12 posts d'un profil, pas une liste arbitraire d'URL.

### Confidentialité et propriété

La page produit affirme que les téléchargements ne sont ni suivis ni journalisés. La politique
générale dit pourtant que Toolzu journalise IP, navigateur, FAI, pages de sortie et nombre de
clics, emploie des cookies de remarketing Google Ads et permet à des sous-traitants d'accéder à
certaines données. Les deux affirmations ne peuvent être conciliées sans périmètre et durée de
rétention plus précis ([Downloader](https://toolzu.com/downloader/instagram/),
[confidentialité](https://toolzu.com/privacy-policy/)).

Toolzu appartient à Wiseway SIA, même société, adresse et numéro d'enregistrement qu'Inflact.
Cette relation est déclarée dans les conditions Toolzu et les mentions Inflact
([conditions Toolzu](https://toolzu.com/terms-of-service/),
[confidentialité Inflact](https://inflact.com/privacy-policy/)). Il est donc plausible que les
deux produits partagent de l'infrastructure ou de la technologie, mais aucune source ne le
confirme : cela reste une inférence.

## Inflact

### Fonctionnement

Inflact donne l'explication la plus précise : l'utilisateur soumet l'URL d'un contenu public et
les serveurs proxy Inflact récupèrent le fichier. Le service gratuit l'envoie vers la mémoire du
terminal. Les formules premium téléchargent en masse un profil public et stockent les fichiers
dans Inflact Cloud pendant 30 jours. Les photos sont livrées en JPG et les vidéos en MP4, sans
réencodage ou amélioration annoncée
([Downloader](https://inflact.com/instagram-downloader/),
[Downloader photo](https://inflact.com/instagram-downloader/photo/),
[présentation du lot](https://inflact.com/blog/premium-instagram-downloader/)).

Il s'agit donc bien d'un backend/proxy spécialisé, pas d'un navigateur graphique ouvert chez
l'utilisateur. Aucune source ne dévoile l'extracteur, les cookies ou l'API Instagram utilisés.
La caption n'est pas annoncée parmi les sorties du Downloader.

### Limites et confidentialité

Les pages 2026 sont incohérentes sur le quota gratuit : certaines indiquent deux téléchargements
par jour et dix par mois, d'autres trois essais. Le quota ne doit donc pas être considéré comme
un contrat stable. Le lot, la priorité proxy et le profil complet sont payants. Seuls les comptes
publics sont annoncés pour ce Downloader.

Inflact collecte notamment IP, cookies, navigateur, horaires, géolocalisation approximative et
referrer. Ses conditions générales contiennent par ailleurs des clauses plus larges sur
l'utilisation d'identifiants Instagram et le risque de bannissement, sans préciser si elles
s'appliquent au Downloader public actuel. Cette ambiguïté empêche de conclure « aucune donnée
privée » pour l'ensemble du produit
([confidentialité](https://inflact.com/privacy-policy/),
[conditions](https://inflact.com/terms-of-service/)).

## 4K Stogram

### Fonctionnement

4K Stogram est une application desktop locale. À partir de la version 4.2, une connexion
Instagram dans l'application est obligatoire. Elle télécharge profils, hashtags, lieux,
photos, vidéos, carrousels, Reels, Stories et Highlights. Elle peut suivre les nouveautés,
filtrer par date et utiliser un proxy. Les captions et commentaires peuvent être enregistrés
dans les métadonnées des photos, avec export des posts et captions dans les anciennes formules
payantes
([version 4.2 et login](https://www.4kdownload.com/blog/2021/12/21/introducing-new-4k-stogram-release--1/),
[fonctionnalités](https://www.4kdownload.com/products/product-stogram),
[proxy et version 4.3](https://www.4kdownload.com/blog/2022/02/22/announcing-new-4k-stogram--1/)).

Le mécanisme précis est propriétaire. Le login obligatoire, les « requêtes serveur » et les
risques de trafic détecté décrits par l'éditeur indiquent une session Instagram utilisée par le
client local, avec requêtes automatisées directes ou proxifiées. Rien n'indique qu'un navigateur
graphique soit le chemin principal.

### Statut

4K Stogram est officiellement discontinué. La dernière version affichée est 4.9.0.4680 du
18 mai 2024. Elle reste téléchargeable, mais ne reçoit plus de correctifs, de support ou de
nouvelles fonctions et aucune nouvelle licence premium n'est vendue. L'éditeur invoque les
restrictions imprévisibles d'Instagram et le danger pour les comptes utilisateurs
([avis officiel](https://www.4kdownload.com/faq/faq-why-is-stogram-discontinued)).

Ce cas est instructif mais disqualifiant : réutiliser un compte Instagram pour du batch expose
à la détection et au blocage. L'éditeur recommandait lui-même de ralentir, désactiver les mises à
jour automatiques et utiliser un compte séparé
([conseils anti-blocage](https://www.4kdownload.com/blog/2023/01/17/how-to-safely-use-4k-stogram--1/)).

## Insget

### Fonctionnement

Insget est l'application Android `com.shirokovapp.instasave`, éditée par Spaple. Google Play
indique une mise à jour le 27 juin 2026. Elle accepte une URL copiée ou le partage Android,
télécharge photos, vidéos, Reels, IGTV, Stories et carrousels, puis conserve le média dans la
galerie. Elle sauvegarde explicitement la description du post et son auteur. Le public ne
nécessite pas de login ; le privé et les Stories nécessitent une connexion Instagram et, pour
le privé, que le compte utilisateur suive déjà la source
([fiche Google Play](https://play.google.com/store/apps/details?id=com.shirokovapp.instasave)).

L'application et sa politique disent que les identifiants Instagram ne sont ni traités, ni
stockés, ni transmis à des tiers et que les informations demandées restent sur l'appareil. Une
formulation aussi absolue n'explique pas comment la session est établie. L'inférence raisonnable
est une authentification et des requêtes Instagram effectuées localement, mais le code fermé ne
permet pas de la vérifier
([politique Insget](https://spaple.ru/apps/en/insget/privacy-policy/)).

Google Play déclare une collecte possible des diagnostics, performances et identifiants de
terminal, avec chiffrement en transit et sans mécanisme de suppression. La politique liste
Google Play Services, AdMob, Appodeal et Firebase. « Les identifiants restent locaux » ne signifie
donc pas « aucune télémétrie ou publicité »
([sécurité des données](https://play.google.com/store/apps/datasafety?id=com.shirokovapp.instasave)).

Insget constitue une preuve actuelle qu'un client spécialisé peut restituer média et caption
sans navigateur visible, mais pas une base réutilisable pour un CLI Rust privacy-first.

## Implications pour Scriptor

1. Détecter la plateforme à partir de l'URL avant toute navigation. Pour Instagram, essayer un
   Provider média spécialisé, puis seulement un rendu de page en fallback explicite.
2. Faire produire au Provider une collection ordonnée d'assets, pas un unique fichier : une
   publication peut être une photo, une vidéo ou un carrousel mixte.
3. Modéliser la caption comme un artefact de premier rang, avec auteur, URL canonique,
   horodatage, type de publication et provenance. Ne pas déduire la réussite de la seule présence
   d'un MP4 ou JPG.
4. Séparer public sans session et privé avec session. Le support privé ne doit jamais faire
   partie d'une Policy sûre par défaut. 4K Stogram montre le risque réel de blocage des comptes.
5. Préférer un extracteur local, inspectable et versionné. Les services gratuits peuvent servir
   d'oracles manuels de comparaison, mais leur gouvernance, leurs quotas, leur confidentialité et
   leurs contrats sont trop faibles pour une dépendance de production.
6. Capturer la méthode réelle dans la provenance : Provider et version, authentification,
   endpoint ou stratégie d'extraction, URL CDN finale, cookies/proxy éventuels, ordre des assets,
   hashes et caption brute.
7. Tester séparément au minimum : photo unique, carrousel photo, vidéo de feed, Reel, carrousel
   mixte, caption vide, caption longue, post supprimé, compte privé, rate-limit et expiration
   d'URL CDN.
8. Ne pas extrapoler Instagram à LinkedIn. Aucun des six outils ne revendique LinkedIn ; le
   routage et le Provider LinkedIn doivent être validés indépendamment.

## Tests locaux Instagram

Les essais se trouvent dans `/tmp/scriptor-instagram-bakeoff.GlD7Dq`. Ils utilisent
`yt-dlp 2026.08.19`, `gallery-dl 1.32.1`, `curl 8.21.0` et `ffprobe 9.0.1`, sans cookies et
sans navigateur. Les cinq Sources fournies par l'utilisateur sont publiques.

Le résultat déterminant est que l'extracteur Instagram de `yt-dlp` obtient déjà la caption,
les éléments ordonnés et leurs URL CDN depuis les données GraphQL. Son chemin de téléchargement
standard refuse ensuite les éléments image avec `No video formats found`. L'interface CLI
publique permet toutefois de conserver ces métadonnées avec
`--skip-download --ignore-no-formats-error --dump-single-json`. Elle retourne les huit éléments
du carrousel, leurs miniatures et la caption, ainsi que la miniature et la caption de la photo
seule. Le Provider peut donc parser cette sortie JSON puis télécharger les meilleures URL sans
dépendre de l'API Python privée de `yt-dlp`.

### Matrice vérifiée

| Type | Source | Résultat | Caption |
|---|---|---|---|
| Carrousel photo | `DcphdMaGViZ` | 8 JPEG de 1 122 x 1 402, 2 377 001 octets | complète, 529 octets |
| Photo seule | `DdIrmSPt2qb` | 1 JPEG de 1 365 x 1 820, 227 058 octets | complète, 164 octets |
| Reel | `DdKRcHJOeP3` | MP4 VP9/AAC de 1 080 x 1 920, 41,377 s, 2 130 790 octets | complète, 344 octets |
| Reel | `DdRTSzQhOgu` | MP4 VP9/AAC de 1 080 x 1 920, 23,011 s, 1 038 985 octets | complète, 300 octets |
| Reel | `Dbm0tAtCoDn` | MP4 VP9/AAC de 1 080 x 1 920, 23,168 s, 4 403 013 octets | complète, 324 octets |
| Vidéo de publication classique | `aye83DjauH` | MP4 H.264 de 640 x 640, 8,742 s, 1 017 829 octets | complète, 386 octets |

Les cinq Sources utilisateur ne contenaient aucune vidéo de publication classique. Le dernier
cas emploie donc le fixture public `https://instagram.com/p/aye83DjauH/`. Une URL `/p/` pointant
vers le shortcode d'un Reel a aussi été reconnue avec le même identifiant et six formats : le
routage ne doit pas se fier uniquement au segment `/p/` ou `/reel/`.

### Empreintes des médias Instagram

```text
DcphdMaGViZ/01 d782aca1de2fd6cb5889e6c9d3ccb495cb8e82491e4223769cb2179d525e4def
DcphdMaGViZ/02 942a32e810de588120490050d06d643314f0ebac8a65403dcda7b8908c11a62b
DcphdMaGViZ/03 878d7d7292fbec5b5b09d0d82e3b5df90c589056fc29400136cd4f0aeb9a46d0
DcphdMaGViZ/04 23dc4ad5d516f1e95eca4ea41ab4d8ee5cfeabd09ac12147fa5215f87d00b3ea
DcphdMaGViZ/05 6b3e943439a5a8ab2a2f6fb9e0c1d6536cd1739043c9e462bf735f8d29032a5c
DcphdMaGViZ/06 3eeac07f059b585cf7e88bf3b5a1faddd4576abbb6d4eb954ec9295ac4331803
DcphdMaGViZ/07 bb8ca726e84be03b78fc3430fb28e819c0d8216be66075341992747c2fcfd965
DcphdMaGViZ/08 fbfe85b6cce32fdf6030fa7e7062ec59ced32ee2c4e1cc166115eb8d416f1048
DdIrmSPt2qb    880c77d02418bfaf106a3134ff047b56b78405058984200e86ee297944ae34ed
DdKRcHJOeP3    86f72984bf539ae0181e27351bee45b489ed9b0e2f5df9aa8b4da0af4a091b36
DdRTSzQhOgu    583d0ed0370b9ec505345ee5b3bfa24f1f54cac01ef3370cd56b3dc5dd6935d2
Dbm0tAtCoDn    3048fc8b3c37a7d2a47180731cd3ef9471970bbbfb19b666539540e730e5cae4
aye83DjauH     909bc3d959d55c9ed4c6a64a389dd264aad5ba3bb7514c2b64988ed935d31653
```

`gallery-dl` redirige les cinq Sources vers le login et ne fonctionne pas sans cookies dans cet
environnement. Un GET HTTP simple reçoit bien un statut 200, mais son HTML initial ne contient
ni la caption ni les médias. Le Provider Instagram recommandé doit donc utiliser l'extracteur
spécialisé `yt-dlp`, produire une collection ordonnée de médias de types mixtes, et traiter la
caption comme un artefact indépendant. Le navigateur ne doit être qu'un fallback explicite.

## Tests locaux LinkedIn

Les essais suivants se trouvent dans `/tmp/linkedin-provider-bakeoff.gFr8de`. Ils démontrent
qu'un Provider LinkedIn HTTP-first peut acquérir les principaux types de publication sans
cookies et sans navigateur. `yt-dlp` seul n'est pas un extracteur LinkedIn général : il échoue
sur la photo et le document mais réussit sur la vidéo après découverte de son URL.

### Photo

Source : publication fournie par l'utilisateur,
`thomasquinet_la-landing-page-parfaite-nexiste-pas-pourtant-share-7504156561989165056-lXpz`.

- caption JSON-LD : 1 083 octets ;
- JPEG de publication : 800 × 1 000, 105 966 octets ;
- SHA-256 : `d1295db0d026ac17c86e1ccbd04c002425569248e6da929eedc4a060bbcf091a` ;
- cookies et navigateur : aucun ;
- `yt-dlp` : échec.

### Galerie multi-images

Source : [publication Reagan Tillman](https://www.linkedin.com/posts/reagan-tillman-147335241_ive-been-a-little-quiet-on-linkedin-lately-activity-7486053781730918400-Jxou).

- caption JSON-LD : 1 050 octets ;
- `SocialMediaPosting.image` : tableau ordonné de 7 `ImageObject` ;
- 7 JPEG téléchargés, de 800 x 600 ou 1 152 x 1 536 ;
- cookies et navigateur : aucun ;
- `yt-dlp` : échec.

```text
01 89baa2121bcce9ac2f1a13cceddc1eb6337a81f3aeb32243079e8bfc911f4d49
02 cb8a7c17c0c4a24b7e7b66f3c8ab3cdd77e354069623d2405ce41acaa297ba2a
03 cfcc221efe92e9c45dcb77d651cbdb9bfa60001fc984695585a4ea8a3a7c2e0d
04 480c8f4fbf4ca4bdbe3769fd5d054dfaf9a4346196931916234fc3054c369f21
05 90e861584cc1b0f6850dd9d0e53584b20e203d8501ca285d621766fe094286f4
06 48afc8b8159d3d5190697a3fde51fde4482c36a9cdeed9c980f0177867d4cbb0
07 5d7ef3d428e6c8712365af1533968e4b76606c5608d6139d6aff082b8de6c724
```

### Document carrousel

Source : [publication Buffer](https://www.linkedin.com/posts/bufferapp_ideas-for-your-next-linkedin-carousel-activity-7141085148824899584-61fx).

- caption JSON-LD : 739 octets ;
- `data-native-document-config` : `totalPageCount` de 6 et URL de manifest ;
- PDF : 6 pages, 74 834 octets ;
- SHA-256 du PDF : `86a7b68c7ba532aab4e5890a1347d20825c8c90d102b7f6110a3ae8c11edb944` ;
- rendu disponible : 6 PNG de 1 923 × 1 923 ;
- cookies et navigateur : aucun ;
- `yt-dlp` : échec.

### Vidéo

Source : [publication Schwedelson](https://www.linkedin.com/posts/schwedelson_1-format-for-linkedin-engagement-right-now-activity-7454518714114273280-zzIx).

- JSON-LD `VideoObject` : `contentUrl`, caption, transcript et piste VTT ;
- MP4 H.264/AAC : 720 × 1 280, 59,536 s, 6 581 212 octets ;
- SHA-256 : `65b3322873c9a646c49d96f6e856ae3c8c045b278af46e2d0e278090b068f4e5` ;
- caption isolée : 81 octets ;
- cookies et navigateur : aucun.

Le Provider LinkedIn recommandé doit donc commencer par un GET HTTP, parser le JSON-LD puis
`data-native-document-config`, et transmettre l'URL vidéo à `yt-dlp` ou télécharger l'asset
directement. Les CDN retournent des URL signées : il faut acquérir immédiatement le média et
enregistrer URL finale, horodatage et hash dans la provenance. Le navigateur ne sert qu'en
fallback si ces données structurées sont absentes.
