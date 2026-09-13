use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use serde::Deserialize;
use url::Url;

use crate::resource::directory_size;

#[derive(Debug, Deserialize)]
pub struct Provenance {
    pub final_url: String,
}

pub enum Capture {
    Completed(Provenance),
    Cancelled,
}

pub fn capture<F>(
    url: &str,
    staging: &Path,
    deadline: SystemTime,
    disk_byte_limit: u64,
    download_byte_limit: u64,
    is_cancelled: F,
) -> Result<Capture>
where
    F: Fn() -> Result<bool>,
{
    validate_public_url(url)?;
    let mut command = Command::new("scriptor-page-renderer");
    command
        .process_group(0)
        .arg("--url")
        .arg(url)
        .arg("--output-dir")
        .arg(staging)
        .arg("--max-output-bytes")
        .arg(disk_byte_limit.to_string())
        .arg("--max-download-bytes")
        .arg(download_byte_limit.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .context("launching Playwright page renderer")?;
    loop {
        if is_cancelled()? {
            terminate_process_group(&child)?;
            let _ = child.wait();
            return Ok(Capture::Cancelled);
        }
        if SystemTime::now() >= deadline {
            terminate_process_group(&child)?;
            let _ = child.wait();
            bail!("Capture exceeds safe-web@1 duration budget");
        }
        if directory_size(staging)? > disk_byte_limit {
            terminate_process_group(&child)?;
            let _ = child.wait();
            bail!("Capture exceeds safe-web@1 disk budget");
        }
        if child
            .try_wait()
            .context("checking page renderer status")?
            .is_some()
        {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let output = child
        .wait_with_output()
        .context("collecting page renderer output")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if let Some(code) = renderer_error_code(&stderr) {
            return Err(crate::agent::coded_error(code, stderr.trim()));
        }
        bail!(
            "page renderer failed (exit code {:?}): {}",
            output.status.code(),
            stderr.trim()
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
    Ok(Capture::Completed(provenance))
}

fn renderer_error_code(stderr: &str) -> Option<&'static str> {
    [
        "web_download_budget_exceeded",
        "web_disk_budget_exceeded",
        "web_port_refused",
        "web_private_target_refused",
        "web_renderer_firefox_unavailable",
        "web_renderer_unknown",
        "web_url_credentials_refused",
        "web_url_scheme_refused",
        "web_websocket_refused",
    ]
    .into_iter()
    .find(|code| stderr.lines().any(|line| line.starts_with(code)))
}

fn terminate_process_group(child: &Child) -> Result<()> {
    let pid = i32::try_from(child.id()).context("converting page renderer PID")?;
    match killpg(Pid::from_raw(pid), Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(error).context("stopping page renderer process group"),
    }
}

pub fn validate_public_url(value: &str) -> Result<()> {
    let url = Url::parse(value).with_context(|| format!("parsing Web URL `{value}`"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(crate::agent::coded_error(
            "web_url_scheme_refused",
            "only HTTP(S) URLs are permitted",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(crate::agent::coded_error(
            "web_url_credentials_refused",
            "URLs with credentials are not permitted",
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| crate::agent::coded_error("web_url_host_refused", "URL has no host"))?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err(crate::agent::coded_error(
            "web_private_target_refused",
            "localhost is not a public target",
        ));
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(ip) {
            return Err(crate::agent::coded_error(
                "web_private_target_refused",
                format!("{ip} is not public"),
            ));
        }
        return Ok(());
    }
    let port = url
        .port_or_known_default()
        .context("resolving Web URL port")?;
    let addresses = (host, port).to_socket_addrs().map_err(|error| {
        crate::agent::coded_error("web_dns_resolution_failed", error.to_string())
    })?;
    let mut found = false;
    for address in addresses {
        found = true;
        if is_private_ip(address.ip()) {
            return Err(crate::agent::coded_error(
                "web_private_target_refused",
                format!("{host} resolves to a non-public address"),
            ));
        }
    }
    if !found {
        return Err(crate::agent::coded_error(
            "web_dns_resolution_failed",
            format!("{host} has no address"),
        ));
    }
    Ok(())
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_unspecified()
                || ip.is_documentation()
                || ip.is_multicast()
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                || (octets[0] == 198 && matches!(octets[1], 18 | 19))
                || ip == Ipv4Addr::UNSPECIFIED
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.segments().get(..2) == Some(&[0x2001, 0x0db8])
                || ipv6_embedded_private(ip)
        }
    }
}

fn ipv6_embedded_private(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    let compatible = segments.get(..6) == Some(&[0; 6]);
    let embedded = if compatible {
        let [high, low] = [segments[6], segments[7]];
        let [first, second] = high.to_be_bytes();
        let [third, fourth] = low.to_be_bytes();
        Some(Ipv4Addr::new(first, second, third, fourth))
    } else {
        ip.to_ipv4_mapped()
    };
    embedded.is_some_and(|address| is_private_ip(IpAddr::V4(address)))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{renderer_error_code, validate_public_url};

    #[test]
    fn refuses_non_http_private_and_credentialed_urls_before_renderer() {
        for url in [
            "file:///etc/passwd",
            "http://127.0.0.1/",
            "http://[::1]/",
            "http://[::127.0.0.1]/",
            "http://[::7f00:1]/",
            "http://[::ffff:169.254.169.254]/latest/meta-data/",
            "http://[::ffff:172.16.0.1]/",
            "http://169.254.169.254/latest/meta-data/",
            "http://100.64.0.1/",
            "http://198.18.0.1/",
            "http://user:secret@example.com/",
        ] {
            assert!(validate_public_url(url).is_err(), "{url} must be refused");
        }
    }

    #[test]
    fn preserves_machine_readable_renderer_refusals() {
        assert_eq!(
            renderer_error_code("web_private_target_refused\n"),
            Some("web_private_target_refused")
        );
    }
}
