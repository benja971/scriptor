use clap::Parser;

/// Arguments du CLI `scriptor`.
#[allow(dead_code)]
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    /// Chemin de fichier local ou URL (Instagram, `TikTok`, `YouTube`, lien direct...).
    pub source: String,
}

/// Distinction Source locale / Source distante, déterminée à partir du préfixe
/// `http://`/`https://` de l'argument fourni (cf. user story 18 de la spec).
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Local(String),
    Remote(String),
}

/// Détecte le type de Source à partir de la chaîne fournie en argument.
#[allow(dead_code)]
pub fn detect_source(input: &str) -> Source {
    if input.starts_with("http://") || input.starts_with("https://") {
        Source::Remote(input.to_string())
    } else {
        Source::Local(input.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_http_url_as_remote() {
        let source = detect_source("http://example.com/video.mp4");
        assert_eq!(
            source,
            Source::Remote("http://example.com/video.mp4".to_string())
        );
    }

    #[test]
    fn detects_https_url_as_remote() {
        let source = detect_source("https://www.youtube.com/watch?v=abc123");
        assert_eq!(
            source,
            Source::Remote("https://www.youtube.com/watch?v=abc123".to_string())
        );
    }

    #[test]
    fn detects_relative_path_as_local() {
        let source = detect_source("videos/interview.mp4");
        assert_eq!(source, Source::Local("videos/interview.mp4".to_string()));
    }

    #[test]
    fn detects_absolute_path_as_local() {
        let source = detect_source("/home/user/videos/interview.mp4");
        assert_eq!(
            source,
            Source::Local("/home/user/videos/interview.mp4".to_string())
        );
    }

    #[test]
    fn empty_string_is_local() {
        let source = detect_source("");
        assert_eq!(source, Source::Local(String::new()));
    }

    #[test]
    fn schemeless_url_like_path_is_local() {
        let source = detect_source("example.com/video.mp4");
        assert_eq!(source, Source::Local("example.com/video.mp4".to_string()));
    }
}
