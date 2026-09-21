(() => {
  const permalinkFor = (anchor) => {
    const url = new URL(anchor.href, window.location.origin);
    const match = url.pathname.match(/^\/(?:(?:[^/]+)\/)?(p|reel|tv)\/([^/]+)\/?$/u);
    if (!match) {
      return null;
    }
    return `https://www.instagram.com/${match[1]}/${match[2]}/`;
  };
  const links = [
    ...new Set(
      [...document.querySelectorAll("a[href]")]
        .map(permalinkFor)
        .filter((link) => link !== null),
    ),
  ];
  const output = links.join("\n");
  if (output.length === 0) {
    window.alert("Aucun permalien Instagram public n’est rendu dans cette page.");
    return;
  }
  navigator.clipboard
    .writeText(output)
    .then(() => window.alert(`${links.length} permaliens publics copiés.`))
    .catch(() => window.prompt("Copie ces permaliens publics :", output));
})();
