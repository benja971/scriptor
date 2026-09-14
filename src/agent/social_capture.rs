use std::path::Path;

use anyhow::{Result, bail};
use url::Url;

use super::{Acquisition, AcquisitionResult, Job, PreparedCapture, sha256_bytes};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SocialPlatform {
    Instagram,
    LinkedIn,
}

pub(super) fn classify_url(url: &str) -> Option<SocialPlatform> {
    let parsed = Url::parse(url).ok()?;
    match parsed.host_str()?.to_ascii_lowercase().as_str() {
        "instagram.com" | "www.instagram.com" => Some(SocialPlatform::Instagram),
        "linkedin.com" | "www.linkedin.com" => Some(SocialPlatform::LinkedIn),
        _ => None,
    }
}

pub(super) struct SocialAcquisition<'a> {
    pub(super) job: &'a Job,
    pub(super) url: &'a str,
    pub(super) platform: SocialPlatform,
}

impl Acquisition for SocialAcquisition<'_> {
    fn source_hash(&self, _staging: &Path) -> Result<AcquisitionResult<String>> {
        Ok(AcquisitionResult::Ready(sha256_bytes(self.url.as_bytes())))
    }

    fn acquire(
        &self,
        _capture: &super::publication::StagedCapture,
        _source_hash: &str,
    ) -> Result<AcquisitionResult<PreparedCapture>> {
        let provider = match self.platform {
            SocialPlatform::Instagram => "instagram-provider",
            SocialPlatform::LinkedIn => "linkedin-provider",
        };
        if !super::policy_allows(&self.job.policy, provider) {
            bail!("Provider `{provider}` is not allowed by Policy");
        }
        bail!("social capture provider `{provider}` is not implemented")
    }
}

#[cfg(test)]
mod tests {
    use super::{SocialPlatform, classify_url};

    #[test]
    fn classifies_supported_hosts_case_insensitively() {
        assert_eq!(
            classify_url("https://WWW.Instagram.com:443/p/example"),
            Some(SocialPlatform::Instagram)
        );
        assert_eq!(
            classify_url("https://linkedin.com/posts/example"),
            Some(SocialPlatform::LinkedIn)
        );
    }

    #[test]
    fn does_not_match_subdomains_or_paths() {
        assert_eq!(
            classify_url("https://instagram.com.example/p/example"),
            None
        );
        assert_eq!(
            classify_url("https://example.com/instagram.com/p/example"),
            None
        );
    }
}
