# Validation réelle des PageRenderers sur NixOS

Validation effectuée le 14 septembre 2026 sur NixOS 26.05, depuis
`nix develop`, avec deux Sources Instagram publiques fournies par
l'utilisateur. Les URLs canoniques ont été conservées ici sans leurs paramètres
de partage.

Ce test concernait directement `nix develop --command
scriptor-page-renderer`. Les temps et mémoires incluent donc le faible surcoût
du shell Nix. Chaque invocation utilisait une limite de téléchargement de 2 GiB
et une limite de sortie de 1 GiB.

Depuis la Capture sociale dédiée, Instagram et LinkedIn ne passent plus par ce
renderer: cette validation reste utile pour son Contrat générique Web et ses
protections réseau, pas pour attester l'Extraction sociale actuelle.

## Firefox

Source : `https://www.instagram.com/p/DcphdMaGViZ/`

- état : succès ;
- durée : 14,60 s ;
- CPU : 92 % ;
- mémoire résidente maximale : 394 896 KiB ;
- taille du dossier : 1 419 582 octets ;
- DOM : 1 156 298 octets ;
- Markdown : 653 octets ;
- screenshot : PNG 1280 x 720, 225 664 octets,
  SHA-256 `3c794bb39cfd895071b35d196c185c9e630ad276bcf5149ccf4f644f3cd222e1` ;
- Découvertes : 29 contenus incorporés inventoriés ;
- redirection : aucune.

La première tentative a reproduit un `EPIPE` non géré quand le navigateur
fermait une connexion du proxy. Des gestionnaires d'erreur relient désormais
les deux côtés des flux HTTP et CONNECT. Le même scénario a ensuite réussi.

## Chromium

Source : `https://www.instagram.com/reel/DdKRcHJOeP3/`

- état : succès ;
- durée : 8,22 s ;
- CPU : 44 % ;
- mémoire résidente maximale : 166 644 KiB ;
- taille du dossier : 1 118 155 octets ;
- DOM : 986 695 octets ;
- Markdown : 653 octets ;
- screenshot : PNG 1280 x 720, 106 923 octets,
  SHA-256 `46bca209e18d7e039709d3b82495552b44f3ea71aa2c0571d9d4ba4000c91d49` ;
- Découvertes : 19 contenus incorporés inventoriés ;
- redirection : aucune, hormis la normalisation d'encodage de l'URL finale.

## Inspection visuelle et limites

Les deux screenshots sont lisibles et montrent la publication demandée en
arrière-plan. La boîte de consentement aux cookies Instagram masque toutefois
la majeure partie du contenu. Scriptor n'accepte ni cookie ni interaction
métier, donc le renderer ne ferme pas cette boîte. Le succès prouve le contrat
technique DOM, Markdown, screenshot, provenance et Découvertes, mais pas une
Extraction complète du contenu de la publication.

Le Contrat agent démarre par Firefox et ne relance Chromium que lorsque Firefox
retourne l'erreur structurée `web_renderer_firefox_unavailable`. Cette relance
ne s'applique jamais aux échecs de navigation ou de sécurité. Le moteur retenu
est conservé dans `provenance.json` et dans le Provider de l'Extraction
Markdown.

## Lightpanda

Lightpanda est disponible seulement par sélection explicite : `capture <URL>
--policy safe-web@1 --renderer lightpanda`. Il produit le DOM et le Markdown
depuis une unique session CDP via `LP.getMarkdown`, sans screenshot, inventaire
ni acquisition secondaire de ressources. La suite déterministe
`nix develop --command scriptor-page-renderer-test` valide ce périmètre avec le
proxy épinglé, une fixture locale et l'absence des deux artefacts interdits.
Cette validation n'est pas une mesure live comparable aux résultats Firefox et
Chromium ci-dessus : elle ne leur attribue donc ni durée, ni mémoire, ni CPU.
