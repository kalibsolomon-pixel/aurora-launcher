# Aurora Launcher design language

This document is the desktop launcher's visual and interaction source of truth. It is
concise enough to implement against and concrete enough that a future screen does not
invent a new visual language. It synthesizes three inputs:

- the **Aurora Client design language** (`DESIGN_LANGUAGE.md` and `GUI_AUDIT.md` in the
  reference-only Aurora-Client repository) — the product family's identity;
- **Pandora Launcher** (Moulberry) — a structural/UX reference for how a desktop
  instance-oriented launcher organizes information;
- the realities of **this codebase**: a Tauri 2 desktop window (default 920×640,
  minimum 720×520), Svelte presentation only, and real Rust-owned launcher state.

## A. Design principles

1. **Functionality before decoration.** Every visual element explains or enables real
   launcher state. No news, ads, promotional banners, cosmetics, or filler.
2. **Hierarchy through position and weight, not volume.** One page title per screen;
   section labels are quiet; metadata is secondary text. No giant hero headings —
   this is a tool, not a landing page.
3. **Grouping over scattering.** Related information lives in one grouped surface with
   hairline-separated rows, not a wall of independent cards. A major surface is a
   quiet elevation step above the canvas — its silhouette comes from fill and
   shadow, never from a thick or dominant outline — and controls inside a group
   stay flat: no card-on-card nesting.
4. **Contextual explanation, contextual action.** An explanation appears where it
   is needed (a row subtitle only when the state is not visible at a glance, a
   group footer only when the group needs one). A maintenance or diagnostic
   action likewise appears only when the current state makes it the relevant next
   step — a missing Java runtime offers *Install*, a healthy one offers nothing —
   never as a permanent utility. Restraint is a rule, not an afterthought.
5. **Consistency.** Spacing, radii, type roles, status colors, and interaction states
   come from the tokens in this document. New screens use tokens; they do not add
   one-off values.
6. **Restraint in effects.** Shadows, translucency, and animation are budgeted (see
   H and J). Aurora's identity comes from the icon, typography, spacing, and
   interaction quality — not from covering the UI in aurora gradients.

## B. Relationship to Aurora Client

**Shared with the mod's design language** (transferable principles):

- strong title / secondary / muted-text relationships (the mod's
  `ON_BACKGROUND` / `ON_BACKGROUND_SECONDARY` / `ON_BACKGROUND_MUTED` tiers);
- grouped information: rounded groups, hairline-separated rows, muted section
  headers with more space above than below, quiet group footers;
- tighter spacing within groups, generous spacing between groups;
- consistent alignment and a consistent content indent;
- row subtitles only where state is not visible at a glance;
- generous interaction targets;
- restrained, purposeful motion on hover/press/selection;
- status semantics (positive/neutral/warning/negative) mapped from real state;
- semantic tokens instead of scattered literals;
- selective depth: major surfaces may be translucent; interactive rows inside an
  already-elevated container stay flat.

**Adapted for desktop**: a real type scale (the mod has exactly one GUI font size; a
desktop app needs page-title/section/body/metadata roles); mouse-and-keyboard
interaction with visible `:focus-visible` states; window-resilient layout instead of a
fixed GUI scale; OS-native system font stack instead of Minecraft's font; scrolling
content region with a stable navigation sidebar.

**Not copied literally**: Minecraft GUI widgets (toggles, pose-scaled glyphs, GUI-pixel
units), the mod's glass-over-the-world rendering pipeline, hover-dwell tooltips, and
any in-game screen layout. The launcher must feel like the same product family, not
like a scaled-up Minecraft settings screen.

## C. Pandora influence

Pandora (Moulberry's open-source launcher) informs **workflow and structure only**:

- a persistent left sidebar: brand mark, a short stack of destinations, and account
  identity pinned at the bottom — always reachable, never buried;
- page title at the top of the content region with the contextual primary action at
  the top-right of the relevant content (not a global toolbar);
- instance-oriented workflow: the selected instance and its launch path stay one
  click away; advanced operations (logs, output, kill) are contextual to a running
  instance rather than permanent navigation;
- master–detail browsing with tabs inside a detail view;
- practical use of desktop space: medium information density, comfortable rows.

**Non-cloning rule**: Pandora's branding, icons, styling, exact layouts, and code are
never copied. Where a Pandora pattern does not fit Aurora's state or principles, the
Aurora Client design language wins. The synthesis is *Pandora's information
architecture + Aurora's hierarchy, spacing, identity, and restraint*.

## D. Application structure

```text
┌──────────┬──────────────────────────────────────┐
│ brand    │  page title            [contextual   │
│ ──────   │                          action]     │
│ Home     │                                      │
│ Instances│   page content (scrolls vertically)  │
│ Accounts │                                      │
│ About    │                                      │
│ ──────   │                                      │
│ account  │                                      │
└──────────┴──────────────────────────────────────┘
```

- **Shell**: one persistent left sidebar (fixed width, ≈204 px — compact enough that
  the content region keeps visual priority, wide enough that the wordmark, full nav
  labels, selected/focus states, and account identity never feel cramped) and one
  content region (the page). The shell never scrolls; only page content scrolls.
- **Navigation** reflects *actual current capabilities only*: Home, Instances,
  Accounts, About (real backend state), plus a Developer page in development builds.
  Future destinations (Library, Settings) are documented in this file, not built.
- **Account identity/access**: a compact account chip pinned at the bottom of the
  sidebar showing the selected Minecraft name and sign-in state; it navigates to
  Accounts. With no account it offers sign-in.
- **Primary action placement**: the primary action of a page sits at the top-right of
  the content it acts on (Play on Home), with obvious hierarchy but modest size —
  never an oversized promotional object.
- Navigation items are buttons with `aria-current="page"`, visible keyboard focus,
  and a clear selected state. The current page never disappears behind mode switches.

## E. Spacing system

Base unit 4 px; a deliberately small scale (use the nearest token, never a one-off):

| Token | Value | Typical use |
| --- | --- | --- |
| `--space-1` | 4 px | inside badges, icon gaps |
| `--space-2` | 8 px | tight label/detail stacks |
| `--space-3` | 12 px | row padding (vertical), related controls |
| `--space-4` | 16 px | row padding (horizontal), intra-group gaps |
| `--space-5` | 24 px | between groups |
| `--space-6` | 32 px | page top/bottom padding |
| `--space-7` | 48 px | reserved major separation |

Rhythm: tighter inside a group, generous between groups (the mod's rule). One
consistent content indent per page. Rows are ≥ 40 px effective height — generous
targets.

## F. Typography hierarchy

System font stack (`system-ui`, Segoe UI on Windows) at the OS's rendering — a
native-desktop feel, no bundled webfont. Roles (size/weight/color), avoiding
all-caps except the single small brand label where needed:

| Role | Size/weight | Color |
| --- | --- | --- |
| Product/window identity (sidebar wordmark) | 1rem/600 | primary |
| Page title | 1.375rem/600, once per page | primary |
| Section title | 0.95rem/600 | primary |
| Body | 0.9rem/400 | primary |
| Secondary (descriptions, row subtitles) | 0.85rem/400 | secondary |
| Metadata (versions, ids, sizes) | 0.8rem/400 | secondary |
| Label (terse field/status labels) | 0.75rem/500 | muted |

Line height ~1.5 for body text. Version strings and identifiers use the metadata role
in running text (not pills).

Two hard rules learned from the pilot review:

- **No decorative uppercase "eyebrow" headings.** A small violet uppercase category
  label stacked above a title (`PERSISTENT INSTANCES` over *Instances*) reads as a
  themed web dashboard and an AI-generated aesthetic. Hierarchy is page title →
  section title → body/metadata — never decorative-category → big-title → subtitle →
  card-title stacks. The single tolerated all-caps exception is the small brand
  label where genuinely needed.
- **Opaque technical identifiers stay out of primary product UI.** UUIDs, internal
  instance/account ids, digests, hashes, and cache paths belong in details,
  advanced, or developer contexts, and only when the user's immediate task needs
  them. Primary surfaces show names, versions, and states.

## G. Color / surface hierarchy

Semantic roles only (defined once as CSS custom properties in `src/app.css`); tokens
are the contract, components never hard-code hex values:

| Role | Token family | Value (dark theme) |
| --- | --- | --- |
| Application background (content canvas) | `--color-background` | neutral dark ink `#101116` |
| Sidebar | `--color-sidebar` | near-black `#0a0b0e` |
| Primary surface (groups) | `--color-surface` | `#16171d` — one quiet step above the canvas |
| Elevated/interactive surface | `--color-surface-raised` | `#1d1f27` |
| Sunken surface (inputs) | `--color-surface-sunken` | `#0d0e13` |
| Border/divider | `--color-border`, `--color-border-strong` | hairlines `#24262f` / `#343743` |
| Surface edge (elevation outline) | `--color-surface-edge` | barely-visible `rgba(255,255,255,0.05)` |
| Primary text | `--color-text` | `#edf1fb` |
| Secondary text | `--color-text-secondary` | `#a7b0c5` |
| Muted text | `--color-text-muted` | `#76819a` |
| Aurora accent (selection, focus, primary action) | `--color-accent` | aurora violet `#8b80ff` (strong `#6f5df2`, soft tint for selected backgrounds) |
| Success / ready | `--color-success` | mint `#7ee2b8` |
| Warning / needs attention | `--color-warning` | amber `#ecc36b` |
| Error / failed | `--color-error` | rose `#ff9a9a` |
| Working / in progress | `--color-working` | secondary accent `#8fb7ff` |

Rules from the pilot review, binding for every future screen:

- **The canvas and the surface stack are neutral.** A restrained near-black gray
  family — not blue, not violet, not a gradient wash. The sidebar and the content
  canvas distinguish themselves through a subtle depth step (near-black sidebar,
  slightly lighter canvas and groups), never through different hues. Not pure
  black either: layering must remain legible.
- **The accent is rare, and therefore valuable.** Aurora violet communicates
  selection, keyboard focus, primary actions, progress, and identity accents. It
  is never the dominant page background, never a canvas wash, never a decorative
  gradient. If a screen reads as purple, it is wrong.
- Status colors appear as tinted dot + label text on a faint tinted background — never
  as large filled regions. Only status uses badges; metadata never does.

## H. Depth / glass

Preferred depth chain (at most one translucent layer deep):

```text
background → major surface → interactive surface → focus/accent
```

- The application background is flat, neutral ink. It carries no aurora wash — a
  violet-tinted canvas was reviewed and rejected; ambient color belongs to the
  icon and the accent, not to the room the UI sits in.
- Major surfaces (sidebar, grouped panels) are quiet elevation steps above the
  canvas: an opaque fill one step lighter, one soft low-alpha shadow, and at most
  a barely-visible edge (`--color-surface-edge`). A border must never dominate a
  major surface's silhouette — if the outline is what makes a panel read as a
  panel, the design has failed. Avoid the web-dashboard pattern of
  page → giant outlined card → section → nested controls.
- Interactive surfaces (buttons, rows, nav items) are flat fills; **no nested
  translucency and no card-on-card elevation**: never glass panels containing
  glass cards containing glass buttons, and never an outlined card inside an
  outlined card (the mod's glass-budget principle: chrome may be glass; small
  elements inside stay flat).
- Focus is expressed by the accent (outline/ring), not by adding shadow layers.

## I. Shape language

- Radii tokens: `--radius-sm` 6 px (inputs, small buttons), `--radius-md` 10 px
  (buttons, nav items), `--radius-lg` 14 px (grouped panels). No mixed radii within a
  screen; pills are reserved for status badges.
- Borders: 1 px hairlines; borders separate, they do not decorate.
- Controls: quiet rectangles; the primary button is the only accent-filled control on
  a page.
- Selected navigation: a filled interactive-surface pill spanning the nav column,
  full-strength text, plus an accent edge marker on the leading side.
- Cards/rows: groups are panels; rows inside are hairline-separated with no individual
  borders or shadows; the last row has no separator.

## J. Motion

- Durations: `--motion-fast` 90 ms (hover, press), `--motion-base` 150 ms
  (selection, page state). Easing: ease-out. No spring physics, no long sequences.
- Hover: background/border color shift only. Press: one shade darker, no movement.
- Selection (navigation): background fade-in at base duration.
- Page transitions: none (instant swap keeps the shell stable; a fade adds nothing a
  native launcher needs).
- Progress: indeterminate spinners are small and static-colored; progress text is
  plain (phase + counts), driven only by native events.
- `prefers-reduced-motion: reduce` disables all non-essential animation.
- Motion communicates interaction and state change only; it is never decoration.

## K. Interaction states

Every reusable interactive component defines all applicable states:

- **default** — quiet fill, secondary text;
- **hover** — raised fill or border shift, `--motion-fast`;
- **active/pressed** — one shade darker;
- **focus-visible** — 2 px accent outline with 2 px offset; never removed for
  aesthetics; keyboard navigation must reach every control;
- **disabled** — reduced opacity (~50%), `cursor: progress` for in-flight operations,
  no hover effects;
- **selected** (navigation, instances, accounts) — filled selected background,
  full-strength text, accent edge marker where the component type warrants it;
- **destructive** (remove/sign-out) — visually secondary and unmistakably not the
  primary action: a quiet outline button with error-tinted text, never accent-filled
  and never the largest control on the screen.

## L. Status semantics

One consistent mapping from real backend state to presentation:

| State | Color | Presentation |
| --- | --- | --- |
| Ready / verified | success | dot + "Ready" |
| Installing / downloading / resolving | working | dot + phase text from native progress |
| Launching | working | dot + "Starting…" |
| Running | success | dot + "Running" |
| Needs attention (blocked, sign-in required, Java missing) | warning | dot + actionable message |
| Failed / damaged / error | error | dot + message; error details keep the structured code |
| Unavailable / not checked / none | muted | dot + neutral text |

Readiness presented on Home is Rust's `PlayReadiness` decision verbatim; the frontend
never reconstructs readiness from unrelated fields.

## M. Empty / loading / error states

These are intentional product states, not raw backend text:

- **Loading**: a small spinner plus one plain sentence ("Requesting status from the
  native launcher core…") in a reserved area — no layout jump.
- **Empty**: state the situation **once**, in one sentence, and offer the single
  next action as a primary button that navigates to where the work happens
  (Home with no instances: "No instances yet" / "Create an isolated Minecraft
  installation to get started." / **Create instance** → Instances). The same
  condition is never repeated as a second status line beneath the explanation,
  the state is never boxed in a card, and the launcher never performs the action
  automatically. Empty states are honest, never illustrated mascots or filler.
- **Error**: the backend's user-readable message plus its structured code in subdued
  monospace; errors are `role="alert"` and live in the group where the action
  happened. Malformed persisted state is reported, never hidden or auto-repaired.

## N. Window / responsive behavior

- Intended normal window: the Tauri default **920×640**; design target for Home.
- Minimum supported: **720×520** (the configured window minimum). Everything remains
  usable: sidebar keeps its width (content ≥ ~516 px), groups collapse to a single
  column, the page header wraps the primary action below the title when needed.
- Only the content region scrolls — vertically, with the standard overlay scrollbar;
  the sidebar and page title area stay put. **No horizontal scrolling at any
  supported size.**
- Navigation never collapses to icons-only at supported sizes; the sidebar is stable.
- Do not assume fullscreen or very large displays; content columns cap at ~760 px and
  align to the start (a tool window, not a stretched web page).

## O. Icon / branding rule

- The canonical Aurora icon is `static/aurora-icon.png` — a byte-for-byte copy of the
  developer-created artwork (`Aurora Icon.png`, 3162×3162 PNG, monochrome white
  flame-in-ring mark on full transparency, SHA-256
  `faba1af24b964cfd085505c63e9e43d7abedebd70d70794e841a3251d0fa5a19`).
- It must not be regenerated, redrawn, recolored, cropped, given a background, or
  otherwise visually modified without explicit developer direction. It renders
  directly on dark surfaces (its transparency is part of the design).
- The application shell uses it at small brand sizes (sidebar) at 1:1 geometry —
  scaled only, never restyled.
- **The canonical icon is the product mark.** All platform packaging derivatives
  (`src-tauri/icons/` — the window/taskbar `.ico`, `.icns`, and PNG sizes) and the
  SPA favicon (`static/favicon.png`) are *mechanical* derivatives of the canonical
  artwork: exact geometry, scaled only (currently generated with
  `tauri icon static/aurora-icon.png`). Regenerating them must remain a mechanical
  transform — never a redraw, recolor, or reinterpretation.
- **Known, deliberate trade-off**: the monochrome white mark is
  transparent-background by design, so on light OS surfaces (light title bars,
  light taskbars) its visibility is imperfect. Do not "fix" this by recoloring the
  mark, adding a background to the canonical source, or inventing a new logo
  treatment; the resolution is an explicit developer decision that remains open.
- Identity is otherwise typographic: the "Aurora" wordmark, hierarchy, and the
  restrained accent — not aurora gradients across the UI.

## Future information architecture (documented, not built)

- **Library** — a content/mod browsing destination. No backend capability exists
  (no mod management, no content sources); it must not be a fake screen.
- **Settings** — launcher-level settings. No settings backend exists yet; when real
  preferences are implemented they belong here, and About remains the status/version
  surface.
- Advanced instance operations (logs viewing, repair, runtime details, folders)
  arrive contextual to instances, not as top-level navigation.
