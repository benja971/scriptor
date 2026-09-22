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
use crate::resource::ResourceBudget;

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
    #[allow(clippy::too_many_lines)]
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
        let is_cancelled = || Ok(read_job(&self.job.id)?.state == "cancelled");
        let budget = ResourceBudget::new(
            capture.staging(),
            self.job.created_at,
            self.job.policy.snapshot.limits.duration_limit_secs,
            self.job.policy.snapshot.limits.disk_byte_limit,
        )
        .with_cancellation(&is_cancelled);
        let mut version_command = Command::new("yt-dlp");
        version_command.args(["--ignore-config", "--version"]);
        let version = budget
            .output(&mut version_command)
            .context("launching yt-dlp --version")?;
        if !version.status.success() {
            bail!("yt-dlp --version failed");
        }
        let version = String::from_utf8_lossy(&version.stdout).trim().to_string();
        let mut metadata_command = Command::new("yt-dlp");
        metadata_command
            .args([
                "--ignore-config",
                "--skip-download",
                "--dump-single-json",
                "--ignore-no-formats-error",
            ])
            .arg(self.url);
        let output = budget
            .output(&mut metadata_command)
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
            parameters: json!({"yt_dlp_version":version, "ignore_config":true, "skip_download":true, "dump_single_json":true, "ignore_no_formats_error":true}),
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
        let media_urls = instagram_media_urls(&metadata)?;
        let deadline = UNIX_EPOCH
            .checked_add(Duration::from_secs(
                self.job
                    .created_at
                    .saturating_add(self.job.policy.snapshot.limits.duration_limit_secs),
            ))
            .context("calculating Instagram acquisition deadline")?;
        let mut artifacts = Vec::new();
        let mut media_provenance = Vec::new();
        let mut videos = Vec::new();
        let mut capabilities = vec![
            success("metadata", provider.clone()),
            success("caption", provider.clone()),
        ];
        let mut partial = false;
        let mut downloaded_bytes = 0_u64;
        for (index, media_url) in media_urls.iter().enumerate() {
            let order = u32::try_from(index).context("converting Instagram media order")?;
            let acquisition_staging = capture.staging().join(format!("media-acquisition-{order}"));
            let download_limit = self
                .job
                .policy
                .snapshot
                .limits
                .download_byte_limit
                .saturating_sub(downloaded_bytes);
            let acquired = if download_limit == 0 {
                Err(anyhow::anyhow!(
                    "Capture exceeds safe-web@1 download budget"
                ))
            } else {
                crate::web::acquire_binary(
                    media_url,
                    &acquisition_staging,
                    deadline,
                    self.job.policy.snapshot.limits.disk_byte_limit,
                    download_limit,
                    || Ok(read_job(&self.job.id)?.state == "cancelled"),
                )
            };
            let acquired = match acquired {
                Ok(acquired) => acquired,
                Err(error) => {
                    partial = true;
                    capabilities.push(super::failed_capability(
                        &format!("media-{order}"),
                        provider.clone(),
                        &error,
                    ));
                    continue;
                }
            };
            downloaded_bytes = downloaded_bytes
                .checked_add(acquired.size_bytes)
                .context("summing Instagram download budget")?;
            let media_relative = format!("proofs/media-{order}.{}", extension(&acquired.mime));
            fs::copy(&acquired.path, capture.staging().join(&media_relative))
                .context("materializing Instagram media")?;
            if acquired.mime.starts_with("video/") {
                videos.push((order, capture.staging().join(&media_relative)));
            }
            artifacts.push(proof(
                capture.staging(),
                &format!("media-{order}"),
                &media_relative,
                &acquired.mime,
                Some(order),
            )?);
            capabilities.push(success(&format!("media-{order}"), provider.clone()));
            media_provenance.push(RemoteMediaProvenance {
                order,
                requested_url: acquired.requested_url,
                final_url: acquired.final_url,
                mime: acquired.mime,
                sha256: acquired.sha256,
                size_bytes: acquired.size_bytes,
                redirect_chain: acquired.redirect_chain,
            });
        }
        if artifacts.is_empty() {
            partial = true;
        }
        let canonical_url = metadata
            .get("webpage_url")
            .and_then(Value::as_str)
            .map(str::to_string);
        let final_url = canonical_url
            .clone()
            .unwrap_or_else(|| self.url.to_string());
        let mut manifest = Manifest {
            format_version: 1,
            capture_id: capture.capture_id().to_string(),
            capture_version: 1,
            source: SourceIdentity {
                locator: final_url.clone(),
                sha256: source_hash.to_string(),
            },
            policy: self.job.policy.clone(),
            published_at: now_secs(),
            proof: metadata_proof.clone(),
            extractions: vec![caption_extraction],
            capabilities,
            artifacts,
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
                account_handle: metadata
                    .get("channel")
                    .or_else(|| metadata.get("uploader_id"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                published_at: metadata
                    .get("upload_date")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                media: media_provenance,
            }),
        };
        if enrich_video_extractions(self.job, capture.staging(), &mut manifest, &videos)? {
            return Ok(AcquisitionResult::Cancelled);
        }
        partial |= manifest
            .capabilities
            .iter()
            .any(|capability| capability.state != "succeeded");
        Ok(AcquisitionResult::Ready(PreparedCapture::new(
            manifest, partial,
        )))
    }
}

#[allow(clippy::too_many_lines)]
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
    let mut downloaded_bytes = 0_u64;
    let metadata = crate::web::acquire_binary(
        url,
        capture.staging(),
        deadline,
        job.policy.snapshot.limits.disk_byte_limit,
        job.policy.snapshot.limits.download_byte_limit,
        || Ok(read_job(&job.id)?.state == "cancelled"),
    )?;
    downloaded_bytes = downloaded_bytes
        .checked_add(metadata.size_bytes)
        .context("summing LinkedIn download budget")?;
    let raw_metadata = fs::read(&metadata.path).context("reading LinkedIn structured metadata")?;
    let mut post = linkedin_post(&raw_metadata).context("parsing LinkedIn structured metadata")?;
    if post.media_urls.is_empty() {
        post.media_urls = linkedin_document_media_urls(
            job,
            capture.staging(),
            deadline,
            &raw_metadata,
            &mut downloaded_bytes,
        )?;
    }
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
    let mut artifacts = Vec::new();
    let mut media_provenance = Vec::new();
    let mut videos = Vec::new();
    let mut capabilities = vec![
        success("metadata", provider.clone()),
        success("caption", provider.clone()),
    ];
    let mut partial = false;
    for (index, media_url) in post.media_urls.iter().enumerate() {
        let order = u32::try_from(index).context("converting LinkedIn media order")?;
        let acquisition_staging = capture.staging().join(format!("media-acquisition-{order}"));
        let download_limit = job
            .policy
            .snapshot
            .limits
            .download_byte_limit
            .saturating_sub(downloaded_bytes);
        let acquired = if download_limit == 0 {
            Err(anyhow::anyhow!(
                "Capture exceeds safe-web@1 download budget"
            ))
        } else {
            crate::web::acquire_binary(
                media_url,
                &acquisition_staging,
                deadline,
                job.policy.snapshot.limits.disk_byte_limit,
                download_limit,
                || Ok(read_job(&job.id)?.state == "cancelled"),
            )
        };
        let acquired = match acquired {
            Ok(acquired) => acquired,
            Err(error) => {
                partial = true;
                capabilities.push(super::failed_capability(
                    &format!("media-{order}"),
                    provider.clone(),
                    &error,
                ));
                continue;
            }
        };
        downloaded_bytes = downloaded_bytes
            .checked_add(acquired.size_bytes)
            .context("summing LinkedIn download budget")?;
        let media_relative = format!("proofs/media-{order}.{}", extension(&acquired.mime));
        fs::copy(&acquired.path, capture.staging().join(&media_relative))
            .context("materializing LinkedIn media")?;
        if acquired.mime.starts_with("video/") {
            videos.push((order, capture.staging().join(&media_relative)));
        }
        artifacts.push(proof(
            capture.staging(),
            &format!("media-{order}"),
            &media_relative,
            &acquired.mime,
            Some(order),
        )?);
        capabilities.push(success(&format!("media-{order}"), provider.clone()));
        media_provenance.push(RemoteMediaProvenance {
            order,
            requested_url: acquired.requested_url,
            final_url: acquired.final_url,
            mime: acquired.mime,
            sha256: acquired.sha256,
            size_bytes: acquired.size_bytes,
            redirect_chain: acquired.redirect_chain,
        });
    }
    if artifacts.is_empty() {
        bail!("LinkedIn structured metadata media acquisition failed");
    }
    let final_url = post
        .canonical_url
        .clone()
        .unwrap_or_else(|| metadata.final_url.clone());
    let mut manifest = Manifest {
        format_version: 1,
        capture_id: capture.capture_id().to_string(),
        capture_version: 1,
        source: SourceIdentity {
            locator: final_url.clone(),
            sha256: source_hash.to_string(),
        },
        policy: job.policy.clone(),
        published_at: now_secs(),
        proof: metadata_proof.clone(),
        extractions: vec![caption_extraction],
        capabilities,
        artifacts,
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
            account_handle: None,
            published_at: post.published_at,
            media: media_provenance,
        }),
    };
    if enrich_video_extractions(job, capture.staging(), &mut manifest, &videos)? {
        return Ok(AcquisitionResult::Cancelled);
    }
    partial |= manifest
        .capabilities
        .iter()
        .any(|capability| capability.state != "succeeded");
    Ok(AcquisitionResult::Ready(PreparedCapture::new(
        manifest, partial,
    )))
}

fn enrich_video_extractions(
    job: &Job,
    staging: &Path,
    manifest: &mut Manifest,
    videos: &[(u32, std::path::PathBuf)],
) -> Result<bool> {
    let is_cancelled = || Ok(read_job(&job.id)?.state == "cancelled");
    let budget = ResourceBudget::new(
        staging,
        job.created_at,
        job.policy.snapshot.limits.duration_limit_secs,
        job.policy.snapshot.limits.disk_byte_limit,
    )
    .with_cancellation(&is_cancelled);
    for (order, path) in videos {
        super::capture_local_media(job, path, staging, manifest, &budget, Some(*order));
        if budget.is_cancelled()? {
            return Ok(true);
        }
    }
    super::enrich_image_ocr(job, staging, manifest, &budget)?;
    if budget.is_cancelled()? {
        return Ok(true);
    }
    Ok(false)
}

struct LinkedInPost {
    caption: String,
    media_urls: Vec<String>,
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
    let media_urls = linkedin_media_urls(post).unwrap_or_default();
    Ok(LinkedInPost {
        caption,
        media_urls,
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
    let type_end = type_offset.checked_add(1)?;
    let start = contents.find('>')?.checked_add(type_end)?;
    let end = html.get(start..)?.find("</script>")?.checked_add(start)?;
    html.get(start..end).map(str::trim)
}

fn find_linkedin_post(value: &Value) -> Option<&Value> {
    match value {
        Value::Object(object) => {
            if object.contains_key("articleBody")
                || object.contains_key("image")
                || object.contains_key("contentUrl")
            {
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

fn linkedin_media_urls(value: &Value) -> Option<Vec<String>> {
    let video = value.get("contentUrl").and_then(Value::as_str);
    let image = value.get("image");
    let urls = match image {
        Some(Value::Array(images)) => images
            .iter()
            .filter_map(json_image_url)
            .map(str::to_string)
            .collect(),
        Some(image) => json_image_url(image).map(|url| vec![url.to_string()])?,
        None => Vec::new(),
    };
    Some(if urls.is_empty() {
        video.map(|url| vec![url.to_string()])?
    } else {
        urls
    })
}

fn linkedin_document_media_urls(
    job: &Job,
    staging: &Path,
    deadline: std::time::SystemTime,
    html: &[u8],
    downloaded_bytes: &mut u64,
) -> Result<Vec<String>> {
    let html = std::str::from_utf8(html).context("decoding LinkedIn document configuration")?;
    let manifest_url = html_config_url(html, "manifestUrl")
        .context("LinkedIn document configuration has no manifest URL")?;
    let download_limit = job
        .policy
        .snapshot
        .limits
        .download_byte_limit
        .saturating_sub(*downloaded_bytes);
    if download_limit == 0 {
        bail!("Capture exceeds safe-web@1 download budget");
    }
    let manifest = crate::web::acquire_binary(
        &manifest_url,
        &staging.join("document-manifest"),
        deadline,
        job.policy.snapshot.limits.disk_byte_limit,
        download_limit,
        || Ok(read_job(&job.id)?.state == "cancelled"),
    )?;
    *downloaded_bytes = downloaded_bytes
        .checked_add(manifest.size_bytes)
        .context("summing LinkedIn download budget")?;
    let value: Value = serde_json::from_slice(
        &fs::read(&manifest.path).context("reading LinkedIn document manifest")?,
    )
    .context("parsing LinkedIn document manifest")?;
    let mut urls = value
        .get("transcribedDocumentUrl")
        .and_then(Value::as_str)
        .map(str::to_string)
        .into_iter()
        .collect::<Vec<_>>();
    let image_manifest_url = value
        .get("perResolutions")
        .and_then(Value::as_array)
        .and_then(|resolutions| {
            resolutions.iter().max_by_key(|resolution| {
                resolution
                    .get("width")
                    .and_then(Value::as_u64)
                    .unwrap_or_default()
            })
        })
        .and_then(|resolution| resolution.get("imageManifestUrl"))
        .and_then(Value::as_str);
    if let Some(image_manifest_url) = image_manifest_url {
        let download_limit = job
            .policy
            .snapshot
            .limits
            .download_byte_limit
            .saturating_sub(*downloaded_bytes);
        if download_limit == 0 {
            bail!("Capture exceeds safe-web@1 download budget");
        }
        let images = crate::web::acquire_binary(
            image_manifest_url,
            &staging.join("document-pages-manifest"),
            deadline,
            job.policy.snapshot.limits.disk_byte_limit,
            download_limit,
            || Ok(read_job(&job.id)?.state == "cancelled"),
        )?;
        *downloaded_bytes = downloaded_bytes
            .checked_add(images.size_bytes)
            .context("summing LinkedIn download budget")?;
        let pages: Value = serde_json::from_slice(
            &fs::read(&images.path).context("reading LinkedIn document pages manifest")?,
        )
        .context("parsing LinkedIn document pages manifest")?;
        collect_urls(&pages, &mut urls);
    }
    if urls.is_empty() {
        bail!("LinkedIn document manifest has no media URL");
    }
    Ok(urls)
}

fn html_config_url(html: &str, key: &str) -> Option<String> {
    let marker = format!("{key}&quot;:&quot;");
    let value = html.split_once(&marker)?.1.split_once("&quot;")?.0;
    Some(value.replace("&amp;", "&"))
}

fn collect_urls(value: &Value, urls: &mut Vec<String>) {
    match value {
        Value::String(url) if url.starts_with("https://") || url.starts_with("http://") => {
            if !urls.contains(url) {
                urls.push(url.clone());
            }
        }
        Value::Array(values) => values.iter().for_each(|value| collect_urls(value, urls)),
        Value::Object(values) => values.values().for_each(|value| collect_urls(value, urls)),
        _ => {}
    }
}

fn json_image_url(value: &Value) -> Option<&str> {
    match value {
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

fn instagram_media_urls(metadata: &Value) -> Result<Vec<&str>> {
    let entries = metadata.get("entries").and_then(Value::as_array);
    let values = entries.map_or_else(|| vec![metadata], |entries| entries.iter().collect());
    values
        .into_iter()
        .map(instagram_media_url)
        .collect::<Option<Vec<_>>>()
        .context("Instagram metadata has no media URL")
}

fn instagram_media_url(metadata: &Value) -> Option<&str> {
    metadata
        .get("url")
        .and_then(Value::as_str)
        .or_else(|| {
            metadata
                .get("formats")
                .and_then(Value::as_array)?
                .iter()
                .find(|format| format.get("vcodec").and_then(Value::as_str) != Some("none"))?
                .get("url")?
                .as_str()
        })
        .or_else(|| {
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
        "video/mp4" => "mp4",
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
