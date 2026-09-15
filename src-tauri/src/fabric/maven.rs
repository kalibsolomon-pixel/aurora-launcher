//! Maven coordinates and repositories as official Fabric metadata uses them.
//!
//! Fabric metadata identifies libraries by Maven coordinate (`group:artifact:
//! version[:classifier]`) and a repository base URL, rather than by the fully
//! described artifact URLs Mojang metadata publishes. This module is the
//! minimum deterministic mapping from that representation to one artifact URL:
//!
//! ```text
//! group:name:version + repository base
//!         ↓
//! repository-relative Maven layout path
//!         ↓
//! artifact URL
//! ```
//!
//! This is deliberately *not* a Maven client: no POM parsing, no dependency
//! graph traversal, no repository search. Coordinates and repositories are
//! validated strictly (charset, shape, HTTPS) before any URL is constructed,
//! so no unvalidated remote string can influence a local or remote path.

use std::fmt;

use url::Url;

use crate::downloads::is_loopback_host;

/// A parsed Maven coordinate identifying one Fabric-provided library.
///
/// The accepted charset is deliberately conservative but includes `+`,
/// which official Fabric versions really use (for example
/// `net.fabricmc:sponge-mixin:0.17.4+mixin.0.8.7`). This type is separate
/// from the Mojang library coordinate in `minecraft::plan` on purpose: the
/// two metadata sources own their own validation rules, and neither becomes
/// the other's domain model.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MavenCoordinate {
    group: String,
    artifact: String,
    version: String,
    classifier: Option<String>,
}

impl MavenCoordinate {
    /// Parses a Maven coordinate from official Fabric metadata.
    ///
    /// Only three- or four-segment coordinates over the conservative charset
    /// are accepted, and no segment may be a traversal-shaped `.` or `..`.
    pub fn parse(coordinate: &str) -> Result<Self, InvalidMavenCoordinate> {
        let invalid = |reason: String| InvalidMavenCoordinate {
            coordinate: coordinate.to_owned(),
            reason,
        };

        let segments: Vec<&str> = coordinate.split(':').collect();
        if segments.len() < 3 || segments.len() > 4 {
            return Err(invalid(format!(
                "a Maven coordinate needs 3 or 4 segments, found {}",
                segments.len()
            )));
        }

        for segment in &segments {
            if segment.is_empty() {
                return Err(invalid("coordinate segments must not be empty".to_owned()));
            }
            let valid = segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'));
            if !valid {
                return Err(invalid(
                    "coordinate segments may only contain letters, digits, '.', '_', '-', and '+'"
                        .to_owned(),
                ));
            }
            if *segment == "." || *segment == ".." {
                return Err(invalid(
                    "coordinate segments must not be '.' or '..'".to_owned(),
                ));
            }
        }

        Ok(Self {
            group: segments[0].to_owned(),
            artifact: segments[1].to_owned(),
            version: segments[2].to_owned(),
            classifier: segments.get(3).map(|segment| (*segment).to_owned()),
        })
    }

    pub fn group(&self) -> &str {
        &self.group
    }

    pub fn artifact(&self) -> &str {
        &self.artifact
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn classifier(&self) -> Option<&str> {
        self.classifier.as_deref()
    }

    /// The canonical Maven coordinate string
    /// (`group:artifact:version[:classifier]`).
    pub fn as_maven_string(&self) -> String {
        match &self.classifier {
            Some(classifier) => {
                format!(
                    "{}:{}:{}:{classifier}",
                    self.group, self.artifact, self.version
                )
            }
            None => format!("{}:{}:{}", self.group, self.artifact, self.version),
        }
    }

    /// The repository-relative path this coordinate occupies in the Maven
    /// layout (`<group path>/<artifact>/<version>/<artifact>-<version>
    /// [-<classifier>].jar`).
    ///
    /// Because every coordinate segment is charset- and shape-validated, the
    /// derived path can never traverse or escape the repository.
    pub fn repository_path(&self) -> String {
        let file = match &self.classifier {
            Some(classifier) => {
                format!("{}-{}-{classifier}.jar", self.artifact, self.version)
            }
            None => format!("{}-{}.jar", self.artifact, self.version),
        };
        format!(
            "{}/{}/{}/{}",
            self.group.replace('.', "/"),
            self.artifact,
            self.version,
            file
        )
    }
}

impl fmt::Display for MavenCoordinate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.as_maven_string())
    }
}

/// A malformed Maven coordinate from official Fabric metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidMavenCoordinate {
    pub coordinate: String,
    pub reason: String,
}

impl fmt::Display for InvalidMavenCoordinate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Maven coordinate '{}' is invalid: {}",
            self.coordinate, self.reason
        )
    }
}

impl std::error::Error for InvalidMavenCoordinate {}

/// A validated Maven repository base URL.
///
/// Production repositories must use HTTPS; cleartext HTTP is accepted only
/// for explicit loopback hosts, matching the launcher's established
/// test-transport policy. The base must be a plain directory URL: no query,
/// no fragment, no embedded credentials, and a trailing path separator so
/// coordinate-derived paths always append beneath it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MavenRepository {
    base: Url,
}

impl MavenRepository {
    /// The official Fabric Maven repository, as pinned by the fabric-meta
    /// service itself for the loader and intermediary artifacts it adds to
    /// profiles (`Reference.FABRIC_MAVEN_URL`).
    pub const OFFICIAL_FABRIC_URL: &'static str = "https://maven.fabricmc.net/";

    pub fn parse(url_text: &str) -> Result<Self, InvalidMavenRepository> {
        let parsed = Url::parse(url_text).map_err(|_| InvalidMavenRepository {
            url: url_text.to_owned(),
            reason: "the URL is not valid".to_owned(),
        })?;

        if parsed.cannot_be_a_base() {
            return Err(InvalidMavenRepository {
                url: url_text.to_owned(),
                reason: "the URL is not valid".to_owned(),
            });
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(InvalidMavenRepository {
                url: url_text.to_owned(),
                reason: "the URL must not embed user credentials".to_owned(),
            });
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(InvalidMavenRepository {
                url: url_text.to_owned(),
                reason: "a repository base URL must not carry a query or fragment".to_owned(),
            });
        }
        if parsed.scheme() == "https" {
            // accepted below
        } else if parsed.scheme() == "http" && is_loopback_host(&parsed) {
            // accepted below (test transport)
        } else {
            return Err(InvalidMavenRepository {
                url: url_text.to_owned(),
                reason: "a production repository must use HTTPS".to_owned(),
            });
        }

        let path = parsed.path();
        if !path.ends_with('/') {
            return Err(InvalidMavenRepository {
                url: url_text.to_owned(),
                reason: "a repository base URL must end with '/'".to_owned(),
            });
        }
        if path
            .split('/')
            .any(|segment| segment == ".." || segment == ".")
        {
            return Err(InvalidMavenRepository {
                url: url_text.to_owned(),
                reason: "a repository base URL must not traverse".to_owned(),
            });
        }

        Ok(Self { base: parsed })
    }

    pub fn base_url(&self) -> &Url {
        &self.base
    }

    /// The deterministic artifact URL for one coordinate: the repository base
    /// joined with the coordinate's Maven layout path.
    ///
    /// Both parts are already validated (clean directory base; charset-safe
    /// relative path), so the join is plain concatenation and needs no
    /// re-encoding — the `+` official Fabric versions use stays a literal
    /// path character, exactly as the real repository requires.
    pub fn artifact_url(&self, coordinate: &MavenCoordinate) -> Url {
        let joined = format!("{}{}", self.base.as_str(), coordinate.repository_path());
        Url::parse(&joined).expect(
            "joining a validated repository base with a validated layout path yields a valid URL",
        )
    }
}

impl fmt::Display for MavenRepository {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.base.as_str())
    }
}

/// A malformed Maven repository base URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidMavenRepository {
    pub url: String,
    pub reason: String,
}

impl fmt::Display for InvalidMavenRepository {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Maven repository '{}' is invalid: {}",
            self.url, self.reason
        )
    }
}

impl std::error::Error for InvalidMavenRepository {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_coordinates_parse_including_official_plus_versions() {
        let plain = MavenCoordinate::parse("org.ow2.asm:asm:9.10.1").unwrap();
        assert_eq!(plain.group(), "org.ow2.asm");
        assert_eq!(plain.artifact(), "asm");
        assert_eq!(plain.version(), "9.10.1");
        assert_eq!(plain.classifier(), None);
        assert_eq!(plain.as_maven_string(), "org.ow2.asm:asm:9.10.1");

        // The '+' version suffix is real official Fabric metadata.
        let plus = MavenCoordinate::parse("net.fabricmc:sponge-mixin:0.17.4+mixin.0.8.7").unwrap();
        assert_eq!(plus.version(), "0.17.4+mixin.0.8.7");

        let classified = MavenCoordinate::parse("org.lwjgl:lwjgl:3.4.1:natives-windows").unwrap();
        assert_eq!(classified.classifier(), Some("natives-windows"));
    }

    #[test]
    fn malformed_and_traversal_like_coordinates_are_rejected() {
        for malformed in [
            "only-two:segments",
            "g:a:1:classifier:extra",
            "g:a:",
            "",
            "g:a:1.0:bad classifier",
            "net.fabricmc:fabric-loader",
        ] {
            assert!(
                MavenCoordinate::parse(malformed).is_err(),
                "{malformed:?} must be rejected"
            );
        }

        // Traversal-shaped segments never reach path construction.
        for traversal in ["..:..:..", "a:b:..", "a:b:1:.."] {
            assert!(
                MavenCoordinate::parse(traversal).is_err(),
                "{traversal:?} must be rejected"
            );
        }
    }

    #[test]
    fn repository_paths_follow_the_maven_layout() {
        let plain = MavenCoordinate::parse("org.ow2.asm:asm:9.10.1").unwrap();
        assert_eq!(
            plain.repository_path(),
            "org/ow2/asm/asm/9.10.1/asm-9.10.1.jar"
        );

        let plus = MavenCoordinate::parse("net.fabricmc:sponge-mixin:0.17.4+mixin.0.8.7").unwrap();
        assert_eq!(
            plus.repository_path(),
            "net/fabricmc/sponge-mixin/0.17.4+mixin.0.8.7/sponge-mixin-0.17.4+mixin.0.8.7.jar"
        );

        let classified = MavenCoordinate::parse("org.lwjgl:lwjgl:3.4.1:natives-windows").unwrap();
        assert_eq!(
            classified.repository_path(),
            "org/lwjgl/lwjgl/3.4.1/lwjgl-3.4.1-natives-windows.jar"
        );
    }

    #[test]
    fn repositories_enforce_https_and_a_clean_directory_base() {
        assert!(MavenRepository::parse("https://maven.fabricmc.net/").is_ok());
        assert!(MavenRepository::parse("https://repo.example.invalid/repository/").is_ok());

        // Cleartext non-loopback is a production non-starter.
        assert!(MavenRepository::parse("http://maven.example.invalid/").is_err());
        // Loopback cleartext is the documented test transport.
        assert!(MavenRepository::parse("http://127.0.0.1:8080/maven/").is_ok());

        for broken in [
            "https://maven.fabricmc.net/artifacts", // no trailing separator on a real path
            "https://maven.fabricmc.net/artifacts?q=1",
            "https://maven.fabricmc.net/#fragment",
            "https://user:secret@maven.fabricmc.net/",
            "not a url",
            "ftp://maven.fabricmc.net/",
        ] {
            assert!(
                MavenRepository::parse(broken).is_err(),
                "{broken:?} must be rejected"
            );
        }

        // Dot segments are resolved away by URL parsing itself, so such a
        // base can never smuggle a traversal into the joined artifact path.
        let normalized = MavenRepository::parse("https://maven.fabricmc.net/../escape/")
            .expect("dot segments normalize away during parsing");
        assert_eq!(normalized.base_url().path(), "/escape/");
    }

    #[test]
    fn artifact_urls_are_deterministic_and_preserve_plus_literals() {
        let repository = MavenRepository::parse("https://maven.fabricmc.net/").unwrap();

        let asm = MavenCoordinate::parse("org.ow2.asm:asm:9.10.1").unwrap();
        assert_eq!(
            repository.artifact_url(&asm).as_str(),
            "https://maven.fabricmc.net/org/ow2/asm/asm/9.10.1/asm-9.10.1.jar"
        );

        // '+' stays a literal path character, as the real Fabric Maven
        // repository requires.
        let mixin = MavenCoordinate::parse("net.fabricmc:sponge-mixin:0.17.4+mixin.0.8.7").unwrap();
        assert_eq!(
            repository.artifact_url(&mixin).as_str(),
            "https://maven.fabricmc.net/net/fabricmc/sponge-mixin/0.17.4+mixin.0.8.7/sponge-mixin-0.17.4+mixin.0.8.7.jar"
        );

        // Determinism: identical inputs, identical output.
        assert_eq!(
            repository.artifact_url(&asm),
            MavenRepository::parse("https://maven.fabricmc.net/")
                .unwrap()
                .artifact_url(&asm)
        );
    }
}
