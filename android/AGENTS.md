# Android app (agent notes)

## Layout
- Package `io.github.facum.ezdisplay`, module `app`, flavors `full` and `play`. The Rust library is built by the
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

## Naming
Do not name pen-tablet vendors or commercial drawing apps in code, strings, docs, tests or commit messages; describe
the behaviour instead (for example "graphics tablet", "drawing apps"). The platforms the app runs on or talks to
(Android, Samsung and the S Pen, KDE, Windows) and open-source apps used as test examples (Krita) are fine.

## Icons
Material Symbols Rounded vector drawables from `@material-symbols/svg-400` (viewport 960 with translateY 960), named
`ic_<symbol>`. Do not add a material-icons dependency.
