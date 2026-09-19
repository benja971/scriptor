use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::{Job, Policy};

pub(super) enum Source {
    Local(PathBuf),
    Web(String),
}

impl Source {
    pub(super) fn into_job_source(self) -> String {
        match self {
            Self::Local(path) => path.to_string_lossy().into_owned(),
            Self::Web(url) => url,
        }
    }
}

pub(super) fn admit(source: &Path, policy: &Policy) -> Result<Source> {
    match policy_kind(policy)? {
        PolicyKind::Local => admit_local(source),
        PolicyKind::Web => admit_web(source, policy),
    }
}

pub(super) fn admit_job(job: &Job) -> Result<Source> {
    admit(Path::new(&job.source), &job.policy)
}

fn admit_local(source: &Path) -> Result<Source> {
    let source = source
        .canonicalize()
        .with_context(|| format!("resolving local Source {}", source.display()))?;
    if !source.is_file() {
        bail!("local Source must be a regular file: {}", source.display());
    }
    Ok(Source::Local(source))
}

fn admit_web(source: &Path, policy: &Policy) -> Result<Source> {
    const SAFE_WEB_PROVIDERS: &[&str] = &[
        "ffmpeg",
        "ffprobe",
        "whisper-cli",
        "pdftotext",
        "pdfinfo",
        "tesseract",
        "page-renderer",
        "lightpanda",
        "instagram-provider",
        "linkedin-provider",
        "scriptor-binary-acquirer",
        "yt-dlp",
    ];
    if policy
        .snapshot
        .allowed_providers
        .iter()
        .any(|provider| !SAFE_WEB_PROVIDERS.contains(&provider.as_str()))
    {
        bail!("safe-web@1 has an unsupported Provider");
    }
    let url = source.to_str().context("Web Source must be valid UTF-8")?;
    crate::web::validate_public_url(url)?;
    Ok(Source::Web(url.to_string()))
}

enum PolicyKind {
    Local,
    Web,
}

fn policy_kind(policy: &Policy) -> Result<PolicyKind> {
    let (kind, allows_remote_calls) = match (policy.id.as_str(), policy.version) {
        ("safe-local", 1) => (PolicyKind::Local, false),
        ("safe-web", 1) => (PolicyKind::Web, true),
        _ => bail!("unsupported Policy {}@{}", policy.id, policy.version),
    };
    if policy.snapshot.allows_remote_calls != allows_remote_calls {
        bail!(
            "Policy {}@{} has an inconsistent remote-call admission",
            policy.id,
            policy.version
        );
    }
    Ok(kind)
}
