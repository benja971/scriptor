# Provider capabilities

Read only the section matching the requested Source or Dérivé. These are the
capabilities exposed by Scriptor's current Contrat agent, not every capability
an installed binary may have.

## Policies

`safe-local@1` accepts local Sources and local Dérivés. It records duplicate
mode `reuse`, forbids remote calls, and allows `ffmpeg`, `ffprobe`,
`whisper-cli`, `pdftotext`, `pdfinfo`, `tesseract` and
`scriptor-local-derive` where applicable.

`safe-web@1` accepts public HTTP(S) Sources through the protected Web
acquisition path. It allows network acquisition under its recorded budgets but
does not authorize any Dérivé Recette. A Web Capture must therefore finish
before a separately requested local Dérivé can use `safe-local@1`.

Both Policies currently record these upper bounds in every Job:

- depth: 2;
- Sources: 50;
- downloads: 2 GiB;
- disk: 10 GiB;
- duration: 30 minutes;
- concurrency: 2.

Read the effective snapshot from the Job instead of relying on these cached
values in the final report.

## Local media

`ffmpeg` and `ffprobe` preserve the media Preuve, extract audio and locate
Frames. `whisper-cli` produces the transcription Extraction. Provider versions,
effective parameters and dependencies are recorded in the Capture.

A Capability may be `failed` or `not_attempted` while already valid artefacts
are published in a `partial` Capture. Report those artefacts and stop the
workflow. Do not retry or replace a Provider automatically.

## Local PDF

`pdftotext` produces the text Extraction and `pdfinfo` provides page Locators.
The original PDF remains the hashed Preuve. A missing, refused or failed
Provider appears as a partial Capability and stops the workflow.

## Local image

`tesseract` produces OCR text with typed image-region Locators. OCR failure is
partial and does not invalidate the original hashed Preuve.

## Public Web

The exposed PageRenderer uses Firefox. A complete successful Web Capture
contains rendered DOM, semantic Markdown, a screenshot, provenance and
inventoried Découvertes. The protected binary acquirer is used only by an
explicit `capture continue` request.

The standalone renderer test validates both Firefox and Chromium contracts,
but the current Contrat agent does not expose a Chromium selector or automatic
fallback. WebKit and Lightpanda are not supported Providers. Never advertise or
select them.

Authenticated pages, cookies, paywalls, private networks and business
interactions are outside the supported scope.

## Public social posts

Public Instagram and LinkedIn publication URLs are routed to dedicated
Providers before the generic PageRenderer. Instagram uses `yt-dlp` metadata;
LinkedIn uses structured page metadata and the protected binary acquirer.
Both Providers preserve the post caption and each acquired media item in
publication order.

Every acquired social video is then processed locally: `ffmpeg` extracts its
audio, `whisper-cli` produces a transcription, and `ffmpeg`/`ffprobe` extract
keyframes. Any unavailable capability yields a `partial` Capture while the
caption and successfully acquired media remain usable. Captions and
transcriptions are included in `capture search`.

Do not select Playwright for a supported social post. A dedicated Provider can
still fail when the post is private, deleted, login-gated or its media URLs are
unavailable; report the resulting terminal or partial state without fallback.

## Local Dérivés

`scriptor-local-derive` supports `structured-summary`, `proven-claims`,
`checklist`, `markdown-note` and `sourced-answer`. It receives only the selected
verified Références and returns one immutable Dérivé.

The Provider engine is not distributed with Scriptor and must already exist in
`PATH`. Its absence is a terminal Job error, not permission to install a model,
call a remote Provider or synthesize the Dérivé directly.

Requested and effective parameters use a closed non-secret schema:

- text: `language`, `model`, `model_sha256`, `style`;
- unsigned integer: `max_tokens`, `seed`;
- number: `temperature`, `top_p`.

Any other field or nested value is refused before persistence.
