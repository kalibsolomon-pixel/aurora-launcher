//! Launcher appearance preferences: the built-in theme catalog, the curated
//! accent palette, deterministic custom-accent derivation, and the validation
//! that keeps every selectable combination readable.
//!
//! This module is the appearance domain of the launcher configuration. It owns
//! the vocabulary that can be persisted (`ThemeId`, `AccentSelection`), the
//! curated palettes behind both, and the one derivation rule that turns an
//! arbitrary custom accent color into the full set of CSS accent tokens. It
//! never touches the filesystem; persistence lives in `config`.

use serde::{Deserialize, Serialize};

/// The built-in launcher themes. All three are dark; they differ through a
/// coherent surface palette, never through layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeId {
    /// The approved default: neutral near-black surfaces, restrained violet.
    AuroraDark,
    /// A slightly cooler, deeper graphite/slate variant.
    Midnight,
    /// True-black major surfaces for maximum darkness.
    Oled,
}

impl ThemeId {
    /// The stable persisted identifier (also the CSS `data-theme` value).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AuroraDark => "aurora-dark",
            Self::Midnight => "midnight",
            Self::Oled => "oled",
        }
    }

    /// Parses a persisted or requested theme identifier. Unknown values are
    /// `None` so callers can decide between rejection (a user request) and
    /// normalization to the default (a hand-edited file).
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "aurora-dark" => Some(Self::AuroraDark),
            "midnight" => Some(Self::Midnight),
            "oled" => Some(Self::Oled),
            _ => None,
        }
    }

    /// The user-facing label shown by the Settings page.
    pub fn label(self) -> &'static str {
        match self {
            Self::AuroraDark => "Aurora Dark",
            Self::Midnight => "Midnight",
            Self::Oled => "OLED Black",
        }
    }

    /// One line describing the theme's character.
    pub fn description(self) -> &'static str {
        match self {
            Self::AuroraDark => "The default Aurora look — neutral near-black surfaces.",
            Self::Midnight => "Cooler, deeper graphite surfaces with a subtle slate character.",
            Self::Oled => "True-black major surfaces for maximum darkness.",
        }
    }

    /// Every built-in theme, in presentation order.
    pub fn all() -> [Self; 3] {
        [Self::AuroraDark, Self::Midnight, Self::Oled]
    }
}

impl Default for ThemeId {
    fn default() -> Self {
        Self::AuroraDark
    }
}

/// An accent color as validated sRGB hex (`#rrggbb`, case-insensitive input,
/// canonical lowercase output).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccentHex {
    red: u8,
    green: u8,
    blue: u8,
}

/// Below this relative luminance a custom accent cannot stay visible as a
/// focus ring or selection marker on the darkest supported surface (#000000
/// under the OLED theme); requesting one is rejected with an explanation.
const MIN_CUSTOM_ACCENT_LUMINANCE: f64 = 0.08;

impl AccentHex {
    /// Parses `#rrggbb` or `rrggbb` (case-insensitive).
    pub fn parse(value: &str) -> Result<Self, InvalidAccent> {
        let trimmed = value.trim().trim_start_matches('#');
        if trimmed.len() != 6 || !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(InvalidAccent {
                reason: format!("'{value}' is not a six-digit hex color such as #8b80ff"),
            });
        }

        let channel = |slice: &str| u8::from_str_radix(slice, 16).unwrap();
        Ok(Self {
            red: channel(&trimmed[0..2]),
            green: channel(&trimmed[2..4]),
            blue: channel(&trimmed[4..6]),
        })
    }

    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    /// Canonical `#rrggbb` form (directly usable as a CSS color).
    pub fn as_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.red, self.green, self.blue)
    }

    fn rgb(self) -> (f64, f64, f64) {
        (
            f64::from(self.red) / 255.0,
            f64::from(self.green) / 255.0,
            f64::from(self.blue) / 255.0,
        )
    }

    /// WCAG 2.1 relative luminance.
    fn relative_luminance(self) -> f64 {
        let channel = |value: f64| {
            if value <= 0.03928 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = self.rgb();
        0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
    }

    /// WCAG 2.1 contrast ratio against another color.
    fn contrast_ratio(self, other: Self) -> f64 {
        let a = self.relative_luminance();
        let b = other.relative_luminance();
        let (higher, lower) = if a >= b { (a, b) } else { (b, a) };
        (higher + 0.05) / (lower + 0.05)
    }

    fn to_hsl(self) -> (f64, f64, f64) {
        let (r, g, b) = self.rgb();
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let lightness = (max + min) / 2.0;
        if (max - min).abs() < f64::EPSILON {
            return (0.0, 0.0, lightness);
        }
        let delta = max - min;
        let saturation = if lightness > 0.5 {
            delta / (2.0 - max - min)
        } else {
            delta / (max + min)
        };
        let hue = if (max - r).abs() < f64::EPSILON {
            ((g - b) / delta + if g < b { 6.0 } else { 0.0 }) / 6.0
        } else if (max - g).abs() < f64::EPSILON {
            ((b - r) / delta + 2.0) / 6.0
        } else {
            ((r - g) / delta + 4.0) / 6.0
        };
        (hue * 360.0, saturation, lightness)
    }

    fn from_hsl(hue: f64, saturation: f64, lightness: f64) -> Self {
        let hue = (((hue % 360.0) + 360.0) % 360.0) / 360.0;
        let channel = |n: f64| {
            let k = (n + hue * 12.0) % 12.0;
            let a = saturation * lightness.min(1.0 - lightness);
            lightness - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
        };
        Self {
            red: (channel(0.0) * 255.0).round() as u8,
            green: (channel(8.0) * 255.0).round() as u8,
            blue: (channel(4.0) * 255.0).round() as u8,
        }
    }
}

/// The text color placed on accent-filled controls.
const ACCENT_TEXT_WHITE: AccentHex = AccentHex::new(0xff, 0xff, 0xff);
/// Near-black ink for bright accent fills (cyan/green/amber/neutral).
const ACCENT_TEXT_INK: AccentHex = AccentHex::new(0x10, 0x11, 0x16);

/// The complete set of accent-family CSS token values for one accent color.
///
/// These map one-to-one onto the `--color-accent*` custom properties: the
/// base (markers, focus ring), the primary-button fill and its hover/pressed
/// variants, the text color that stays readable on those fills, and the
/// translucent tints used for selected backgrounds and soft emphasis.
#[derive(Debug, Clone, PartialEq)]
pub struct AccentPalette {
    pub base: AccentHex,
    pub strong: AccentHex,
    pub hover: AccentHex,
    pub pressed: AccentHex,
    pub on_accent: AccentHex,
    /// Selected-background tint over dark surfaces.
    pub soft_alpha: f64,
    /// Focus/soft outline tint.
    pub outline_alpha: f64,
}

impl AccentPalette {
    /// CSS `rgba(...)` value for the selected-background tint.
    pub fn soft_css(&self) -> String {
        format!(
            "rgba({}, {}, {}, {:.2})",
            self.base.red, self.base.green, self.base.blue, self.soft_alpha
        )
    }

    /// CSS `rgba(...)` value for the outline tint.
    pub fn outline_css(&self) -> String {
        format!(
            "rgba({}, {}, {}, {:.2})",
            self.base.red, self.base.green, self.base.blue, self.outline_alpha
        )
    }
}

/// One curated accent preset.
pub struct AccentPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub palette: AccentPalette,
}

/// The curated accent palette, in presentation order.
///
/// Every preset was designed (and is verified by tests) against the dark
/// surfaces: the base stays visible as a focus ring on #000000, and the
/// button fill keeps ≥ 4.5:1 contrast with its text color in default, hover,
/// and pressed states.
pub fn accent_presets() -> &'static [AccentPreset] {
    static PRESETS: &[AccentPreset] = &[
        AccentPreset {
            id: "violet",
            label: "Aurora violet",
            palette: AccentPalette {
                base: AccentHex::new(0x8b, 0x80, 0xff),
                strong: AccentHex::new(0x6f, 0x5d, 0xf2),
                hover: AccentHex::new(0x6f, 0x63, 0xe8),
                pressed: AccentHex::new(0x61, 0x52, 0xe0),
                on_accent: ACCENT_TEXT_WHITE,
                soft_alpha: 0.14,
                outline_alpha: 0.55,
            },
        },
        AccentPreset {
            id: "blue",
            label: "Blue",
            palette: AccentPalette {
                base: AccentHex::new(0x7b, 0xa4, 0xff),
                strong: AccentHex::new(0x3a, 0x6f, 0xd8),
                hover: AccentHex::new(0x42, 0x73, 0xd6),
                pressed: AccentHex::new(0x35, 0x63, 0xc8),
                on_accent: ACCENT_TEXT_WHITE,
                soft_alpha: 0.14,
                outline_alpha: 0.55,
            },
        },
        AccentPreset {
            id: "cyan",
            label: "Cyan",
            palette: AccentPalette {
                base: AccentHex::new(0x5b, 0xd0, 0xe0),
                strong: AccentHex::new(0x37, 0xb3, 0xc6),
                hover: AccentHex::new(0x43, 0xbc, 0xcd),
                pressed: AccentHex::new(0x2f, 0xa7, 0xb8),
                on_accent: ACCENT_TEXT_INK,
                soft_alpha: 0.14,
                outline_alpha: 0.55,
            },
        },
        AccentPreset {
            id: "green",
            label: "Green",
            palette: AccentPalette {
                base: AccentHex::new(0x5e, 0xc9, 0x8f),
                strong: AccentHex::new(0x3a, 0xa9, 0x6d),
                hover: AccentHex::new(0x46, 0xb1, 0x77),
                pressed: AccentHex::new(0x32, 0x99, 0x62),
                on_accent: ACCENT_TEXT_INK,
                soft_alpha: 0.14,
                outline_alpha: 0.55,
            },
        },
        AccentPreset {
            id: "amber",
            label: "Amber",
            palette: AccentPalette {
                base: AccentHex::new(0xe2, 0xb6, 0x55),
                strong: AccentHex::new(0xd0, 0xa0, 0x3e),
                hover: AccentHex::new(0xd7, 0xab, 0x4c),
                pressed: AccentHex::new(0xc2, 0x95, 0x3a),
                on_accent: ACCENT_TEXT_INK,
                soft_alpha: 0.14,
                outline_alpha: 0.55,
            },
        },
        AccentPreset {
            id: "rose",
            label: "Rose",
            palette: AccentPalette {
                base: AccentHex::new(0xef, 0x8f, 0xa5),
                strong: AccentHex::new(0xb0, 0x3a, 0x58),
                hover: AccentHex::new(0xc0, 0x4a, 0x68),
                pressed: AccentHex::new(0xa1, 0x33, 0x50),
                on_accent: ACCENT_TEXT_WHITE,
                soft_alpha: 0.14,
                outline_alpha: 0.55,
            },
        },
        AccentPreset {
            id: "neutral",
            label: "Neutral",
            palette: AccentPalette {
                base: AccentHex::new(0xdf, 0xe3, 0xee),
                strong: AccentHex::new(0xc6, 0xcc, 0xdb),
                hover: AccentHex::new(0xd1, 0xd7, 0xe6),
                pressed: AccentHex::new(0xba, 0xc1, 0xd2),
                on_accent: ACCENT_TEXT_INK,
                soft_alpha: 0.12,
                outline_alpha: 0.50,
            },
        },
    ];
    PRESETS
}

/// Finds a curated preset by id.
pub fn accent_preset(id: &str) -> Option<&'static AccentPreset> {
    accent_presets()
        .iter()
        .find(|preset| preset.id == id.trim())
}

/// The user's accent selection: a curated preset id or a validated custom
/// color. This is the persisted vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum AccentSelection {
    Preset { id: String },
    Custom { hex: String },
}

impl Default for AccentSelection {
    fn default() -> Self {
        Self::Preset {
            id: "violet".to_owned(),
        }
    }
}

impl AccentSelection {
    /// The curated preset this selection names, if any.
    pub fn preset(&self) -> Option<&'static AccentPreset> {
        match self {
            Self::Preset { id } => accent_preset(id),
            Self::Custom { .. } => None,
        }
    }

    /// Resolves the full palette for this selection.
    ///
    /// Presets return their curated palette; custom colors run the
    /// deterministic derivation below. Unknown preset ids and invalid custom
    /// colors are rejected — normalization on load happens in `config`.
    pub fn palette(&self) -> Result<AccentPalette, InvalidAccent> {
        match self {
            Self::Preset { id } => self
                .preset()
                .map(|preset| preset.palette.clone())
                .ok_or_else(|| InvalidAccent {
                    reason: format!("'{id}' is not a known accent preset"),
                }),
            Self::Custom { hex } => derive_custom_accent(hex),
        }
    }
}

/// An accent value that cannot produce a usable, accessible UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidAccent {
    pub reason: String,
}

impl std::fmt::Display for InvalidAccent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "the accent color is not usable: {}", self.reason)
    }
}

impl std::error::Error for InvalidAccent {}

/// Deterministically derives the full accent palette for a custom color.
///
/// Rules, applied in order:
///
/// 1. The base must parse as `#rrggbb` and stay visible as a focus ring on a
///    true-black surface (relative luminance ≥ 0.08; curated presets exceed
///    3:1 against #000000). Darker requests are rejected with an explanation
///    rather than silently brightened into a different color.
/// 2. The primary-button fill keeps the base hue and saturation with HSL
///    lightness clamped into a solid-fill band; the text color is white or
///    ink — whichever reaches ≥ 4.5:1 contrast. A fill where neither passes
///    (a mid-luminance desaturated gray) is darkened step by step until
///    white text passes.
/// 3. Interaction states shift the fill by fixed HSL-lightness deltas in the
///    direction that only increases text contrast for the chosen polarity —
///    white text darkens the fill (hover −0.03, pressed −0.07), ink text
///    brightens it (hover +0.05, pressed +0.02) — so every state stays
///    ≥ 4.5:1 by construction and no state collapses into the resting fill.
/// 4. Tints are translucent versions of the untouched base color.
pub fn derive_custom_accent(hex: &str) -> Result<AccentPalette, InvalidAccent> {
    let base = AccentHex::parse(hex)?;
    if base.relative_luminance() < MIN_CUSTOM_ACCENT_LUMINANCE {
        return Err(InvalidAccent {
            reason: format!(
                "{} is too dark to stay visible on Aurora's dark surfaces; choose a brighter color",
                base.as_hex()
            ),
        });
    }

    let (hue, saturation, base_lightness) = base.to_hsl();

    // The solid-fill band for primary buttons: deep enough to feel like a
    // fill, bright enough to stay clearly colored.
    let fill_lightness = base_lightness.clamp(0.34, 0.62);
    let mut strong = AccentHex::from_hsl(hue, saturation, fill_lightness);

    let on_accent = if strong.contrast_ratio(ACCENT_TEXT_WHITE) >= 4.5 {
        ACCENT_TEXT_WHITE
    } else if strong.contrast_ratio(ACCENT_TEXT_INK) >= 4.5 {
        ACCENT_TEXT_INK
    } else {
        // Mid-luminance fills (typically desaturated ones) serve neither
        // polarity; darken until white text becomes readable.
        let mut lightness = fill_lightness;
        while lightness > 0.10 {
            lightness = (lightness - 0.05).max(0.10);
            strong = AccentHex::from_hsl(hue, saturation, lightness);
            if strong.contrast_ratio(ACCENT_TEXT_WHITE) >= 4.5 {
                break;
            }
        }
        ACCENT_TEXT_WHITE
    };

    let (hover_delta, pressed_delta) = if on_accent == ACCENT_TEXT_WHITE {
        (-0.03, -0.07)
    } else {
        (0.05, 0.02)
    };
    let hover = AccentHex::from_hsl(
        hue,
        saturation,
        (fill_lightness + hover_delta).clamp(0.05, 0.95),
    );
    let pressed = AccentHex::from_hsl(
        hue,
        saturation,
        (fill_lightness + pressed_delta).clamp(0.05, 0.95),
    );

    Ok(AccentPalette {
        base,
        strong,
        hover,
        pressed,
        on_accent,
        soft_alpha: 0.14,
        outline_alpha: 0.55,
    })
}

/// The launcher-wide appearance preferences persisted in the configuration.
///
/// Deliberately minimal: the theme and the accent. Further appearance
/// options are added only when implemented behavior needs them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppearancePreferences {
    pub theme: String,
    pub accent: AccentSelection,
}

impl AppearancePreferences {
    /// Preferences with the default Aurora look.
    pub fn new() -> Self {
        Self {
            theme: ThemeId::default().as_str().to_owned(),
            accent: AccentSelection::default(),
        }
    }

    /// The parsed theme, defaulting when the stored id is unknown (a
    /// hand-edited or forward-produced file; appearance never blocks
    /// startup).
    pub fn theme_id(&self) -> ThemeId {
        ThemeId::parse(&self.theme).unwrap_or_default()
    }

    /// Normalizes unknown or invalid stored values to the defaults.
    ///
    /// Appearance is cosmetic launcher-wide state: a well-formed document
    /// carrying an unknown theme id or unusable accent falls back to the
    /// default look rather than failing the whole configuration. Structurally
    /// malformed documents still fail at the config boundary.
    pub fn normalized(&self) -> Self {
        let mut normalized = self.clone();
        if ThemeId::parse(&normalized.theme).is_none() {
            normalized.theme = ThemeId::default().as_str().to_owned();
        }
        if normalized.accent.palette().is_err() {
            normalized.accent = AccentSelection::default();
        }
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: AccentHex = AccentHex::new(0x00, 0x00, 0x00);

    #[test]
    fn theme_ids_round_trip_through_their_persisted_strings() {
        for theme in ThemeId::all() {
            assert_eq!(ThemeId::parse(theme.as_str()), Some(theme));
            assert!(!theme.label().is_empty());
            assert!(!theme.description().is_empty());
        }
        assert_eq!(ThemeId::parse("aurora-dark"), Some(ThemeId::AuroraDark));
        assert_eq!(ThemeId::parse("neon"), None);
        assert_eq!(ThemeId::parse("  midnight "), Some(ThemeId::Midnight));
    }

    #[test]
    fn accent_hex_parses_and_normalizes() {
        assert_eq!(AccentHex::parse("#8B80FF").unwrap().as_hex(), "#8b80ff");
        assert_eq!(AccentHex::parse("8b80ff").unwrap().as_hex(), "#8b80ff");
        for invalid in ["#8b80f", "8b80ff0", "#zzzzzz", "", "#8b80fg"] {
            assert!(AccentHex::parse(invalid).is_err(), "'{invalid}' parsed");
        }
    }

    #[test]
    fn every_curated_preset_stays_accessible_on_dark_surfaces() {
        for preset in accent_presets() {
            let palette = &preset.palette;
            let label = preset.id;

            // Focus rings and markers must stay visible even on the OLED
            // theme's true black.
            assert!(
                palette.base.contrast_ratio(BLACK) >= 3.0,
                "{label}: base {} has {:.2}:1 against black",
                palette.base.as_hex(),
                palette.base.contrast_ratio(BLACK)
            );

            // Primary-button text must stay readable in every fill state.
            for (state, fill) in [
                ("strong", palette.strong),
                ("hover", palette.hover),
                ("pressed", palette.pressed),
            ] {
                let ratio = fill.contrast_ratio(palette.on_accent);
                assert!(
                    ratio >= 4.5,
                    "{label}: {state} {} vs text {} is only {:.2}:1",
                    fill.as_hex(),
                    palette.on_accent.as_hex(),
                    ratio
                );
            }

            // Interaction states must be distinct from the resting fill.
            assert_ne!(
                palette.hover, palette.strong,
                "{label}: hover equals strong"
            );
            assert_ne!(
                palette.pressed, palette.strong,
                "{label}: pressed equals strong"
            );

            // The tints must be translucent versions of the base, not the
            // same hex pasted into every token.
            assert!(palette.soft_alpha > 0.0 && palette.soft_alpha < 1.0);
            assert!(palette.outline_alpha > 0.0 && palette.outline_alpha < 1.0);
        }
    }

    #[test]
    fn the_violet_preset_reproduces_the_approved_default_accent() {
        let palette = &accent_preset("violet").unwrap().palette;

        assert_eq!(palette.base.as_hex(), "#8b80ff");
        assert_eq!(palette.strong.as_hex(), "#6f5df2");
        assert_eq!(palette.on_accent.as_hex(), "#ffffff");
        assert_eq!(palette.soft_css(), "rgba(139, 128, 255, 0.14)");
        assert_eq!(palette.outline_css(), "rgba(139, 128, 255, 0.55)");
    }

    #[test]
    fn preset_selections_resolve_and_unknown_ids_are_rejected() {
        let selection = AccentSelection::Preset {
            id: "amber".to_owned(),
        };
        assert_eq!(selection.palette().unwrap().base.as_hex(), "#e2b655");

        let unknown = AccentSelection::Preset {
            id: "hotdog".to_owned(),
        };
        assert!(unknown.palette().is_err());
    }

    #[test]
    fn custom_accents_derive_deterministically_with_readable_buttons() {
        for hex in ["#ff5533", "#40E0D0", "#c0ffee", "#dfe3ee", "#9d7bd8"] {
            let palette =
                derive_custom_accent(hex).unwrap_or_else(|e| panic!("'{hex}' rejected: {e}"));

            assert_eq!(
                palette.base.as_hex(),
                AccentHex::parse(hex).unwrap().as_hex()
            );
            for fill in [palette.strong, palette.hover, palette.pressed] {
                let ratio = fill.contrast_ratio(palette.on_accent);
                assert!(
                    ratio >= 4.5,
                    "custom {hex}: fill {} vs {} is only {:.2}:1",
                    fill.as_hex(),
                    palette.on_accent.as_hex(),
                    ratio
                );
            }
            assert!(palette.base.contrast_ratio(BLACK) >= 2.6);
            assert_ne!(
                palette.hover, palette.strong,
                "custom {hex}: hover equals strong"
            );
            assert_ne!(
                palette.pressed, palette.strong,
                "custom {hex}: pressed equals strong"
            );

            // Determinism: the same input produces the same palette.
            assert_eq!(derive_custom_accent(hex).unwrap(), palette);
        }
    }

    #[test]
    fn custom_accent_input_is_validated_with_explanations() {
        let error = derive_custom_accent("not-a-color").unwrap_err();
        assert!(error.reason.contains("six-digit hex"));

        let too_dark = derive_custom_accent("#050505").unwrap_err();
        assert!(too_dark.reason.contains("too dark"));

        // The curated violet itself must be a legal custom color.
        assert!(derive_custom_accent("#8b80ff").is_ok());
    }

    #[test]
    fn appearance_preferences_normalize_unknown_values_to_the_default_look() {
        let hand_edited = AppearancePreferences {
            theme: "neon".to_owned(),
            accent: AccentSelection::Preset {
                id: "hotdog".to_owned(),
            },
        };
        let normalized = hand_edited.normalized();
        assert_eq!(normalized.theme_id(), ThemeId::AuroraDark);
        assert_eq!(normalized.accent, AccentSelection::default());

        let unusable_custom = AppearancePreferences {
            theme: "oled".to_owned(),
            accent: AccentSelection::Custom {
                hex: "#000000".to_owned(),
            },
        };
        assert_eq!(
            unusable_custom.normalized().accent,
            AccentSelection::default()
        );
        assert_eq!(unusable_custom.normalized().theme_id(), ThemeId::Oled);

        // Valid preferences survive normalization untouched.
        let valid = AppearancePreferences {
            theme: "midnight".to_owned(),
            accent: AccentSelection::Custom {
                hex: "#FF5533".to_owned(),
            },
        };
        assert_eq!(valid.normalized(), valid);
    }

    #[test]
    fn appearance_preferences_serialize_to_inspectable_camel_case() {
        let preferences = AppearancePreferences::new();
        let json = serde_json::to_string_pretty(&preferences).unwrap();

        assert!(json.contains("\"theme\": \"aurora-dark\""));
        assert!(json.contains("\"accent\": {"));
        assert!(json.contains("\"type\": \"preset\""));
        assert!(json.contains("\"id\": \"violet\""));

        let parsed: AppearancePreferences = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, preferences);
    }

    #[test]
    fn a_custom_selection_serializes_with_its_hex() {
        let selection = AccentSelection::Custom {
            hex: "#ff5533".to_owned(),
        };
        let json = serde_json::to_string(&selection).unwrap();
        assert!(json.contains("\"type\":\"custom\""));
        assert!(json.contains("\"hex\":\"#ff5533\""));

        let parsed: AccentSelection = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, selection);
    }
}
