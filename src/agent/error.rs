use std::fmt::{Display, Formatter};

use anyhow::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentErrorCode {
    DownloadBudgetExceeded,
    DiskBudgetExceeded,
    PortRefused,
    PrivateTargetRefused,
    RendererFirefoxUnavailable,
    RendererChromiumUnavailable,
    RendererUnknown,
    UrlCredentialsRefused,
    UrlHostRefused,
    UrlSchemeRefused,
    WebsocketRefused,
    ProxySchemeRefused,
    ProxyTargetRefused,
    DnsResolutionFailed,
}

impl AgentErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DownloadBudgetExceeded => "web_download_budget_exceeded",
            Self::DiskBudgetExceeded => "web_disk_budget_exceeded",
            Self::PortRefused => "web_port_refused",
            Self::PrivateTargetRefused => "web_private_target_refused",
            Self::RendererFirefoxUnavailable => "web_renderer_firefox_unavailable",
            Self::RendererChromiumUnavailable => "web_renderer_chromium_unavailable",
            Self::RendererUnknown => "web_renderer_unknown",
            Self::UrlCredentialsRefused => "web_url_credentials_refused",
            Self::UrlHostRefused => "web_url_host_refused",
            Self::UrlSchemeRefused => "web_url_scheme_refused",
            Self::WebsocketRefused => "web_websocket_refused",
            Self::ProxySchemeRefused => "web_proxy_scheme_refused",
            Self::ProxyTargetRefused => "web_proxy_target_refused",
            Self::DnsResolutionFailed => "web_dns_resolution_failed",
        }
    }

    pub fn from_renderer_code(value: &str) -> Option<Self> {
        match value {
            "web_download_budget_exceeded" => Some(Self::DownloadBudgetExceeded),
            "web_disk_budget_exceeded" => Some(Self::DiskBudgetExceeded),
            "web_port_refused" => Some(Self::PortRefused),
            "web_private_target_refused" => Some(Self::PrivateTargetRefused),
            "web_renderer_firefox_unavailable" => Some(Self::RendererFirefoxUnavailable),
            "web_renderer_chromium_unavailable" => Some(Self::RendererChromiumUnavailable),
            "web_renderer_unknown" => Some(Self::RendererUnknown),
            "web_url_credentials_refused" => Some(Self::UrlCredentialsRefused),
            "web_url_scheme_refused" => Some(Self::UrlSchemeRefused),
            "web_websocket_refused" => Some(Self::WebsocketRefused),
            "web_proxy_scheme_refused" => Some(Self::ProxySchemeRefused),
            "web_proxy_target_refused" => Some(Self::ProxyTargetRefused),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct CodedError {
    pub(super) code: AgentErrorCode,
    message: String,
}

impl Display for CodedError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CodedError {}

pub fn coded_error(code: AgentErrorCode, message: impl Into<String>) -> Error {
    CodedError {
        code,
        message: message.into(),
    }
    .into()
}
