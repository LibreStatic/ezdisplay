# Android app (agent notes)

Repository-wide rules (languages and localization, naming) are in the root `AGENTS.md`.

## Layout
- Package `com.librestatic.ezdisplay`, module `app`, flavors `full` and `play`. The Rust library is built by the
  cargo-ndk task as part of assemble (slow the first time).
- `MainActivity` is Views (the streaming surface). `InputPrefs` is the contract with the native input engine:
  keep `toJson` output stable.
- `settings/` is Jetpack Compose with Material 3 Expressive (material3 1.5.0-beta02) and Material You
  (dynamic colour on API 31+, seed schemes otherwise). Changes are saved at once; there is no Save or Cancel.
  - `SettingsStore` (prefs plus state), `SettingsActions` (what the UI may change), pure rules next to it.
  - `SettingsScreen.kt`: `SettingsContent(state, actions)` is stateless and picks a layout from the available width
    (`BoxWithConstraints` `maxWidth`): Phone (< 600 dp, single column, key editor as a second screen), ListDetail
    (600-999 dp, section list plus detail), Wide (>= 1000 dp, one page plus a 480 dp key-editor pane that exists only
    while a key editor is visible). There is no navigation rail. `BoxWithConstraints` is used instead of
    `WindowSizeClass` because the size classes put 841 and 1056 dp in the same 840 dp bucket. The Wide page measures
    its own width: >= 900 dp gives three profile columns and zoom beside touch, otherwise two columns and stacked.
    At 1691x978 dp the Wide page must not scroll (covered by `SettingsWideTest`).
  - `Sections.kt`: shared components. Colours and type come from `MaterialTheme` only.
  - `DisplaySection.kt`: the Display section (screensaver switch, idle / brightness / blank-after / shape speed sliders,
    movement mode (cross the screen or bounce), preview, replay of the getting-started guide). The Wide page must stay
    free of scrolling, so it has no Display block: a `DisplaySheetButton` on the title row ("Screensaver · 5 minutes"
    or "Screensaver off") opens the full `DisplayContent` in a `ModalBottomSheet` (`DisplaySheet`).
- `onboarding/` is the getting-started guide (`OnboardingActivity`, `OnboardingContent(state, actions, onFinish)` with the
  steps Welcome, HostInstall, Connection, PenProfile, Display, Done). `OnboardingStore` keeps the completed version
  (`onboarding_version` in the `input` prefs, `ONBOARDING_VERSION`); an install that already has other prefs counts as
  completed. `OnboardingStore.shouldShow(context)` is what `MainActivity` asks before starting. Settings replays the
  guide with the `replay` extra. The Connection step reuses `ConnectionContent`; the pen step writes through
  `SettingsActions`. `OnboardingScreenshotTest` captures every step at phone and tablet widths.
- `ui/expressive/` is the shared Material 3 Expressive kit: `MorphShape`, `PageTransitions` (shared-axis X, crossfade
  with reduced motion), `StepShape` (morphing step indicator), `ShapeBackdrop` (drifting shape field of the
  onboarding guide) and `Motion` (reduced motion = animator duration scale 0).
- `display/PanelType.kt` detects OLED / LCD (sysfs, system properties, model list, HDR min luminance) and caches it in
  prefs; `screensaver/` holds `ScreensaverContent` (its own dim monochrome field of themed silhouettes from
  `ScreensaverGlyphs`: monitor, tablet, stylus, brush, palette, mouse; not `ShapeBackdrop`), `ScreensaverMotion` (pure
  Kotlin motion model: Drift or Bounce, speed level 1..10 at 10 dp/s each, rare steps with reduced motion) and
  `ScreensaverPrefs`. The screensaver prefs live in the `input` file but are never part of `InputPrefs.toJson`.

## Build and test
- Set `ANDROID_HOME` when `local.properties` is missing.
- Run Gradle from this directory and redirect output to a log, then grep or tail it:
  `./gradlew assembleFullDebug assemblePlayDebug lintFullDebug testFullDebugUnitTest > build.log 2>&1`
- Tests are JUnit4 plus Robolectric (`sdk=36`, set in `robolectric.properties`) and compose-ui-test. Layout tests
  choose the size with `@Config(qualifiers = "w412dp-h915dp")`. Robolectric needs JDK 21 or newer for SDK 36
  (CI uses 21); older JDKs fail every test class with `UnsupportedOperationException` in `DefaultSdkProvider`.

## Rendering the UI
- `./gradlew :app:recordRoborazziFullDebug` writes PNGs to `app/build/outputs/roborazzi/`. No reference images are
  stored and CI does not compare; plain `testFullDebugUnitTest` only renders.
- To add a capture, add a test to `SettingsScreenshotTest`: pick `qualifiers` (size, `night` or `notnight`, an optional
  `280dpi`), wrap in `SettingsTheme(dynamicColor = false)` so colours are the deterministic seed scheme, and call
  `captureRoboImage("build/outputs/roborazzi/<name>.png")`.
- Each capture also prints its semantics tree (`printToString`) to the test's stdout (in
  `app/build/test-results/testFullDebugUnitTest/*.xml`). Read the tree first: hierarchy, text, selected state, tags.
- Loop: edit, check the semantics tree, and take a screenshot only for genuinely visual questions (spacing, clipping,
  colour). Use a real device for insets, the S Pen and Samsung's own settings.

## Icons
Material Symbols Rounded vector drawables from `@material-symbols/svg-400` (viewport 960 with translateY 960), named
`ic_<symbol>`. Do not add a material-icons dependency.
