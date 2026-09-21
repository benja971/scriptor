# Projeter les erreurs structurées derrière un seam privé

## Status

accepted

Le module privé `error` construit les erreurs structurées du Contrat agent, des Jobs et des Capabilities. Les modules d'acquisition restent propriétaires de leurs erreurs métier et les transmettent à ce seam de projection.

## Consequences

Les fallback `capture_failed` et `derive_provider_failed` sont conservés quand un Provider ne fournit pas d'erreur typée. Aucun trait public n'est créé et le Contrat agent reste inchangé.
