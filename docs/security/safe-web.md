# Safe Web acquisition boundary

`safe-web@1` permits only public HTTP(S) page acquisition. It permits no
remote Provider. Each URL is checked before the renderer starts, before each
routed resource request, and again after navigation: unsupported schemes,
credentials, localhost, private, link-local, loopback, multicast and DNS
answers containing non-public addresses are refused.

The renderer disables service workers and runs Firefox by default. It never
silently switches browser engines: an unavailable Firefox produces the
structured `web_renderer_firefox_unavailable` Job error. Chromium remains an
explicit renderer mode and applies its own WebRTC network restrictions.

WebRTC cannot escape the boundary: Firefox disables PeerConnection with launch
preferences, while Chromium is launched with its non-proxied UDP policy and no
media permissions. Page JavaScript sees no WebRTC API as defense in depth.

This is still an application-layer SSRF guard. DNS rebinding can change an
accepted hostname between validation and the browser's own connection.
Deployments requiring a kernel-level guarantee must enforce an egress firewall
or isolated network namespace that blocks private and link-local ranges for the
renderer process.
