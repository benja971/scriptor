use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use url::Url;

use super::{
    Acquisition, AcquisitionResult, Capability, Extraction, Job, Locator, Manifest,
    PreparedCapture, Proof, Provider, ProviderDependency, RemoteMediaProvenance, RemoteProvenance,
    SourceIdentity, now_secs, read_job, sha256_bytes, sha256_file,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SocialPlatform {
    Instagram,
    LinkedIn,
}

pub(super) fn classify_url(url: &str) -> Option<SocialPlatform> {
    match Url::parse(url)
        .ok()?
        .host_str()?
        .to_ascii_lowercase()
        .as_str()
    {
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
    fn source_hash(&self, _: &Path) -> Result<AcquisitionResult<String>> {
        Ok(AcquisitionResult::Ready(sha256_bytes(self.url.as_bytes())))
    }
    fn acquire(
        &self,
        capture: &super::publication::StagedCapture,
        source_hash: &str,
    ) -> Result<AcquisitionResult<PreparedCapture>> {
        if self.platform == SocialPlatform::LinkedIn {
            return acquire_linkedin(self.job, self.url, capture, source_hash);
        }
        if !super::policy_allows(&self.job.policy, "instagram-provider") {
            bail!("Provider `instagram-provider` is not allowed by Policy");
        }
        fs::create_dir_all(capture.staging().join("proofs"))
            .context("creating Instagram Proof directory")?;
        fs::create_dir_all(capture.staging().join("extractions"))
            .context("creating Instagram Extraction directory")?;
        let version = Command::new("yt-dlp")
            .arg("--version")
            .output()
            .context("launching yt-dlp --version")?;
        if !version.status.success() {
            bail!("yt-dlp --version failed");
        }
        let version = String::from_utf8_lossy(&version.stdout).trim().to_string();
        let output = Command::new("yt-dlp")
            .args([
                "--dump-single-json",
                "--ignore-no-formats-error",
                "--no-playlist",
                self.url,
            ])
            .output()
            .context("launching yt-dlp Instagram metadata extraction")?;
        if !output.status.success() {
            bail!(
                "yt-dlp Instagram metadata extraction failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let metadata: Value =
            serde_json::from_slice(&output.stdout).context("parsing yt-dlp Instagram metadata")?;
        let provider = Provider {
            name: "instagram-provider".to_string(),
            version: "1".to_string(),
            parameters: json!({"yt_dlp_version":version, "dump_single_json":true, "ignore_no_formats_error":true}),
            dependencies: vec![ProviderDependency {
                name: "yt-dlp".to_string(),
                version,
            }],
        };
        let metadata_relative = "proofs/instagram-metadata.json";
        let metadata_path = capture.staging().join(metadata_relative);
        fs::write(
            &metadata_path,
            serde_json::to_vec_pretty(&metadata).context("serializing Instagram metadata proof")?,
        )
        .context("writing Instagram metadata proof")?;
        let metadata_proof = proof(
            capture.staging(),
            "proof-instagram-metadata",
            metadata_relative,
            "application/json",
            None,
        )?;
        let caption = metadata
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        let caption_relative = "extractions/caption.txt";
        let caption_path = capture.staging().join(caption_relative);
        fs::write(&caption_path, caption).context("writing Instagram caption")?;
        let caption_extraction = Extraction {
            artifact_id: "extraction-caption".to_string(),
            path: caption_relative.to_string(),
            mime: "text/plain".to_string(),
            sha256: sha256_file(&caption_path)?,
            size_bytes: fs::metadata(&caption_path)
                .context("reading Instagram caption metadata")?
                .len(),
            locator: Some(Locator::Url {
                value: self.url.to_string(),
            }),
            locator_provider: None,
            provider: provider.clone(),
            proof_artifact_id: metadata_proof.artifact_id.clone(),
            created_at: now_secs(),
        };
        let image_url = image_url(&metadata).context("Instagram metadata has no photo URL")?;
        let deadline = UNIX_EPOCH
            .checked_add(Duration::from_secs(
                self.job
                    .created_at
                    .saturating_add(self.job.policy.snapshot.limits.duration_limit_secs),
            ))
            .context("calculating Instagram acquisition deadline")?;
        let acquired = crate::web::acquire_binary(
            image_url,
            capture.staging(),
            deadline,
            self.job.policy.snapshot.limits.disk_byte_limit,
            self.job.policy.snapshot.limits.download_byte_limit,
            || Ok(read_job(&self.job.id)?.state == "cancelled"),
        )?;
        let media_relative = format!("proofs/media-0.{}", extension(&acquired.mime));
        fs::copy(&acquired.path, capture.staging().join(&media_relative))
            .context("materializing Instagram photo")?;
        let media = proof(
            capture.staging(),
            "media-0",
            &media_relative,
            &acquired.mime,
            Some(0),
        )?;
        let canonical_url = metadata
            .get("webpage_url")
            .and_then(Value::as_str)
            .map(str::to_string);
        let final_url = canonical_url
            .clone()
            .unwrap_or_else(|| self.url.to_string());
        let manifest = Manifest {
            capture_id: capture.capture_id().to_string(),
            source: SourceIdentity {
                locator: final_url.clone(),
                sha256: source_hash.to_string(),
            },
            policy: self.job.policy.clone(),
            published_at: now_secs(),
            proof: metadata_proof.clone(),
            extractions: vec![caption_extraction],
            capabilities: vec![
                success("metadata", provider.clone()),
                success("caption", provider.clone()),
                success("media-0", provider.clone()),
            ],
            artifacts: vec![media],
            discoveries: Vec::new(),
            remote_provenance: Some(RemoteProvenance {
                requested_url: self.url.to_string(),
                final_url,
                mime: "application/json".to_string(),
                sha256: metadata_proof.sha256.clone(),
                size_bytes: metadata_proof.size_bytes,
                redirect_chain: vec![self.url.to_string()],
                canonical_url,
                platform: Some("instagram".to_string()),
                post_id: metadata
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                author: metadata
                    .get("uploader")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                published_at: metadata
                    .get("upload_date")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                media: vec![RemoteMediaProvenance {
                    order: 0,
                    requested_url: acquired.requested_url,
                    final_url: acquired.final_url,
                    mime: acquired.mime,
                    sha256: acquired.sha256,
                    size_bytes: acquired.size_bytes,
                    redirect_chain: acquired.redirect_chain,
                }],
            }),
        };
        Ok(AcquisitionResult::Ready(PreparedCapture::new(
            manifest, false,
        )))
    }
}

fn acquire_linkedin(
    job: &Job,
    url: &str,
    capture: &super::publication::StagedCapture,
    source_hash: &str,
) -> Result<AcquisitionResult<PreparedCapture>> {
    if !super::policy_allows(&job.policy, "linkedin-provider") {
        bail!("Provider `linkedin-provider` is not allowed by Policy");
    }
    fs::create_dir_all(capture.staging().join("proofs"))
        .context("creating LinkedIn Proof directory")?;
    fs::create_dir_all(capture.staging().join("extractions"))
        .context("creating LinkedIn Extraction directory")?;
    let deadline = acquisition_deadline(job)?;
    let metadata = crate::web::acquire_binary(
        url,
        capture.staging(),
        deadline,
        job.policy.snapshot.limits.disk_byte_limit,
        job.policy.snapshot.limits.download_byte_limit,
        || Ok(read_job(&job.id)?.state == "cancelled"),
    )?;
    let raw_metadata = fs::read(&metadata.path).context("reading LinkedIn structured metadata")?;
    let post = linkedin_post(&raw_metadata).context("parsing LinkedIn structured metadata")?;
    let provider = Provider {
        name: "linkedin-provider".to_string(),
        version: "1".to_string(),
        parameters: json!({"anonymous_get":true, "structured_data":"json-ld"}),
        dependencies: vec![ProviderDependency {
            name: "scriptor-binary-acquirer".to_string(),
            version: "1".to_string(),
        }],
    };
    let metadata_relative = "proofs/linkedin-metadata.html";
    let metadata_path = capture.staging().join(metadata_relative);
    fs::write(&metadata_path, raw_metadata).context("writing LinkedIn metadata proof")?;
    let metadata_proof = proof(
        capture.staging(),
        "proof-linkedin-metadata",
        metadata_relative,
        &metadata.mime,
        None,
    )?;
    let caption_relative = "extractions/caption.txt";
    let caption_path = capture.staging().join(caption_relative);
    fs::write(&caption_path, &post.caption).context("writing LinkedIn caption")?;
    let caption_extraction = Extraction {
        artifact_id: "extraction-caption".to_string(),
        path: caption_relative.to_string(),
        mime: "text/plain".to_string(),
        sha256: sha256_file(&caption_path)?,
        size_bytes: fs::metadata(&caption_path)
            .context("reading LinkedIn caption metadata")?
            .len(),
        locator: Some(Locator::Url {
            value: url.to_string(),
        }),
        locator_provider: None,
        provider: provider.clone(),
        proof_artifact_id: metadata_proof.artifact_id.clone(),
        created_at: now_secs(),
    };
    let acquired = crate::web::acquire_binary(
        &post.image_url,
        capture.staging(),
        deadline,
        job.policy.snapshot.limits.disk_byte_limit,
        job.policy.snapshot.limits.download_byte_limit,
        || Ok(read_job(&job.id)?.state == "cancelled"),
    )?;
    let media_relative = format!("proofs/media-0.{}", extension(&acquired.mime));
    fs::copy(&acquired.path, capture.staging().join(&media_relative))
        .context("materializing LinkedIn photo")?;
    let media = proof(
        capture.staging(),
        "media-0",
        &media_relative,
        &acquired.mime,
        Some(0),
    )?;
    let final_url = post
        .canonical_url
        .clone()
        .unwrap_or_else(|| metadata.final_url.clone());
    let manifest = Manifest {
        capture_id: capture.capture_id().to_string(),
        source: SourceIdentity {
            locator: final_url.clone(),
            sha256: source_hash.to_string(),
        },
        policy: job.policy.clone(),
        published_at: now_secs(),
        proof: metadata_proof.clone(),
        extractions: vec![caption_extraction],
        capabilities: vec![
            success("metadata", provider.clone()),
            success("caption", provider.clone()),
            success("media-0", provider.clone()),
        ],
        artifacts: vec![media],
        discoveries: Vec::new(),
        remote_provenance: Some(RemoteProvenance {
            requested_url: url.to_string(),
            final_url,
            mime: metadata.mime,
            sha256: metadata_proof.sha256.clone(),
            size_bytes: metadata_proof.size_bytes,
            redirect_chain: metadata.redirect_chain,
            canonical_url: post.canonical_url,
            platform: Some("linkedin".to_string()),
            post_id: post.id,
            author: post.author,
            published_at: post.published_at,
            media: vec![RemoteMediaProvenance {
                order: 0,
                requested_url: acquired.requested_url,
                final_url: acquired.final_url,
                mime: acquired.mime,
                sha256: acquired.sha256,
                size_bytes: acquired.size_bytes,
                redirect_chain: acquired.redirect_chain,
            }],
        }),
    };
    Ok(AcquisitionResult::Ready(PreparedCapture::new(
        manifest, false,
    )))
}

struct LinkedInPost {
    caption: String,
    image_url: String,
    canonical_url: Option<String>,
    id: Option<String>,
    author: Option<String>,
    published_at: Option<String>,
}

fn acquisition_deadline(job: &Job) -> Result<std::time::SystemTime> {
    UNIX_EPOCH
        .checked_add(Duration::from_secs(
            job.created_at
                .saturating_add(job.policy.snapshot.limits.duration_limit_secs),
        ))
        .context("calculating social acquisition deadline")
}

fn linkedin_post(raw: &[u8]) -> Result<LinkedInPost> {
    let raw = std::str::from_utf8(raw).context("decoding LinkedIn metadata as UTF-8")?;
    let json = linkedin_json_ld(raw).context("finding LinkedIn JSON-LD")?;
    let value: Value = serde_json::from_str(json).context("decoding LinkedIn JSON-LD")?;
    let post = find_linkedin_post(&value).context("finding LinkedIn post in JSON-LD")?;
    let caption = json_string(post, "articleBody")
        .or_else(|| json_string(post, "description"))
        .unwrap_or_default()
        .to_string();
    let image_url = json_image_url(post).context("LinkedIn JSON-LD has no publication image")?;
    Ok(LinkedInPost {
        caption,
        image_url: image_url.to_string(),
        canonical_url: json_string(post, "url").map(str::to_string),
        id: json_string(post, "identifier").map(str::to_string),
        author: post.get("author").and_then(json_name).map(str::to_string),
        published_at: json_string(post, "datePublished").map(str::to_string),
    })
}

fn linkedin_json_ld(html: &str) -> Option<&str> {
    let marker = "application/ld+json";
    let type_offset = html.find(marker)?;
    let contents = html.get(type_offset..)?;
    let start = contents.find('>')?.checked_add(type_offset + 1)?;
    let end = html.get(start..)?.find("</script>")?.checked_add(start)?;
    html.get(start..end).map(str::trim)
}

fn find_linkedin_post(value: &Value) -> Option<&Value> {
    match value {
        Value::Object(object) => {
            if object.contains_key("articleBody") || object.contains_key("image") {
                Some(value)
            } else {
                object.values().find_map(find_linkedin_post)
            }
        }
        Value::Array(values) => values.iter().find_map(find_linkedin_post),
        _ => None,
    }
}

fn json_string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn json_image_url(value: &Value) -> Option<&str> {
    match value.get("image")? {
        Value::String(url) => Some(url),
        Value::Object(image) => image.get("url").and_then(Value::as_str),
        _ => None,
    }
}

fn json_name(value: &Value) -> Option<&str> {
    match value {
        Value::String(name) => Some(name),
        Value::Object(author) => author.get("name").and_then(Value::as_str),
        _ => None,
    }
}

fn image_url(metadata: &Value) -> Option<&str> {
    metadata.get("url").and_then(Value::as_str).or_else(|| {
        metadata
            .get("thumbnails")
            .and_then(Value::as_array)?
            .last()?
            .get("url")?
            .as_str()
    })
}
fn extension(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        _ => "bin",
    }
}
fn proof(
    staging: &Path,
    artifact_id: &str,
    path: &str,
    mime: &str,
    order: Option<u32>,
) -> Result<Proof> {
    let file = staging.join(path);
    Ok(Proof {
        artifact_id: artifact_id.to_string(),
        path: path.to_string(),
        mime: mime.to_string(),
        sha256: sha256_file(&file)?,
        size_bytes: fs::metadata(&file)
            .with_context(|| format!("reading {} metadata", file.display()))?
            .len(),
        locator: Locator::Url {
            value: path.to_string(),
        },
        created_at: now_secs(),
        order,
    })
}
fn success(name: &str, provider: Provider) -> Capability {
    Capability {
        name: name.to_string(),
        state: "succeeded".to_string(),
        provider,
        error: None,
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
    }
}
