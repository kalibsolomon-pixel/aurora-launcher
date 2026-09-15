//! Asset-index consumption: enumerating the objects a verified index
//! requires and deriving their official URLs.
//!
//! The external index DTO lives in [`crate::minecraft::metadata`] (external
//! Mojang shapes stay there); this module is the installation-side boundary
//! that turns a *verified* index into Aurora's own normalized object set.
//! The index is always verified against its official SHA-1 before this code
//! runs, and object URLs and storage paths are derived from the validated
//! hashes alone — a JSON `name` is a logical label, never a local path, and
//! no arbitrary object path from the document ever reaches the filesystem.

use std::collections::BTreeSet;
use std::fmt;

use url::Url;

use crate::downloads::is_loopback_host;
use crate::integrity::Sha1Digest;
use crate::minecraft::metadata::AssetIndexObjectsDocument;

/// The official Mojang asset object host (verified current September 2026):
/// objects are addressed by `<first two hash characters>/<full hash>` under
/// this root.
pub const OFFICIAL_ASSET_OBJECT_ROOT: &str = "https://resources.download.minecraft.net/";

/// The validated root asset objects are downloaded from.
///
/// Production pins Mojang's official host; a loopback constructor exists for
/// deterministic offline tests, mirroring the launcher's established
/// test-transport policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetObjectEndpoints {
    root: Url,
}

impl AssetObjectEndpoints {
    pub fn official() -> Self {
        Self::parse(OFFICIAL_ASSET_OBJECT_ROOT)
            .expect("the official asset object root is a valid HTTPS directory URL")
    }

    pub fn loopback_for_testing(base_url: &str) -> Self {
        let parsed = Self::parse(base_url)
            .unwrap_or_else(|error| panic!("test asset root must be valid: {error}"));
        assert!(
            parsed.root.scheme() == "http" && is_loopback_host(&parsed.root),
            "test asset endpoints must stay on the loopback"
        );
        parsed
    }

    fn parse(url_text: &str) -> Result<Self, InvalidAssetRoot> {
        // URL parsing normalizes a bare host to path "/", so the trailing
        // separator is checked on the input text itself.
        if !url_text.ends_with('/') {
            return Err(InvalidAssetRoot {
                url: url_text.to_owned(),
                reason: "a root must end with '/'".to_owned(),
            });
        }
        let parsed = Url::parse(url_text).map_err(|_| InvalidAssetRoot {
            url: url_text.to_owned(),
            reason: "the URL is not valid".to_owned(),
        })?;
        if parsed.cannot_be_a_base()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(InvalidAssetRoot {
                url: url_text.to_owned(),
                reason:
                    "a root must be a clean directory URL without credentials, query, or fragment"
                        .to_owned(),
            });
        }
        if parsed.scheme() == "https" {
            // accepted
        } else if parsed.scheme() == "http" && is_loopback_host(&parsed) {
            // accepted (test transport)
        } else {
            return Err(InvalidAssetRoot {
                url: url_text.to_owned(),
                reason: "a production root must use HTTPS".to_owned(),
            });
        }

        Ok(Self { root: parsed })
    }

    pub fn root(&self) -> &Url {
        &self.root
    }

    /// The official object URL for one hash:
    /// `<root><first two characters>/<full hash>`.
    pub fn object_url(&self, hash: &Sha1Digest) -> Url {
        let hex = hash.as_hex();
        let joined = format!("{}{}/{}", self.root.as_str(), &hex[..2], hex);
        Url::parse(&joined).expect("joining a validated root with a validated hash is valid")
    }
}

/// A malformed asset-object root URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidAssetRoot {
    pub url: String,
    pub reason: String,
}

impl fmt::Display for InvalidAssetRoot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "asset object root '{}' is invalid: {}",
            self.url, self.reason
        )
    }
}

impl std::error::Error for InvalidAssetRoot {}

/// One required asset object, normalized: its official SHA-1 (which is also
/// its content address) and its official size.
///
/// Identity is the hash — the index's logical names are deliberately absent
/// from this type, because installation stores objects by hash and only the
/// (installed) index maps names to objects at launch time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedAssetObject {
    sha1: Sha1Digest,
    size_bytes: u64,
}

impl PlannedAssetObject {
    pub fn sha1(&self) -> &Sha1Digest {
        &self.sha1
    }

    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// The object's path relative to the game directory:
    /// `assets/objects/<first two characters>/<full hash>`, derived purely
    /// from the validated hash.
    pub fn game_relative_path(&self) -> String {
        let hex = self.sha1.as_hex();
        format!("assets/objects/{}/{}", &hex[..2], hex)
    }
}

/// Normalizes a verified asset-index document into the deterministic set of
/// objects it requires.
///
/// Every entry must re-validate as it is normalized (canonical SHA-1,
/// positive size); one malformed object invalidates the enumeration, because
/// a partially enumerated index would install a partial asset set. Duplicate
/// hashes (several logical names sharing one object — real behavior in
/// official indexes) collapse to one requirement; iteration order follows the
/// DTO's sorted map, so the result is deterministic.
pub fn plan_asset_objects(
    index: &AssetIndexObjectsDocument,
) -> Result<Vec<PlannedAssetObject>, InvalidAssetObject> {
    let mut seen: BTreeSet<[u8; 20]> = BTreeSet::new();
    let mut objects = Vec::with_capacity(index.objects.len());

    for (name, object) in &index.objects {
        let sha1 = Sha1Digest::parse(&object.hash).map_err(|error| InvalidAssetObject {
            name: name.clone(),
            reason: error.to_string(),
        })?;
        if object.size == 0 {
            return Err(InvalidAssetObject {
                name: name.clone(),
                reason: "the object declares a size of zero".to_owned(),
            });
        }
        if seen.insert(*sha1.as_bytes()) {
            objects.push(PlannedAssetObject {
                sha1,
                size_bytes: object.size,
            });
        }
    }

    Ok(objects)
}

/// One asset-object entry that cannot be normalized into an acquisition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidAssetObject {
    pub name: String,
    pub reason: String,
}

impl fmt::Display for InvalidAssetObject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "asset object '{}' is invalid: {}",
            self.name, self.reason
        )
    }
}

impl std::error::Error for InvalidAssetObject {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minecraft::metadata::AssetIndexObjectsDocument;

    #[test]
    fn object_urls_follow_the_official_layout_from_the_hash_alone() {
        let official = AssetObjectEndpoints::official();
        let hash = Sha1Digest::parse("5ff04807c356f1beed0b86ccf659b44b9983e3fa").unwrap();

        assert_eq!(
            official.object_url(&hash).as_str(),
            "https://resources.download.minecraft.net/5f/5ff04807c356f1beed0b86ccf659b44b9983e3fa"
        );
        assert_eq!(
            official.object_url(&hash).as_str(),
            official.object_url(&hash).as_str(),
            "derivation is deterministic"
        );

        let loopback = AssetObjectEndpoints::loopback_for_testing("http://127.0.0.1:9123/assets/");
        assert_eq!(
            loopback.object_url(&hash).as_str(),
            "http://127.0.0.1:9123/assets/5f/5ff04807c356f1beed0b86ccf659b44b9983e3fa"
        );
    }

    #[test]
    fn roots_enforce_the_transport_policy_and_a_clean_directory_shape() {
        assert!(AssetObjectEndpoints::parse("https://resources.download.minecraft.net/").is_ok());
        assert!(AssetObjectEndpoints::parse("http://127.0.0.1:9000/assets/").is_ok());

        for broken in [
            "http://assets.example.invalid/",
            "https://resources.download.minecraft.net",
            "https://resources.download.minecraft.net/?q=1",
            "https://user:secret@resources.download.minecraft.net/",
            "not a url",
        ] {
            assert!(
                AssetObjectEndpoints::parse(broken).is_err(),
                "{broken:?} must be rejected"
            );
        }
    }

    #[test]
    fn duplicate_objects_collapse_to_one_requirement_and_paths_derive_from_hashes() {
        let index = AssetIndexObjectsDocument::from_json(
            r#"{
                "objects": {
                    "icons/icon_16x16.png": { "hash": "5ff04807c356f1beed0b86ccf659b44b9983e3fa", "size": 781 },
                    "icons/icon_16x16_hd.png": { "hash": "5ff04807c356f1beed0b86ccf659b44b9983e3fa", "size": 781 },
                    "minecraft/sounds/random/click.ogg": { "hash": "916021c195f5799e23fd4b5e2c8e0b2b2d8b1a2c", "size": 3443 }
                }
            }"#,
        )
        .unwrap();

        let objects = plan_asset_objects(&index).unwrap();

        assert_eq!(objects.len(), 2, "two names share one object");
        assert_eq!(
            objects[0].game_relative_path(),
            "assets/objects/5f/5ff04807c356f1beed0b86ccf659b44b9983e3fa"
        );
        assert_eq!(objects[0].size_bytes(), 781);
        assert_eq!(
            objects[1].game_relative_path(),
            "assets/objects/91/916021c195f5799e23fd4b5e2c8e0b2b2d8b1a2c"
        );
    }
}
