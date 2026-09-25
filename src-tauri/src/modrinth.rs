//! Modrinth v2 adapter. API DTOs and URL authority stay inside this module.
//! Every version is checked against the instance before it becomes a plan.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::downloads::{DownloadOptions, Sha512ArtifactSource};
use crate::instance_content::{
    ContentCompatibility, ContentState, ContentType, DependencyKind, ProviderArtifactSource,
    ProviderDependency, ProviderIdentity, ProviderInstallPlan, ProviderRecord,
};

const BASE: &str = "https://api.modrinth.com/v2/";
const FABRIC_API_PROJECT: &str = "P7dR8mSH";
const MAX_RESPONSE: usize = 4 * 1024 * 1024;
const MAX_GRAPH: usize = 64;

#[derive(Debug, Clone)]
pub struct Context {
    pub minecraft_version: String,
    pub loader: String,
}

#[derive(Debug, Clone)]
pub struct Client {
    base: Url,
    http: reqwest::Client,
}

impl Client {
    pub fn official() -> Self {
        static OFFICIAL: OnceLock<Client> = OnceLock::new();
        OFFICIAL
            .get_or_init(|| Self {
                base: Url::parse(BASE).expect("fixed official Modrinth API URL"),
                http: crate::downloads::build_client(&DownloadOptions::default()),
            })
            .clone()
    }

    #[cfg(test)]
    fn for_testing(base: &str) -> Self {
        Self {
            base: Url::parse(base).unwrap(),
            http: crate::downloads::build_client(&DownloadOptions::default()),
        }
    }

    fn endpoint(&self, segments: &[&str]) -> Result<Url, Error> {
        let mut url = self.base.clone();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| Error::InvalidResponse)?;
            path.pop_if_empty();
            for segment in segments {
                if segment.is_empty()
                    || segment.contains(['/', '\\'])
                    || segment == &"."
                    || segment == &".."
                {
                    return Err(Error::InvalidResponse);
                }
                path.push(segment);
            }
        }
        Ok(url)
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, url: Url) -> Result<T, Error> {
        let mut response = self
            .http
            .get(url)
            .header(
                reqwest::header::USER_AGENT,
                concat!(
                    "kalibsolomon-pixel/aurora-launcher/",
                    env!("CARGO_PKG_VERSION")
                ),
            )
            .send()
            .await
            .map_err(|_| Error::Network)?;
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let reset_seconds = response
                .headers()
                .get("x-ratelimit-reset")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok());
            return Err(Error::RateLimited(reset_seconds));
        }
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(Error::NotFound);
        }
        if !response.status().is_success() {
            return Err(Error::Network);
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_RESPONSE as u64)
        {
            return Err(Error::InvalidResponse);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Network)? {
            if body.len() + chunk.len() > MAX_RESPONSE {
                return Err(Error::InvalidResponse);
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| Error::InvalidResponse)
    }

    pub async fn search(
        &self,
        context: &Context,
        kind: ContentType,
        query: &str,
        offset: u32,
    ) -> Result<SearchPage, Error> {
        if query.len() > 160 || offset > 10_000 {
            return Err(Error::InvalidRequest);
        }
        let mut url = self.endpoint(&["search"])?;
        let project_type = project_type(kind);
        let mut facets = vec![
            vec![format!("project_type:{project_type}")],
            vec![format!("versions:{}", context.minecraft_version)],
        ];
        if kind == ContentType::Mod {
            facets.push(vec!["categories:fabric".into()]);
            facets.push(
                CLIENT_ENVIRONMENTS
                    .iter()
                    .map(|value| format!("environment:{value}"))
                    .collect(),
            );
        }
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("query", query.trim());
            pairs.append_pair(
                "facets",
                &serde_json::to_string(&facets).expect("facets serialize"),
            );
            pairs.append_pair("limit", "20");
            pairs.append_pair("offset", &offset.to_string());
        }
        let result: SearchDto = self.get(url).await?;
        Ok(SearchPage {
            offset: result.offset,
            total_hits: result.total_hits,
            hits: result
                .hits
                .into_iter()
                .filter(|hit| {
                    hit.project_type == project_type
                        && hit
                            .versions
                            .iter()
                            .any(|version| version == &context.minecraft_version)
                        && (kind != ContentType::Mod
                            || hit.environment.iter().any(|env| client_environment(env)))
                })
                .map(|hit| ProjectSummary {
                    project_id: hit.project_id,
                    title: hit.title,
                    summary: hit.description,
                    author: hit.author,
                    downloads: hit.downloads,
                    icon_url: hit.icon_url.as_deref().and_then(safe_icon_url),
                    project_type: kind,
                })
                .collect(),
        })
    }

    async fn project(&self, id: &str) -> Result<ProjectDto, Error> {
        validate_id(id)?;
        self.get(self.endpoint(&["project", id])?).await
    }

    async fn version(&self, id: &str) -> Result<VersionDto, Error> {
        validate_id(id)?;
        self.get(self.endpoint(&["version", id])?).await
    }

    async fn versions(
        &self,
        context: &Context,
        kind: ContentType,
        project_id: &str,
    ) -> Result<Vec<VersionDto>, Error> {
        validate_id(project_id)?;
        let mut url = self.endpoint(&["project", project_id, "version"])?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair(
                "game_versions",
                &serde_json::to_string(&[&context.minecraft_version]).unwrap(),
            );
            if kind == ContentType::Mod {
                pairs.append_pair(
                    "loaders",
                    &serde_json::to_string(&[&context.loader]).unwrap(),
                );
            } else if kind == ContentType::ResourcePack {
                pairs.append_pair("loaders", "[\"minecraft\"]");
            }
            pairs.append_pair("include_changelog", "false");
        }
        let versions: Vec<VersionDto> = self.get(url).await?;
        Ok(versions
            .into_iter()
            .filter(|version| compatible(context, kind, version))
            .collect())
    }

    pub async fn details(
        &self,
        context: &Context,
        kind: ContentType,
        project_id: &str,
    ) -> Result<ProjectDetails, Error> {
        let project = self.project(project_id).await?;
        if project.project_type != project_type(kind) {
            return Err(Error::NoCompatibleVersion);
        }
        let mut versions = self.versions(context, kind, &project.id).await?;
        versions.sort_by(|a, b| b.date_published.cmp(&a.date_published));
        let choices: Vec<_> = versions
            .iter()
            .map(|version| VersionChoice {
                id: version.id.clone(),
                name: version.name.clone(),
                version_number: version.version_number.clone(),
                version_type: version.version_type.clone(),
                date_published: version.date_published.clone(),
                environment: version.environment.clone(),
                loaders: version.loaders.clone(),
            })
            .collect();
        let default_version_id = choices
            .iter()
            .find(|item| item.version_type == "release")
            .or_else(|| choices.first())
            .map(|item| item.id.clone());
        Ok(ProjectDetails {
            project_id: project.id,
            title: project.title,
            summary: project.description,
            license: project.license.id,
            game_versions: project.game_versions,
            loaders: project.loaders,
            environments: project.environment,
            versions: choices,
            default_version_id,
            project_type: kind,
        })
    }

    pub async fn resolve(
        &self,
        context: &Context,
        kind: ContentType,
        project_id: &str,
        version_id: &str,
        installed: &ContentState,
    ) -> Result<Resolved, Error> {
        self.resolve_inner(context, kind, project_id, version_id, installed, None)
            .await
    }

    pub async fn resolve_update(
        &self,
        context: &Context,
        kind: ContentType,
        project_id: &str,
        version_id: &str,
        installed: &ContentState,
    ) -> Result<Resolved, Error> {
        let replacing = ProviderIdentity {
            content_type: kind,
            provider: "modrinth".into(),
            project_id: project_id.into(),
        };
        if installed.find(&replacing).is_none() {
            return Err(Error::InvalidRequest);
        }
        self.resolve_inner(
            context,
            kind,
            project_id,
            version_id,
            installed,
            Some(replacing),
        )
        .await
    }

    async fn resolve_inner(
        &self,
        context: &Context,
        kind: ContentType,
        project_id: &str,
        version_id: &str,
        installed: &ContentState,
        replacing: Option<ProviderIdentity>,
    ) -> Result<Resolved, Error> {
        let mut graph = Graph {
            context,
            client: self,
            installed,
            plans: Vec::new(),
            items: Vec::new(),
            visiting: HashSet::new(),
            seen: HashMap::new(),
            warnings: Vec::new(),
            replacing,
        };
        graph
            .visit(
                project_id.to_owned(),
                Some(version_id.to_owned()),
                Some(kind),
                true,
            )
            .await?;
        if graph.plans.is_empty() && graph.items.is_empty() {
            return Err(Error::NoCompatibleVersion);
        }
        let preview = InstallPreview {
            project_id: project_id.to_owned(),
            version_id: version_id.to_owned(),
            content_type: kind,
            items: graph.items,
            warnings: graph.warnings,
        };
        Ok(Resolved {
            preview,
            plans: graph.plans,
        })
    }

    /// Select the same release-preferred compatible version shown by Details.
    /// Search hits are display data and never authorize a file or version.
    pub async fn resolve_latest(
        &self,
        context: &Context,
        kind: ContentType,
        project_id: &str,
        installed: &ContentState,
    ) -> Result<Resolved, Error> {
        let details = self.details(context, kind, project_id).await?;
        let version_id = details
            .default_version_id
            .ok_or(Error::NoCompatibleVersion)?;
        self.resolve(context, kind, project_id, &version_id, installed)
            .await
    }

    /// Publication time orders versions; release policy follows the installed
    /// channel. A stable install never silently moves to beta or alpha.
    pub async fn update_candidate(
        &self,
        context: &Context,
        installed: &ProviderRecord,
    ) -> Result<Option<VersionChoice>, Error> {
        if installed.provider != "modrinth" {
            return Err(Error::InvalidRequest);
        }
        let current = self.version(&installed.version_id).await?;
        if current.project_id != installed.project_id
            || choose_file(&current, installed.content_type)?.hashes.sha512 != installed.file_id
        {
            return Err(Error::InvalidResponse);
        }
        let allowed: &[&str] = match current.version_type.as_str() {
            "release" => &["release"],
            "beta" => &["release", "beta"],
            "alpha" => &["release", "beta", "alpha"],
            _ => return Err(Error::InvalidResponse),
        };
        let mut versions = self
            .versions(context, installed.content_type, &installed.project_id)
            .await?;
        versions.retain(|candidate| {
            candidate.project_id == installed.project_id
                && candidate.date_published > current.date_published
                && allowed.contains(&candidate.version_type.as_str())
        });
        versions.sort_by(|a, b| {
            b.date_published
                .cmp(&a.date_published)
                .then_with(|| b.id.cmp(&a.id))
        });
        Ok(versions.first().map(|version| VersionChoice {
            id: version.id.clone(),
            name: version.name.clone(),
            version_number: version.version_number.clone(),
            version_type: version.version_type.clone(),
            date_published: version.date_published.clone(),
            environment: version.environment.clone(),
            loaders: version.loaders.clone(),
        }))
    }
}

fn safe_icon_url(raw: &str) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("cdn.modrinth.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !url.path().starts_with("/data/")
    {
        return None;
    }
    Some(url.into())
}

const CLIENT_ENVIRONMENTS: &[&str] = &[
    "client_and_server",
    "client_only",
    "client_only_server_optional",
    "singleplayer_only",
    "client_or_server",
    "client_or_server_prefers_both",
];

fn client_environment(value: &str) -> bool {
    CLIENT_ENVIRONMENTS.contains(&value) || value == "unknown"
}

fn project_type(kind: ContentType) -> &'static str {
    match kind {
        ContentType::Mod => "mod",
        ContentType::ResourcePack => "resourcepack",
        ContentType::ShaderPack => "shader",
    }
}

fn content_type(project_type: &str) -> Result<ContentType, Error> {
    match project_type {
        "mod" => Ok(ContentType::Mod),
        "resourcepack" => Ok(ContentType::ResourcePack),
        "shader" => Ok(ContentType::ShaderPack),
        _ => Err(Error::DependencyUnresolved),
    }
}

fn compatible(context: &Context, kind: ContentType, version: &VersionDto) -> bool {
    version
        .game_versions
        .iter()
        .any(|value| value == &context.minecraft_version)
        && match kind {
            ContentType::Mod => {
                version.loaders.iter().any(|value| value == &context.loader)
                    && client_environment(&version.environment)
            }
            ContentType::ResourcePack => version.loaders.iter().any(|value| value == "minecraft"),
            ContentType::ShaderPack => true,
        }
}

fn validate_id(id: &str) -> Result<(), Error> {
    if id.len() == 8 && id.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        Err(Error::InvalidRequest)
    }
}

fn choose_file(version: &VersionDto, kind: ContentType) -> Result<&FileDto, Error> {
    let extension = if kind == ContentType::Mod {
        ".jar"
    } else {
        ".zip"
    };
    let eligible = |file: &&FileDto| {
        file.filename.to_ascii_lowercase().ends_with(extension)
            && !matches!(
                file.file_type.as_deref(),
                Some("sources-jar" | "dev-jar" | "javadoc-jar" | "signature")
            )
            && crate::instance_content::validate_file_name(&file.filename).is_ok()
            && file.size > 0
    };
    if let Some(primary) = version.files.iter().find(|file| file.primary) {
        return eligible(&primary)
            .then_some(primary)
            .ok_or(Error::InvalidResponse);
    }
    version
        .files
        .iter()
        .filter(eligible)
        .next()
        .ok_or(Error::InvalidResponse)
}

struct Graph<'a> {
    context: &'a Context,
    client: &'a Client,
    installed: &'a ContentState,
    plans: Vec<ProviderInstallPlan>,
    items: Vec<PreviewItem>,
    visiting: HashSet<String>,
    seen: HashMap<String, String>,
    warnings: Vec<String>,
    replacing: Option<ProviderIdentity>,
}

impl Graph<'_> {
    fn visit<'a>(
        &'a mut self,
        project_id: String,
        version_id: Option<String>,
        requested_kind: Option<ContentType>,
        root: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            validate_id(&project_id)?;
            if self.visiting.contains(&project_id) {
                return Err(Error::DependencyCycle);
            }
            if self.seen.contains_key(&project_id) {
                if version_id
                    .as_ref()
                    .is_some_and(|wanted| self.seen.get(&project_id) != Some(wanted))
                {
                    return Err(Error::DependencyUnresolved);
                }
                return Ok(());
            }
            if self.seen.len() >= MAX_GRAPH {
                return Err(Error::DependencyUnresolved);
            }
            if project_id == FABRIC_API_PROJECT && root {
                return Err(Error::DependencyConflict);
            }
            if project_id == FABRIC_API_PROJECT && !root {
                if version_id.is_some() {
                    return Err(Error::DependencyUnresolved);
                }
                self.warnings
                    .push("Fabric API is already launcher managed and protected.".into());
                return Ok(());
            }
            let project = self.client.project(&project_id).await.map_err(|error| {
                if root {
                    error
                } else {
                    Error::DependencyUnresolved
                }
            })?;
            let kind = content_type(&project.project_type)?;
            if requested_kind.is_some_and(|wanted| wanted != kind) {
                return Err(if root {
                    Error::NoCompatibleVersion
                } else {
                    Error::DependencyUnresolved
                });
            }
            let version = if let Some(id) = &version_id {
                let version = self.client.version(id).await?;
                if version.project_id != project.id || !compatible(self.context, kind, &version) {
                    return Err(if root {
                        Error::NoCompatibleVersion
                    } else {
                        Error::DependencyUnresolved
                    });
                }
                version
            } else {
                let mut versions = self
                    .client
                    .versions(self.context, kind, &project.id)
                    .await?;
                versions.sort_by(|a, b| b.date_published.cmp(&a.date_published));
                versions
                    .iter()
                    .find(|item| item.version_type == "release")
                    .or_else(|| versions.first())
                    .cloned()
                    .ok_or(Error::DependencyUnresolved)?
            };
            let file = choose_file(&version, kind)?;
            let source =
                Sha512ArtifactSource::https(&file.url, &file.hashes.sha512, Some(file.size))
                    .map_err(|_| Error::InvalidResponse)?;
            if self.installed.entries.iter().any(|record| {
                record.provider == "modrinth"
                    && record.project_id == project.id
                    && record.version_id != version.id
                    && record.content_type == kind
                    && self.replacing.as_ref() != Some(&record.identity())
            }) {
                return Err(Error::DependencyConflict);
            }
            let already = self.installed.entries.iter().any(|record| {
                record.provider == "modrinth"
                    && record.project_id == project.id
                    && record.version_id == version.id
                    && record.content_type == kind
                    && self.replacing.as_ref() != Some(&record.identity())
            });
            self.visiting.insert(project.id.clone());
            let mut dependencies = Vec::new();
            for dependency in &version.dependencies {
                let dep_project = if let Some(id) = &dependency.project_id {
                    Some(id.clone())
                } else if let Some(id) = &dependency.version_id {
                    Some(self.client.version(id).await?.project_id)
                } else {
                    None
                };
                if let Some(id) = &dep_project {
                    if dependency.dependency_type != "embedded" {
                        dependencies.push(ProviderDependency {
                            kind: match dependency.dependency_type.as_str() {
                                "required" => DependencyKind::Required,
                                "optional" => DependencyKind::Optional,
                                "incompatible" => DependencyKind::Incompatible,
                                _ => return Err(Error::InvalidResponse),
                            },
                            provider: "modrinth".into(),
                            project_id: id.clone(),
                            version_id: dependency.version_id.clone(),
                        });
                    }
                }
                match dependency.dependency_type.as_str() {
                    "required" => {
                        let dep_project = dep_project.ok_or(Error::DependencyUnresolved)?;
                        self.visit(dep_project, dependency.version_id.clone(), None, false)
                            .await?;
                    }
                    "optional" => self.warnings.push(format!(
                        "Optional dependency {} is not installed automatically.",
                        dep_project.as_deref().unwrap_or("external")
                    )),
                    "incompatible" => {
                        if dep_project.as_ref().is_some_and(|id| {
                            self.installed.entries.iter().any(|record| {
                                record.provider == "modrinth" && record.project_id == *id
                            }) || self.seen.contains_key(id)
                        }) {
                            return Err(Error::DependencyConflict);
                        }
                        self.warnings.push("This version declares an incompatible project; inspect local manual mods before installing.".into());
                    }
                    "embedded" => {}
                    _ => return Err(Error::InvalidResponse),
                }
            }
            self.visiting.remove(&project.id);
            self.seen.insert(project.id.clone(), version.id.clone());
            self.items.push(PreviewItem {
                project_id: project.id.clone(),
                title: project.title.clone(),
                version_id: version.id.clone(),
                version_number: version.version_number.clone(),
                file_name: file.filename.clone(),
                already_installed: already,
            });
            if !already {
                self.plans.push(ProviderInstallPlan {
                    content_type: kind,
                    provider: "modrinth".into(),
                    project_id: project.id,
                    version_id: version.id.clone(),
                    file_id: file.hashes.sha512.clone(),
                    file_name: file.filename.clone(),
                    display_version: Some(version.version_number),
                    compatibility: ContentCompatibility {
                        minecraft_versions: version.game_versions,
                        loader: (kind == ContentType::Mod).then(|| self.context.loader.clone()),
                        environment: Some(version.environment),
                    },
                    dependencies,
                    source: ProviderArtifactSource::Sha512(source),
                });
            }
            Ok(())
        })
    }
}

pub struct Resolved {
    pub preview: InstallPreview,
    pub plans: Vec<ProviderInstallPlan>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPage {
    pub offset: u32,
    pub total_hits: u32,
    pub hits: Vec<ProjectSummary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub project_id: String,
    pub title: String,
    pub summary: String,
    pub author: String,
    pub downloads: u64,
    pub icon_url: Option<String>,
    pub project_type: ContentType,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDetails {
    pub project_id: String,
    pub title: String,
    pub summary: String,
    pub license: String,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    pub environments: Vec<String>,
    pub versions: Vec<VersionChoice>,
    pub default_version_id: Option<String>,
    pub project_type: ContentType,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionChoice {
    pub id: String,
    pub name: String,
    pub version_number: String,
    pub version_type: String,
    pub date_published: String,
    pub environment: String,
    pub loaders: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPreview {
    pub project_id: String,
    pub version_id: String,
    pub content_type: ContentType,
    pub items: Vec<PreviewItem>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewItem {
    pub project_id: String,
    pub title: String,
    pub version_id: String,
    pub version_number: String,
    pub file_name: String,
    pub already_installed: bool,
}

#[derive(Debug, Deserialize)]
struct SearchDto {
    hits: Vec<SearchHitDto>,
    offset: u32,
    total_hits: u32,
}
#[derive(Debug, Deserialize)]
struct SearchHitDto {
    project_id: String,
    project_type: String,
    title: String,
    description: String,
    author: String,
    downloads: u64,
    #[serde(default)]
    icon_url: Option<String>,
    versions: Vec<String>,
    environment: Vec<String>,
}
#[derive(Debug, Deserialize)]
struct ProjectDto {
    id: String,
    project_type: String,
    title: String,
    description: String,
    license: LicenseDto,
    game_versions: Vec<String>,
    loaders: Vec<String>,
    environment: Vec<String>,
}
#[derive(Debug, Deserialize)]
struct LicenseDto {
    id: String,
}
#[derive(Debug, Clone, Deserialize)]
struct VersionDto {
    id: String,
    project_id: String,
    name: String,
    version_number: String,
    version_type: String,
    date_published: String,
    game_versions: Vec<String>,
    loaders: Vec<String>,
    environment: String,
    files: Vec<FileDto>,
    dependencies: Vec<DependencyDto>,
}
#[derive(Debug, Clone, Deserialize)]
struct FileDto {
    hashes: HashesDto,
    url: String,
    filename: String,
    primary: bool,
    size: u64,
    file_type: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
struct HashesDto {
    sha512: String,
}
#[derive(Debug, Clone, Deserialize)]
struct DependencyDto {
    project_id: Option<String>,
    version_id: Option<String>,
    dependency_type: String,
}

#[derive(Debug)]
pub enum Error {
    InvalidRequest,
    Network,
    RateLimited(Option<u64>),
    InvalidResponse,
    NotFound,
    NoCompatibleVersion,
    DependencyUnresolved,
    DependencyCycle,
    DependencyConflict,
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest | Self::InvalidResponse => "provider_invalid_response",
            Self::Network => "provider_network_error",
            Self::RateLimited(_) => "provider_rate_limited",
            Self::NotFound => "provider_project_not_found",
            Self::NoCompatibleVersion => "provider_no_compatible_version",
            Self::DependencyUnresolved => "provider_dependency_unresolved",
            Self::DependencyCycle => "provider_dependency_cycle",
            Self::DependencyConflict => "provider_content_collision",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest => write!(formatter, "The Modrinth request is invalid."),
            Self::Network => write!(
                formatter,
                "Modrinth is unavailable. Installed content remains usable."
            ),
            Self::RateLimited(reset) => write!(
                formatter,
                "Modrinth's rate limit was reached. Try again in {} seconds.",
                reset.unwrap_or(60)
            ),
            Self::InvalidResponse => write!(
                formatter,
                "Modrinth returned metadata Aurora cannot use safely."
            ),
            Self::NotFound => write!(formatter, "This Modrinth project or version was not found."),
            Self::NoCompatibleVersion => write!(
                formatter,
                "No version supports this instance's Minecraft and loader."
            ),
            Self::DependencyUnresolved => write!(
                formatter,
                "A required Modrinth dependency cannot be resolved safely."
            ),
            Self::DependencyCycle => {
                write!(formatter, "The required dependency graph contains a cycle.")
            }
            Self::DependencyConflict => write!(
                formatter,
                "An installed project conflicts with a required dependency."
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TestRequest, TestResponse, TestServer};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    fn context() -> Context {
        Context {
            minecraft_version: "1.21.11".into(),
            loader: "fabric".into(),
        }
    }

    fn project(id: &str, kind: &str) -> Value {
        json!({
            "id": id, "project_type": kind, "title": format!("Project {id}"),
            "description": "A test project", "license": {"id": "MIT"},
            "game_versions": ["1.21.11"], "loaders": ["fabric"],
            "environment": ["client_and_server"]
        })
    }

    fn version(id: &str, project_id: &str, deps: Value) -> Value {
        json!({
            "id": id, "project_id": project_id, "name": "Test release",
            "version_number": "1.0.0", "version_type": "release",
            "date_published": "2026-01-01T00:00:00Z",
            "game_versions": ["1.21.11"], "loaders": ["fabric"],
            "environment": "client_and_server",
            "files": [{
                "hashes": {"sha512": "a".repeat(128)},
                "url": "https://cdn.modrinth.com/data/test/file.jar",
                "filename": format!("{id}.jar"), "primary": true,
                "size": 4, "file_type": null
            }],
            "dependencies": deps
        })
    }

    fn server(routes: HashMap<String, Value>) -> TestServer {
        TestServer::spawn(Arc::new(move |request: &TestRequest| {
            let path = request.path.split('?').next().unwrap_or_default();
            routes
                .get(path)
                .map(|value| TestResponse::ok(&serde_json::to_vec(value).unwrap()))
                .unwrap_or_else(|| {
                    eprintln!("missing mock route: {path}");
                    TestResponse::status(404)
                })
        }))
    }

    #[tokio::test]
    async fn search_uses_instance_owned_filters_and_pagination() {
        let paths = Arc::new(Mutex::new(Vec::new()));
        let captured = paths.clone();
        let server = TestServer::spawn(Arc::new(move |request: &TestRequest| {
            captured.lock().unwrap().push(request.path.clone());
            TestResponse::ok(br#"{"hits":[],"offset":20,"total_hits":30}"#)
        }));
        let client = Client::for_testing(&format!("{}/v2/", server.base_url()));
        for kind in [
            ContentType::Mod,
            ContentType::ResourcePack,
            ContentType::ShaderPack,
        ] {
            let page = client.search(&context(), kind, "test", 20).await.unwrap();
            assert_eq!(page.offset, 20);
        }
        let requests = paths.lock().unwrap();
        let facets: Vec<Vec<Vec<String>>> = requests
            .iter()
            .map(|path| {
                let url = Url::parse(&format!("http://localhost{path}")).unwrap();
                let text = url
                    .query_pairs()
                    .find(|(key, _)| key == "facets")
                    .unwrap()
                    .1;
                serde_json::from_str(&text).unwrap()
            })
            .collect();
        assert!(
            facets[0]
                .iter()
                .flatten()
                .any(|value| value == "categories:fabric")
        );
        assert!(
            facets[0]
                .iter()
                .flatten()
                .any(|value| value == "versions:1.21.11")
        );
        assert!(
            facets[1]
                .iter()
                .flatten()
                .any(|value| value == "project_type:resourcepack")
        );
        assert!(
            !facets[1]
                .iter()
                .flatten()
                .any(|value| value == "categories:fabric")
        );
        assert!(
            facets[2]
                .iter()
                .flatten()
                .any(|value| value == "project_type:shader")
        );
    }

    #[tokio::test]
    async fn empty_browse_query_keeps_safe_icons_and_drops_untrusted_ones() {
        let server = TestServer::spawn(Arc::new(|request: &TestRequest| {
            let url = Url::parse(&format!("http://localhost{}", request.path)).unwrap();
            assert_eq!(
                url.query_pairs().find(|(key, _)| key == "query").unwrap().1,
                ""
            );
            assert_eq!(
                url.query_pairs()
                    .find(|(key, _)| key == "offset")
                    .unwrap()
                    .1,
                "0"
            );
            TestResponse::ok(&serde_json::to_vec(&json!({
                "offset": 0, "total_hits": 3,
                "hits": [
                    {"project_id":"AAAABBBB","project_type":"mod","title":"Icon","description":"a","author":"a","downloads":1,"versions":["1.21.11"],"environment":["client_and_server"],"icon_url":"https://cdn.modrinth.com/data/AAAABBBB/icon.png"},
                    {"project_id":"BBBBCCCC","project_type":"mod","title":"None","description":"b","author":"b","downloads":1,"versions":["1.21.11"],"environment":["client_and_server"],"icon_url":null},
                    {"project_id":"CCCCDDDD","project_type":"mod","title":"Bad","description":"c","author":"c","downloads":1,"versions":["1.21.11"],"environment":["client_and_server"],"icon_url":"https://evil.example/data/icon.png"}
                ]
            })).unwrap())
        }));
        let client = Client::for_testing(&format!("{}/v2/", server.base_url()));
        let page = client
            .search(&context(), ContentType::Mod, "", 0)
            .await
            .unwrap();
        assert_eq!(page.hits.len(), 3);
        assert_eq!(
            page.hits[0].icon_url.as_deref(),
            Some("https://cdn.modrinth.com/data/AAAABBBB/icon.png")
        );
        assert!(page.hits[1].icon_url.is_none());
        assert!(page.hits[2].icon_url.is_none());
        assert!(safe_icon_url("javascript:alert(1)").is_none());
        assert!(safe_icon_url("https://cdn.modrinth.com.evil.example/data/a.png").is_none());
    }

    #[tokio::test]
    async fn quick_resolution_uses_latest_compatible_for_each_content_type() {
        for (kind, project_type, loader, extension) in [
            (ContentType::Mod, "mod", "fabric", "jar"),
            (
                ContentType::ResourcePack,
                "resourcepack",
                "minecraft",
                "zip",
            ),
            (ContentType::ShaderPack, "shader", "iris", "zip"),
        ] {
            let mut routes = HashMap::new();
            routes.insert(
                "/v2/project/AAAABBBB".into(),
                project("AAAABBBB", project_type),
            );
            let mut wrong = version("11112222", "AAAABBBB", json!([]));
            wrong["date_published"] = json!("2026-09-01T00:00:00Z");
            wrong["game_versions"] = json!(["1.20.1"]);
            let mut good = version("22223333", "AAAABBBB", json!([]));
            good["loaders"] = json!([loader]);
            good["files"][0]["filename"] = json!(format!("content.{extension}"));
            routes.insert(
                "/v2/project/AAAABBBB/version".into(),
                json!([wrong, good.clone()]),
            );
            routes.insert("/v2/version/22223333".into(), good);
            let server = server(routes);
            let client = Client::for_testing(&format!("{}/v2/", server.base_url()));
            let resolved = client
                .resolve_latest(&context(), kind, "AAAABBBB", &ContentState::empty())
                .await
                .unwrap();
            assert_eq!(resolved.preview.version_id, "22223333");
            assert_eq!(resolved.plans.len(), 1);
        }
    }

    #[tokio::test]
    async fn quick_resolution_no_match_never_reaches_artifact_planning() {
        for (kind, project_type) in [
            (ContentType::Mod, "mod"),
            (ContentType::ResourcePack, "resourcepack"),
            (ContentType::ShaderPack, "shader"),
        ] {
            let mut routes = HashMap::new();
            routes.insert(
                "/v2/project/AAAABBBB".into(),
                project("AAAABBBB", project_type),
            );
            let mut wrong = version("11112222", "AAAABBBB", json!([]));
            wrong["game_versions"] = json!(["1.20.1"]);
            routes.insert("/v2/project/AAAABBBB/version".into(), json!([wrong]));
            let server = server(routes);
            let client = Client::for_testing(&format!("{}/v2/", server.base_url()));
            assert!(matches!(
                client
                    .resolve_latest(&context(), kind, "AAAABBBB", &ContentState::empty())
                    .await,
                Err(Error::NoCompatibleVersion)
            ));
        }
    }

    #[tokio::test]
    async fn update_discovery_respects_publication_compatibility_and_release_channel() {
        let mut current = version("11112222", "AAAABBBB", json!([]));
        current["date_published"] = json!("2026-01-01T00:00:00Z");
        let mut stable = version("22223333", "AAAABBBB", json!([]));
        stable["date_published"] = json!("2026-02-01T00:00:00Z");
        let mut beta = version("33334444", "AAAABBBB", json!([]));
        beta["version_type"] = json!("beta");
        beta["date_published"] = json!("2026-03-01T00:00:00Z");
        let mut wrong_loader = version("44445555", "AAAABBBB", json!([]));
        wrong_loader["date_published"] = json!("2026-04-01T00:00:00Z");
        wrong_loader["loaders"] = json!(["forge"]);
        let mut wrong_game = version("55556666", "AAAABBBB", json!([]));
        wrong_game["date_published"] = json!("2026-05-01T00:00:00Z");
        wrong_game["game_versions"] = json!(["1.20.1"]);
        let mut routes = HashMap::new();
        routes.insert("/v2/version/11112222".into(), current.clone());
        routes.insert(
            "/v2/project/AAAABBBB/version".into(),
            json!([
                wrong_game,
                wrong_loader,
                beta.clone(),
                stable.clone(),
                current.clone()
            ]),
        );
        let first_server = server(routes);
        let client = Client::for_testing(&format!("{}/v2/", first_server.base_url()));
        let mut record = ProviderRecord {
            content_type: ContentType::Mod,
            provider: "modrinth".into(),
            project_id: "AAAABBBB".into(),
            version_id: "11112222".into(),
            file_id: "a".repeat(128),
            file_name: "11112222.jar".into(),
            sha256: "b".repeat(64),
            display_version: Some("1.0.0".into()),
            compatibility: ContentCompatibility {
                minecraft_versions: vec!["1.21.11".into()],
                loader: Some("fabric".into()),
                environment: Some("client_and_server".into()),
            },
            dependencies: vec![],
            explicitly_retained: true,
            requires: vec![],
        };
        assert_eq!(
            client
                .update_candidate(&context(), &record)
                .await
                .unwrap()
                .unwrap()
                .id,
            "22223333"
        );
        // A beta install may move to a newer compatible beta. The current
        // version metadata, not a human-readable version string, sets policy.
        let mut beta_current = current;
        beta_current["version_type"] = json!("beta");
        let mut beta_routes = HashMap::new();
        beta_routes.insert("/v2/version/11112222".into(), beta_current);
        beta_routes.insert("/v2/project/AAAABBBB/version".into(), json!([beta, stable]));
        let beta_server = server(beta_routes);
        let beta_client = Client::for_testing(&format!("{}/v2/", beta_server.base_url()));
        assert_eq!(
            beta_client
                .update_candidate(&context(), &record)
                .await
                .unwrap()
                .unwrap()
                .id,
            "33334444"
        );
        record.file_id = "c".repeat(128);
        assert!(matches!(
            beta_client.update_candidate(&context(), &record).await,
            Err(Error::InvalidResponse)
        ));
    }

    #[tokio::test]
    async fn update_check_distinguishes_no_candidate_and_provider_failures() {
        let mut record = ProviderRecord {
            content_type: ContentType::Mod,
            provider: "modrinth".into(),
            project_id: "AAAABBBB".into(),
            version_id: "11112222".into(),
            file_id: "a".repeat(128),
            file_name: "11112222.jar".into(),
            sha256: "b".repeat(64),
            display_version: Some("1.0.0".into()),
            compatibility: ContentCompatibility {
                minecraft_versions: vec!["1.21.11".into()],
                loader: Some("fabric".into()),
                environment: Some("client_and_server".into()),
            },
            dependencies: vec![],
            explicitly_retained: true,
            requires: vec![],
        };
        let mut routes = HashMap::new();
        routes.insert(
            "/v2/version/11112222".into(),
            version("11112222", "AAAABBBB", json!([])),
        );
        routes.insert(
            "/v2/project/AAAABBBB/version".into(),
            json!([version("11112222", "AAAABBBB", json!([]))]),
        );
        let no_update = server(routes);
        let client = Client::for_testing(&format!("{}/v2/", no_update.base_url()));
        assert!(
            client
                .update_candidate(&context(), &record)
                .await
                .unwrap()
                .is_none()
        );

        record.version_id = "99998888".into();
        assert!(matches!(
            client.update_candidate(&context(), &record).await,
            Err(Error::NotFound)
        ));
        record.version_id = "11112222".into();

        let rate = TestServer::spawn(Arc::new(|_| {
            TestResponse::status(429).with_header("X-Ratelimit-Reset", "12")
        }));
        let client = Client::for_testing(&format!("{}/v2/", rate.base_url()));
        assert!(matches!(
            client.update_candidate(&context(), &record).await,
            Err(Error::RateLimited(Some(12)))
        ));

        let malformed =
            TestServer::spawn(Arc::new(|_| TestResponse::ok(br#"{"unexpected":true}"#)));
        let client = Client::for_testing(&format!("{}/v2/", malformed.base_url()));
        assert!(matches!(
            client.update_candidate(&context(), &record).await,
            Err(Error::InvalidResponse)
        ));

        let client = Client::for_testing("http://127.0.0.1:0/v2/");
        assert!(matches!(
            client.update_candidate(&context(), &record).await,
            Err(Error::Network)
        ));
    }

    #[tokio::test]
    async fn version_choice_prefers_release_and_rejects_wrong_loader() {
        let mut routes = HashMap::new();
        routes.insert("/v2/project/AAAABBBB".into(), project("AAAABBBB", "mod"));
        let mut beta = version("BBBBCCCC", "AAAABBBB", json!([]));
        beta["version_type"] = json!("beta");
        beta["date_published"] = json!("2026-03-01T00:00:00Z");
        let stable = version("CCCCDDDD", "AAAABBBB", json!([]));
        let mut wrong = version("DDDDEEEE", "AAAABBBB", json!([]));
        wrong["loaders"] = json!(["forge"]);
        routes.insert(
            "/v2/project/AAAABBBB/version".into(),
            json!([beta, stable, wrong]),
        );
        let server = server(routes);
        let client = Client::for_testing(&format!("{}/v2/", server.base_url()));
        let details = client
            .details(&context(), ContentType::Mod, "AAAABBBB")
            .await
            .unwrap();
        assert_eq!(details.versions.len(), 2);
        assert_eq!(details.default_version_id.as_deref(), Some("CCCCDDDD"));
    }

    #[tokio::test]
    async fn required_graph_is_transitive_and_optional_is_not_installed() {
        let mut routes = HashMap::new();
        for id in ["AAAABBBB", "BBBBCCCC", "CCCCDDDD"] {
            routes.insert(format!("/v2/project/{id}"), project(id, "mod"));
        }
        let root = version(
            "11112222",
            "AAAABBBB",
            json!([
                {"project_id": "BBBBCCCC", "version_id": null, "dependency_type": "required"},
                {"project_id": "BBBBCCCC", "version_id": null, "dependency_type": "required"},
                {"project_id": "CCCCDDDD", "version_id": null, "dependency_type": "optional"}
            ]),
        );
        let child = version(
            "22223333",
            "BBBBCCCC",
            json!([
                {"project_id": "CCCCDDDD", "version_id": null, "dependency_type": "required"}
            ]),
        );
        let leaf = version("33334444", "CCCCDDDD", json!([]));
        routes.insert("/v2/version/11112222".into(), root);
        routes.insert("/v2/project/BBBBCCCC/version".into(), json!([child]));
        routes.insert("/v2/project/CCCCDDDD/version".into(), json!([leaf]));
        let server = server(routes);
        let client = Client::for_testing(&format!("{}/v2/", server.base_url()));
        let graph = client
            .resolve(
                &context(),
                ContentType::Mod,
                "AAAABBBB",
                "11112222",
                &ContentState::empty(),
            )
            .await
            .unwrap();
        assert_eq!(graph.plans.len(), 3);
        assert_eq!(graph.preview.items[0].project_id, "CCCCDDDD");
        assert_eq!(graph.preview.items[2].project_id, "AAAABBBB");
        assert!(
            graph
                .preview
                .warnings
                .iter()
                .any(|item| item.contains("Optional"))
        );
    }

    #[tokio::test]
    async fn cycle_and_rate_limit_fail_with_distinct_codes() {
        let mut routes = HashMap::new();
        for id in ["AAAABBBB", "BBBBCCCC"] {
            routes.insert(format!("/v2/project/{id}"), project(id, "mod"));
        }
        routes.insert(
            "/v2/version/11112222".into(),
            version(
                "11112222",
                "AAAABBBB",
                json!([
                    {"project_id": "BBBBCCCC", "version_id": null, "dependency_type": "required"}
                ]),
            ),
        );
        routes.insert(
            "/v2/project/BBBBCCCC/version".into(),
            json!([version(
                "22223333",
                "BBBBCCCC",
                json!([
                    {"project_id": "AAAABBBB", "version_id": null, "dependency_type": "required"}
                ])
            )]),
        );
        let server = server(routes);
        let client = Client::for_testing(&format!("{}/v2/", server.base_url()));
        assert!(matches!(
            client
                .resolve(
                    &context(),
                    ContentType::Mod,
                    "AAAABBBB",
                    "11112222",
                    &ContentState::empty()
                )
                .await,
            Err(Error::DependencyCycle)
        ));
        let rate = TestServer::spawn(Arc::new(|_| {
            TestResponse::status(429).with_header("X-Ratelimit-Reset", "12")
        }));
        let client = Client::for_testing(&format!("{}/v2/", rate.base_url()));
        assert!(matches!(
            client.search(&context(), ContentType::Mod, "", 0).await,
            Err(Error::RateLimited(Some(12)))
        ));
    }

    #[test]
    fn primary_file_is_deterministic_and_unsafe_primary_is_rejected() {
        let mut value = version("11112222", "AAAABBBB", json!([]));
        value["files"] = json!([
            {"hashes":{"sha512":"a".repeat(128)},"url":"https://cdn.modrinth.com/a","filename":"source.jar","primary":false,"size":4,"file_type":"sources-jar"},
            {"hashes":{"sha512":"b".repeat(128)},"url":"https://cdn.modrinth.com/b","filename":"release.jar","primary":true,"size":4,"file_type":null}
        ]);
        let parsed: VersionDto = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            choose_file(&parsed, ContentType::Mod).unwrap().filename,
            "release.jar"
        );
        value["files"][1]["filename"] = json!("../escape.jar");
        let parsed: VersionDto = serde_json::from_value(value).unwrap();
        assert!(choose_file(&parsed, ContentType::Mod).is_err());
    }

    #[test]
    fn pack_and_shader_version_rules_are_distinct() {
        let mut pack: VersionDto =
            serde_json::from_value(version("11112222", "AAAABBBB", json!([]))).unwrap();
        pack.loaders = vec!["minecraft".into()];
        pack.environment = "unknown".into();
        assert!(compatible(&context(), ContentType::ResourcePack, &pack));
        assert!(!compatible(&context(), ContentType::Mod, &pack));
        pack.loaders = vec!["iris".into()];
        assert!(compatible(&context(), ContentType::ShaderPack, &pack));
        assert!(!compatible(&context(), ContentType::ResourcePack, &pack));
        pack.game_versions = vec!["1.20.1".into()];
        assert!(!compatible(&context(), ContentType::ShaderPack, &pack));
    }

    #[tokio::test]
    async fn protected_fabric_api_cannot_be_a_provider_root() {
        let client = Client::for_testing("http://127.0.0.1:1/v2/");
        assert!(matches!(
            client
                .resolve(
                    &context(),
                    ContentType::Mod,
                    FABRIC_API_PROJECT,
                    "11112222",
                    &ContentState::empty()
                )
                .await,
            Err(Error::DependencyConflict)
        ));
    }

    #[tokio::test]
    #[ignore = "uses the live public Modrinth API and downloads real provider artifacts"]
    async fn live_mod_menu_resolves_and_installs_into_a_disposable_root() {
        let client = Client::official();
        let context = context();
        let page = client
            .search(&context, ContentType::Mod, "Mod Menu", 0)
            .await
            .unwrap();
        assert!(page.hits.iter().any(|hit| hit.project_id == "mOgUt4GM"));
        let details = client
            .details(&context, ContentType::Mod, "mOgUt4GM")
            .await
            .unwrap();
        let version = details.default_version_id.unwrap();
        let root = std::env::temp_dir().join(format!(
            "aurora-modrinth-acceptance-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(root.join("instances")).unwrap();
        let instance =
            crate::instances::InstanceId::new(uuid::Uuid::new_v4().simple().to_string()).unwrap();
        std::fs::create_dir(root.join("instances").join(instance.as_str())).unwrap();
        let managed = crate::paths::ManagedPaths::from_app_local_data_dir(root.clone()).unwrap();
        let resolved = client
            .resolve(
                &context,
                ContentType::Mod,
                "mOgUt4GM",
                &version,
                &ContentState::empty(),
            )
            .await
            .unwrap();
        assert!(resolved.preview.items.len() >= 2);
        assert!(
            resolved
                .preview
                .warnings
                .iter()
                .any(|warning| warning.contains("Fabric API"))
        );
        let installed =
            crate::instance_content::install_provider_plans(&managed, &instance, resolved.plans)
                .await
                .unwrap();
        assert!(
            installed
                .iter()
                .any(|record| record.project_id == "mOgUt4GM")
        );
        let inventory = crate::instance_mods::scan(&managed, &instance).unwrap();
        assert!(inventory.entries.iter().any(|entry| entry.ownership == crate::instance_mods::ModOwnership::ProviderManaged));
        eprintln!("live Modrinth acceptance root: {}", root.display());
    }
}
