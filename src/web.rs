use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use url::Url;

#[derive(Debug, Deserialize)]
pub struct Provenance {
    pub final_url: String,
}

pub fn capture(url: &str, staging: &Path) -> Result<Provenance> {
    validate_public_url(url)?;
    let output = Command::new("scriptor-page-renderer")
        .arg("--url")
        .arg(url)
        .arg("--output-dir")
        .arg(staging)
        .output()
        .context("launching Playwright page renderer")?;
    if !output.status.success() {
        bail!(
            "page renderer failed (exit code {:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let provenance: Provenance = serde_json::from_slice(
        &fs::read(staging.join("provenance.json")).context("reading Web provenance")?,
    )
    .context("parsing Web provenance")?;
    validate_public_url(&provenance.final_url).context("validating final redirect target")?;
    for path in [
        staging.join("proofs/dom.html"),
        staging.join("proofs/screenshot.png"),
        staging.join("extractions/page.md"),
        staging.join("discoveries.json"),
    ] {
        if !path.is_file() {
            bail!("page renderer did not produce {}", path.display());
        }
    }
    Ok(provenance)
}

pub fn validate_public_url(value: &str) -> Result<()> {
    let url = Url::parse(value).with_context(|| format!("parsing Web URL `{value}`"))?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("web_url_scheme_refused: only HTTP(S) URLs are permitted");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("web_url_credentials_refused: URLs with credentials are not permitted");
    }
    let host = url
        .host_str()
        .context("web_url_host_refused: URL has no host")?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        bail!("web_private_target_refused: localhost is not a public target");
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(ip) {
            bail!("web_private_target_refused: {ip} is not public");
        }
        return Ok(());
    }
    let port = url
        .port_or_known_default()
        .context("resolving Web URL port")?;
    let addresses = (host, port)
        .to_socket_addrs()
        .with_context(|| format!("web_dns_resolution_failed: resolving {host}"))?;
    let mut found = false;
    for address in addresses {
        found = true;
        if is_private_ip(address.ip()) {
            bail!("web_private_target_refused: {host} resolves to a non-public address");
        }
    }
    if !found {
        bail!("web_dns_resolution_failed: {host} has no address");
    }
    Ok(())
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_unspecified()
                || ip.is_documentation()
                || ip.is_multicast()
                || ip == Ipv4Addr::UNSPECIFIED
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.segments().get(..2) == Some(&[0x2001, 0x0db8])
                || ipv6_mapped_private(ip)
        }
    }
}

fn ipv6_mapped_private(ip: Ipv6Addr) -> bool {
    ip.to_ipv4_mapped()
        .is_some_and(|mapped| is_private_ip(IpAddr::V4(mapped)))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::validate_public_url;

    #[test]
    fn refuses_non_http_private_and_credentialed_urls_before_renderer() {
        for url in [
            "file:///etc/passwd",
            "http://127.0.0.1/",
            "http://[::1]/",
            "http://169.254.169.254/latest/meta-data/",
            "http://user:secret@example.com/",
        ] {
            assert!(validate_public_url(url).is_err(), "{url} must be refused");
        }
    }
}
