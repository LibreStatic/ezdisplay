# Project rules

These rules apply to the whole repository. Component notes live next to the code (`android/AGENTS.md`).

## Language
- Commit messages, PR titles and descriptions, branch names, code identifiers, code comments, test names,
  log messages (`tracing`, `Log.*`) and machine-readable output (JSON diagnostics, protocol strings) are
  in English, whatever language the prompt uses.
- Documentation (`README.md`, `docs/`, `AGENTS.md` files, guides, checklists) is written in one language:
  English. Do not add translated copies of documents or prose in another language.
- User-facing text is written in English first and fully localized into English, Spanish, French,
  Portuguese, Italian and German. A string is not done until every locale has it. Keep one register per
  language across both apps: Spanish with Rioplatense voseo ("Elegí"), Brazilian Portuguese, French with
  "vous", German and Italian with the informal imperative.
  - Android: base strings in `android/app/src/main/res/values/strings.xml`, translations in `values-es`,
    `values-fr`, `values-pt`, `values-it` and `values-de`. Use `plurals` for counts and placeholders
    (`%1$s`) instead of concatenation; mark only non-text values `translatable="false"`.
  - PC apps (`ezd-host`, `ezd-fake-client`): Fluent files in `crates/<crate>/i18n/<lang>/`, with `en` as the
    fallback, read with `fl!()`. Localize `--help`, error messages shown to the user and printed summaries;
    logs and JSON stay in English.
- Adding a language means adding it to every list above, to both apps and to the Android locale config.

## Naming
Do not name pen-tablet vendors or commercial drawing apps in code, strings, docs, tests or commit messages; describe
the behaviour instead (for example "graphics tablet", "drawing apps"). The platforms the app runs on or talks to
(Android, Samsung and the S Pen, KDE, Windows) and open-source apps used as test examples (Krita) are fine.

## Diagrams
- Diagrams in documentation (`README.md`, `docs/`, component READMEs) are Mermaid code blocks (` ```mermaid `),
  like "How it works" in `README.md` and the diagrams in `docs/protocol.md`. Do not draw them in ASCII art (box
  drawing, `|---->|` message ladders, `+---+` boxes); when you edit a section that still has one, convert it.
- Pick the type by content: `sequenceDiagram` for message exchanges and handshakes, `stateDiagram-v2` for state
  machines, `flowchart` for pipelines, components, data paths and decisions. Name real messages, fields and states
  from the code, and update the diagram in the same change as the behaviour it shows.
- GitHub renders with a recent Mermaid (11.8 or later). Quote labels that contain punctuation, do not start a label
  with `1.` or `-` (it becomes a Markdown list), and do not use keywords such as `click`, `end`, `class`, `style` or
  `default` as node or state ids. Render every new or changed diagram (Mermaid CLI or a browser) before committing.
- Not diagrams, so they stay plain code blocks: formulas, URI and file formats, command output. Rust doc comments
  keep ASCII diagrams, because rustdoc does not render Mermaid.

## Versioning
- The version is `version` in `[workspace.package]` of the root `Cargo.toml` (crates, packages and the Android app
  follow it). Every push to `main` publishes a GitHub pre-release `v<version>+<run number>`; a `vX.Y.Z` tag publishes
  the release (see `docs/releasing.md`).
- Bump the version in its own commit (`Bump the version to X.Y.Z`, with `cargo update -w` for `Cargo.lock`) before
  every tagged release; never tag a release under the previous version. Patch (`x.y.Z`) for fixes and small,
  self-contained changes (including a single small feature), minor (`x.Y.0`) for medium changes (several features, a
  reworked screen or flow), major (`X.0.0`) for a large overhaul; keep the pre-release suffix (`-alpha`).
