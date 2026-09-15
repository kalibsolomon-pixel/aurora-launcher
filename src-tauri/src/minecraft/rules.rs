//! Rule semantics for official Minecraft version metadata.
//!
//! Modern Mojang version documents attach rule lists to libraries and launch
//! arguments. A rule matches when its conditions hold for the current
//! platform (and, for feature conditions, the launch feature profile), and the
//! last matching rule's action decides the outcome; when a rule list is
//! present but no rule matches, the item does not apply.
//!
//! Observed modern scope (verified against the official `1.21.11` and `26.2`
//! release documents):
//!
//! - library rules use `os.name` (`windows`, `linux`, `osx`) only;
//! - JVM argument rules additionally use `os.arch` (`x86`);
//! - game argument rules use feature flags only.
//!
//! Evaluation here is pure and deterministic, isolated from I/O and the UI,
//! and explicit about defaults, so Windows/Linux/macOS decisions are testable
//! as plain logic rather than assumed from the development machine.

use std::collections::{BTreeMap, HashSet};
use std::fmt;

use serde::Deserialize;

/// An operating system in Mojang's rule vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
pub enum OperatingSystem {
    #[serde(rename = "windows")]
    Windows,
    #[serde(rename = "linux")]
    Linux,
    #[serde(rename = "osx")]
    MacOs,
}

impl OperatingSystem {
    /// The Mojang vocabulary spelling, as used inside rule conditions.
    pub fn as_mojang_str(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::MacOs => "osx",
        }
    }
}

impl fmt::Display for OperatingSystem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_mojang_str())
    }
}

/// A CPU architecture in Mojang's rule vocabulary.
///
/// Only `x86` (32-bit x86) is known to appear in official metadata today; the
/// other spellings are accepted so a future document is not rejected for
/// vocabulary alone. Comparison is exact equality within this vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
pub enum Architecture {
    #[serde(rename = "x86")]
    X86,
    #[serde(rename = "x86_64")]
    X86_64,
    #[serde(rename = "arm64")]
    Arm64,
}

impl Architecture {
    pub fn as_mojang_str(self) -> &'static str {
        match self {
            Self::X86 => "x86",
            Self::X86_64 => "x86_64",
            Self::Arm64 => "arm64",
        }
    }
}

impl fmt::Display for Architecture {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_mojang_str())
    }
}

/// The platform an install plan is resolved for.
///
/// Deliberately explicit: nothing in rule evaluation reads the environment, so
/// the same document plans identically on any host once the profile is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlatformProfile {
    os: OperatingSystem,
    arch: Architecture,
}

impl PlatformProfile {
    pub const fn new(os: OperatingSystem, arch: Architecture) -> Self {
        Self { os, arch }
    }

    /// The profile of the machine running the launcher.
    ///
    /// Returns an error for operating systems or architectures outside the
    /// launcher's supported desktop targets rather than guessing a mapping.
    pub fn current() -> Result<Self, UnsupportedPlatform> {
        let os = match std::env::consts::OS {
            "windows" => OperatingSystem::Windows,
            "linux" => OperatingSystem::Linux,
            "macos" => OperatingSystem::MacOs,
            other => return Err(UnsupportedPlatform::Os(other)),
        };

        let arch = match std::env::consts::ARCH {
            "x86" => Architecture::X86,
            "x86_64" => Architecture::X86_64,
            "aarch64" => Architecture::Arm64,
            other => return Err(UnsupportedPlatform::Arch(other)),
        };

        Ok(Self { os, arch })
    }

    pub fn os(self) -> OperatingSystem {
        self.os
    }

    pub fn arch(self) -> Architecture {
        self.arch
    }
}

/// The launcher does not recognize this desktop platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedPlatform {
    Os(&'static str),
    Arch(&'static str),
}

impl fmt::Display for UnsupportedPlatform {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Os(name) => write!(
                formatter,
                "the launcher does not support planning for the operating system '{name}'"
            ),
            Self::Arch(name) => write!(
                formatter,
                "the launcher does not support planning for the CPU architecture '{name}'"
            ),
        }
    }
}

impl std::error::Error for UnsupportedPlatform {}

/// A launcher feature flag that game arguments may be conditioned on.
///
/// Feature flags are launch-time decisions (demo mode, window resolution,
/// quick-play targets). Planning deliberately knows the vocabulary but enables
/// none of them: feature-conditioned arguments stay unresolved in the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Deserialize)]
pub enum FeatureFlag {
    #[serde(rename = "is_demo_user")]
    IsDemoUser,
    #[serde(rename = "has_custom_resolution")]
    HasCustomResolution,
    #[serde(rename = "has_quick_plays_support")]
    HasQuickPlaysSupport,
    #[serde(rename = "is_quick_play_singleplayer")]
    IsQuickPlaySingleplayer,
    #[serde(rename = "is_quick_play_multiplayer")]
    IsQuickPlayMultiplayer,
    #[serde(rename = "is_quick_play_realms")]
    IsQuickPlayRealms,
}

impl FeatureFlag {
    pub fn as_mojang_str(self) -> &'static str {
        match self {
            Self::IsDemoUser => "is_demo_user",
            Self::HasCustomResolution => "has_custom_resolution",
            Self::HasQuickPlaysSupport => "has_quick_plays_support",
            Self::IsQuickPlaySingleplayer => "is_quick_play_singleplayer",
            Self::IsQuickPlayMultiplayer => "is_quick_play_multiplayer",
            Self::IsQuickPlayRealms => "is_quick_play_realms",
        }
    }
}

impl fmt::Display for FeatureFlag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_mojang_str())
    }
}

/// The concrete launch feature decisions used for full rule evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FeatureProfile {
    enabled: HashSet<FeatureFlag>,
}

impl FeatureProfile {
    /// No features enabled: the default launch profile.
    pub fn none() -> Self {
        Self::default()
    }

    pub fn enabling(flag: FeatureFlag) -> Self {
        let mut profile = Self::none();
        profile.enable(flag);
        profile
    }

    pub fn enable(&mut self, flag: FeatureFlag) {
        self.enabled.insert(flag);
    }

    pub fn is_enabled(&self, flag: FeatureFlag) -> bool {
        self.enabled.contains(&flag)
    }
}

/// Whether a rule allows or disallows the item it is attached to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    Allow,
    Disallow,
}

/// The `os` condition of a rule. Absent fields match any platform.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OsCondition {
    #[serde(default)]
    pub name: Option<OperatingSystem>,
    #[serde(default)]
    pub arch: Option<Architecture>,
}

impl OsCondition {
    /// Whether the platform satisfies this condition. A condition with no
    /// fields matches every platform.
    pub fn matches(&self, platform: PlatformProfile) -> bool {
        let name_ok = self.name.is_none_or(|name| name == platform.os());
        let arch_ok = self.arch.is_none_or(|arch| arch == platform.arch());
        name_ok && arch_ok
    }
}

/// The `features` condition of a rule: every listed flag must have the listed
/// value. Official metadata only publishes `true` values.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FeatureConditions(BTreeMap<FeatureFlag, bool>);

impl FeatureConditions {
    /// Whether the launch feature profile satisfies this condition.
    pub fn matches(&self, profile: &FeatureProfile) -> bool {
        self.0
            .iter()
            .all(|(flag, required)| profile.is_enabled(*flag) == *required)
    }

    /// The flags this condition constrains.
    pub fn constrained_flags(&self) -> impl Iterator<Item = FeatureFlag> + '_ {
        self.0.keys().copied()
    }
}

/// One rule from official version metadata.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Rule {
    pub action: RuleAction,
    #[serde(default)]
    pub os: Option<OsCondition>,
    #[serde(default)]
    pub features: Option<FeatureConditions>,
}

impl Rule {
    /// Whether this rule matches the platform and feature profile.
    pub fn matches(&self, platform: PlatformProfile, features: &FeatureProfile) -> bool {
        let os_ok = self.os.as_ref().is_none_or(|os| os.matches(platform));
        let features_ok = self
            .features
            .as_ref()
            .is_none_or(|conditions| conditions.matches(features));
        os_ok && features_ok
    }
}

/// Whether an item with these rules applies to the platform and profile.
///
/// Semantics (the standard launcher interpretation, applied deterministically):
///
/// - an item with no rule list at all applies to everything;
/// - otherwise the last matching rule's action wins;
/// - if a rule list is present but no rule matches, the item does not apply.
pub fn rules_allow(
    rules: Option<&[Rule]>,
    platform: PlatformProfile,
    features: &FeatureProfile,
) -> bool {
    let Some(rules) = rules else {
        return true;
    };

    let mut allowed = false;
    for rule in rules {
        if rule.matches(platform, features) {
            allowed = rule.action == RuleAction::Allow;
        }
    }
    allowed
}

/// The planning decision for an item with feature conditions still unknown.
///
/// Platform conditions resolve now; feature conditions stay unresolved so the
/// plan keeps launch-time choices launch-time. A `disallow` rule that carries
/// feature conditions cannot fire under the default (no-features) profile and
/// therefore does not decide the outcome during planning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanDecision {
    /// The item does not apply to this platform (or a matching rule denied it).
    Excluded,
    /// The item applies unconditionally on this platform.
    Included,
    /// The item applies on this platform only when every listed feature is
    /// enabled at launch.
    IncludedIfFeatures(Vec<FeatureFlag>),
}

/// Resolves rules for planning: platform conditions decide now, feature
/// conditions remain explicit in the result.
pub fn plan_decision(rules: Option<&[Rule]>, platform: PlatformProfile) -> PlanDecision {
    let Some(rules) = rules else {
        return PlanDecision::Included;
    };

    let mut decision = PlanDecision::Excluded;
    let default_features = FeatureProfile::none();

    for rule in rules {
        let Some(os) = rule.os.as_ref() else {
            // A rule without an os condition constrains only features.
            match rule.action {
                RuleAction::Allow => {
                    let flags: Vec<FeatureFlag> = rule
                        .features
                        .as_ref()
                        .map(|conditions| conditions.constrained_flags().collect())
                        .unwrap_or_default();
                    decision = if flags.is_empty() {
                        PlanDecision::Included
                    } else {
                        PlanDecision::IncludedIfFeatures(flags)
                    };
                }
                RuleAction::Disallow => {
                    // A feature-less disallow denies outright; a disallow with
                    // feature conditions cannot fire while features are off.
                    if rule.features.is_none() {
                        decision = PlanDecision::Excluded;
                    }
                }
            }
            continue;
        };

        if !os.matches(platform) {
            continue;
        }

        match rule.action {
            RuleAction::Allow => {
                let flags: Vec<FeatureFlag> = rule
                    .features
                    .as_ref()
                    .map(|conditions| conditions.constrained_flags().collect())
                    .unwrap_or_default();
                decision = if flags.is_empty() {
                    PlanDecision::Included
                } else {
                    PlanDecision::IncludedIfFeatures(flags)
                };
            }
            RuleAction::Disallow => {
                if rule
                    .features
                    .as_ref()
                    .is_none_or(|c| c.matches(&default_features))
                {
                    decision = PlanDecision::Excluded;
                }
            }
        }
    }

    decision
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOWS_X64: PlatformProfile = PlatformProfile {
        os: OperatingSystem::Windows,
        arch: Architecture::X86_64,
    };
    const LINUX_X64: PlatformProfile = PlatformProfile {
        os: OperatingSystem::Linux,
        arch: Architecture::X86_64,
    };
    const MACOS_ARM64: PlatformProfile = PlatformProfile {
        os: OperatingSystem::MacOs,
        arch: Architecture::Arm64,
    };
    const WINDOWS_X86: PlatformProfile = PlatformProfile {
        os: OperatingSystem::Windows,
        arch: Architecture::X86,
    };

    fn allow_os(name: &str) -> Rule {
        serde_json::from_str(&format!(r#"{{"action":"allow","os":{{"name":"{name}"}}}}"#)).unwrap()
    }

    fn disallow_os(name: &str) -> Rule {
        serde_json::from_str(&format!(
            r#"{{"action":"disallow","os":{{"name":"{name}"}}}}"#
        ))
        .unwrap()
    }

    fn allow_arch(arch: &str) -> Rule {
        serde_json::from_str(&format!(r#"{{"action":"allow","os":{{"arch":"{arch}"}}}}"#)).unwrap()
    }

    fn allow_feature(name: &str) -> Rule {
        serde_json::from_str(&format!(
            r#"{{"action":"allow","features":{{"{name}":true}}}}"#
        ))
        .unwrap()
    }

    fn no_features() -> FeatureProfile {
        FeatureProfile::none()
    }

    #[test]
    fn rules_parse_from_official_shapes() {
        let rule: Rule =
            serde_json::from_str(r#"{"action":"allow","os":{"name":"osx","arch":"x86"}}"#).unwrap();
        assert_eq!(
            rule,
            Rule {
                action: RuleAction::Allow,
                os: Some(OsCondition {
                    name: Some(OperatingSystem::MacOs),
                    arch: Some(Architecture::X86),
                }),
                features: None,
            }
        );

        assert!(serde_json::from_str::<Rule>(r#"{"action":"maybe"}"#).is_err());
        assert!(
            serde_json::from_str::<Rule>(r#"{"action":"allow","os":{"name":"plan9"}}"#).is_err()
        );
        assert!(
            serde_json::from_str::<Rule>(&format!(
                r#"{{"action":"allow","features":{{"{}_future":true}}}}"#,
                "some"
            ))
            .is_err(),
            "unknown feature vocabulary is rejected deliberately"
        );
    }

    #[test]
    fn an_item_without_rules_applies_to_every_platform() {
        for platform in [WINDOWS_X64, LINUX_X64, MACOS_ARM64, WINDOWS_X86] {
            assert!(rules_allow(None, platform, &no_features()));
            assert_eq!(plan_decision(None, platform), PlanDecision::Included);
        }
    }

    #[test]
    fn os_allow_rules_select_the_matching_platform_only() {
        let osx = vec![allow_os("osx")];

        assert!(!rules_allow(Some(&osx), WINDOWS_X64, &no_features()));
        assert!(!rules_allow(Some(&osx), LINUX_X64, &no_features()));
        assert!(rules_allow(Some(&osx), MACOS_ARM64, &no_features()));

        assert_eq!(
            plan_decision(Some(&osx), MACOS_ARM64),
            PlanDecision::Included
        );
        assert_eq!(
            plan_decision(Some(&osx), WINDOWS_X64),
            PlanDecision::Excluded
        );
    }

    #[test]
    fn present_rules_that_match_nothing_exclude_by_default() {
        let linux_only = vec![allow_os("linux")];

        assert!(!rules_allow(Some(&linux_only), WINDOWS_X64, &no_features()));
        assert_eq!(
            plan_decision(Some(&linux_only), WINDOWS_X64),
            PlanDecision::Excluded,
            "the documented default for rule lists is exclusion"
        );

        let empty: Vec<Rule> = Vec::new();
        assert!(!rules_allow(Some(&empty), WINDOWS_X64, &no_features()));
    }

    #[test]
    fn the_last_matching_rule_wins() {
        // Allow everywhere except macOS: the classic deny-after-allow shape.
        let rules = vec![allow_os("osx"), disallow_os("osx")];

        assert!(!rules_allow(Some(&rules), MACOS_ARM64, &no_features()));
        assert_eq!(
            plan_decision(Some(&rules), MACOS_ARM64),
            PlanDecision::Excluded
        );

        // Reversed order: macOS ends up allowed.
        let rules = vec![disallow_os("osx"), allow_os("osx")];
        assert!(rules_allow(Some(&rules), MACOS_ARM64, &no_features()));
        assert_eq!(
            plan_decision(Some(&rules), MACOS_ARM64),
            PlanDecision::Included
        );

        // A non-matching later rule never overrides an earlier match.
        let rules = vec![allow_os("windows"), disallow_os("linux")];
        assert!(rules_allow(Some(&rules), WINDOWS_X64, &no_features()));
        assert_eq!(
            plan_decision(Some(&rules), WINDOWS_X64),
            PlanDecision::Included
        );
    }

    #[test]
    fn arch_conditions_match_exact_vocabulary_only() {
        let x86_only = vec![allow_arch("x86")];

        assert!(rules_allow(Some(&x86_only), WINDOWS_X86, &no_features()));
        assert!(!rules_allow(Some(&x86_only), WINDOWS_X64, &no_features()));
        assert!(!rules_allow(Some(&x86_only), MACOS_ARM64, &no_features()));
    }

    #[test]
    fn feature_conditions_evaluate_against_the_profile() {
        let demo = vec![allow_feature("is_demo_user")];

        assert!(!rules_allow(Some(&demo), WINDOWS_X64, &no_features()));
        assert!(rules_allow(
            Some(&demo),
            WINDOWS_X64,
            &FeatureProfile::enabling(FeatureFlag::IsDemoUser)
        ));
        // Feature rules are platform-independent.
        assert!(rules_allow(
            Some(&demo),
            MACOS_ARM64,
            &FeatureProfile::enabling(FeatureFlag::IsDemoUser)
        ));
    }

    #[test]
    fn planning_keeps_feature_conditions_unresolved() {
        let resolution = vec![allow_feature("has_custom_resolution")];

        assert_eq!(
            plan_decision(Some(&resolution), LINUX_X64),
            PlanDecision::IncludedIfFeatures(vec![FeatureFlag::HasCustomResolution])
        );
    }

    #[test]
    fn os_and_feature_conditions_combine_by_conjunction() {
        let rule: Rule = serde_json::from_str(
            r#"{"action":"allow","os":{"name":"windows"},"features":{"is_demo_user":true}}"#,
        )
        .unwrap();

        assert!(rule.matches(
            WINDOWS_X64,
            &FeatureProfile::enabling(FeatureFlag::IsDemoUser)
        ));
        assert!(!rule.matches(WINDOWS_X64, &no_features()));
        assert!(!rule.matches(
            LINUX_X64,
            &FeatureProfile::enabling(FeatureFlag::IsDemoUser)
        ));
    }

    #[test]
    fn an_empty_os_condition_matches_every_platform() {
        let rule: Rule = serde_json::from_str(r#"{"action":"allow","os":{}}"#).unwrap();

        for platform in [WINDOWS_X64, LINUX_X64, MACOS_ARM64] {
            assert!(rule.matches(platform, &no_features()));
        }
    }

    #[test]
    fn every_desktop_platform_combination_decides_identically_in_isolation() {
        // A matrix smoke check: the three operating systems each resolve the
        // official modern rule shapes the way the platform filter expects.
        for (platform, os_name) in [
            (WINDOWS_X64, "windows"),
            (LINUX_X64, "linux"),
            (MACOS_ARM64, "osx"),
        ] {
            let own = vec![allow_os(os_name)];
            assert_eq!(plan_decision(Some(&own), platform), PlanDecision::Included);

            let other = vec![allow_os(if os_name == "windows" {
                "linux"
            } else if os_name == "linux" {
                "osx"
            } else {
                "windows"
            })];
            assert_eq!(
                plan_decision(Some(&other), platform),
                PlanDecision::Excluded
            );
        }
    }
}
