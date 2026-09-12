# Safe Web acquisition boundary

`safe-web@1` permits only public HTTP(S) page acquisition. It permits no
remote Provider. Each URL is checked before the renderer starts, before each
routed resource request, and again after navigation: unsupported schemes,
credentials, localhost, private, link-local, loopback, multicast and DNS
answers containing non-public addresses are refused.

The renderer disables service workers and tries Firefox first, then Chromium.
This is an application-layer guard, not a network sandbox. DNS rebinding can
still change an accepted hostname between validation and the browser's own
connection. Deployments which require SSRF guarantees must enforce an egress
firewall or isolated network namespace that blocks private and link-local
address ranges for the renderer process.
