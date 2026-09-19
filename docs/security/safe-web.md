# Safe Web acquisition boundary

`safe-web@1` permits only public HTTP(S) acquisition. It permits no remote
Provider: its allowlist contains only local analyzers used after a binary has
been acquired. Each URL is checked before the renderer or binary acquirer
starts, before each routed resource request, and again after navigation or
redirect: unsupported schemes,
credentials, localhost, private, link-local, loopback, multicast and DNS
answers containing non-public addresses are refused.

The renderer and binary acquirer receive a cleared environment with only the
current `PATH`; caller variables, including credentials, cookies and tokens,
are not forwarded.

The renderer disables service workers and starts with Firefox. Only the
structured `web_renderer_firefox_unavailable` error triggers a retry with
Chromium; navigation and security failures never switch engines. The effective
browser is recorded in the renderer provenance and Markdown Extraction
Provider. Chromium applies its own WebRTC network restrictions.

`capture <URL> --policy safe-web@1 --renderer lightpanda` is an explicit,
text-only path. It starts a local Lightpanda CDP server with the same pinned
Node proxy, disables Lightpanda telemetry, and obtains the rendered DOM plus
`LP.getMarkdown` from one browser session. It does not produce a screenshot,
resource inventory, or secondary resource acquisition. Lightpanda's own
private-network flag cannot be enabled because it would reject the loopback
address of that mandatory proxy; the proxy remains the network boundary and
validates and pins every outgoing destination.

WebRTC cannot escape the boundary: Firefox disables PeerConnection with launch
preferences, while Chromium is launched with its non-proxied UDP policy and no
media permissions. Page JavaScript sees no WebRTC API as defense in depth.

This is still an application-layer SSRF guard. DNS rebinding can change an
accepted hostname between validation and the browser's own connection.
Deployments requiring a kernel-level guarantee must enforce an egress firewall
or isolated network namespace that blocks private and link-local ranges for the
renderer process.

Binary discoveries use a separate, cookie-free acquirer behind the same pinned
proxy. It streams into Job staging under a byte ceiling, validates the final
URL, hashes the payload, records the declared MIME type and redirect chain,
then routes only recognized PDF, image, audio, or video types to a specialized
Capture. A MIME declaration that conflicts with recognized magic bytes is not
published. HTML and iframe discoveries are recaptured by the PageRenderer.
