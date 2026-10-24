//! Localization of the CLI text (Fluent, embedded in the binary).
//!
//! English is the fallback and the single source of the user-facing wording: see
//! `i18n/en/ezd-fake-client.ftl`. `tracing` logs, the `diag` JSON and protocol strings are not localized.

use clap::{Arg, ArgAction, Command};
use i18n_embed::DesktopLanguageRequester;
use i18n_embed::fluent::{FluentLanguageLoader, fluent_language_loader};
use i18n_embed::unic_langid::LanguageIdentifier;
use rust_embed::RustEmbed;
use std::sync::LazyLock;

#[derive(RustEmbed)]
#[folder = "i18n"]
pub struct Assets;

/// Loader for the process language; `init` selects it from the desktop environment.
pub static LANGUAGE_LOADER: LazyLock<FluentLanguageLoader> = LazyLock::new(|| loader_for(&[]));

/// Builds a loader with the fallback language plus the best match of `requested`.
pub fn loader_for(requested: &[LanguageIdentifier]) -> FluentLanguageLoader {
    let loader = fluent_language_loader!();
    // The fallback language is always loaded underneath the requested ones.
    i18n_embed::select(&loader, &Assets, requested).expect("embedded translations are valid");
    // No invisible FSI/PSI marks around substituted values in terminal output. Set after loading:
    // the setting applies to the bundles that exist.
    loader.set_use_isolating(false);
    loader
}

/// Selects the language from the environment (`LC_ALL`, `LANG`, ...). Call first thing in `main`.
pub fn init() {
    let requested = DesktopLanguageRequester::requested_languages();
    let loader = &*LANGUAGE_LOADER;
    // Errors can only mean broken embedded assets, caught by the tests; stay on the fallback.
    let _ = i18n_embed::select(loader, &Assets, &requested);
    loader.set_use_isolating(false);
}

#[macro_export]
macro_rules! fl {
    ($id:literal) => {
        i18n_embed_fl::fl!($crate::i18n::LANGUAGE_LOADER, $id)
    };
    ($id:literal, $($args:tt)*) => {
        i18n_embed_fl::fl!($crate::i18n::LANGUAGE_LOADER, $id, $($args)*)
    };
}

/// Translates the parts of the command that clap generates itself: the `Usage:` line, section
/// headings and the help and version flags, recursively for every subcommand. clap's own parse
/// error messages and its `[default: ..]` / `[possible values: ..]` annotations stay in English
/// (accepted limitation). The auto-generated `help` subcommand cannot be translated, so it is
/// replaced by the (translated) `--help` flag of every command.
///
/// Call this on the unbuilt command: `mut_arg` on a built command scrambles the arguments.
pub fn localize_chrome(cmd: Command, l: &FluentLanguageLoader) -> Command {
    use i18n_embed_fl::fl as t;
    let template = format!(
        "{{about-with-newline}}\n{} {{usage}}\n\n{{all-args}}{{after-help}}",
        t!(l, "usage-label")
    );
    let options = t!(l, "heading-options");
    let mut cmd = cmd
        .disable_help_flag(true)
        .disable_help_subcommand(true)
        .help_template(template)
        .subcommand_help_heading(t!(l, "heading-commands"))
        .arg(
            Arg::new("help")
                .short('h')
                .long("help")
                .action(ArgAction::Help)
                .help(t!(l, "help-flag"))
                .help_heading(options.clone()),
        );
    let ids: Vec<_> = cmd.get_arguments().map(|a| a.get_id().clone()).collect();
    for id in ids {
        if id != "help" {
            cmd = cmd.mut_arg(id, |a| a.help_heading(options.clone()));
        }
    }
    let names: Vec<String> = cmd
        .get_subcommands()
        .map(|s| s.get_name().to_string())
        .collect();
    for n in names {
        cmd = cmd.mut_subcommand(n, |s| localize_chrome(s, l));
    }
    cmd
}

/// Adds the translated `-V, --version` flag; only the root command has one.
pub fn localize_version(cmd: Command, l: &FluentLanguageLoader) -> Command {
    use i18n_embed_fl::fl as t;
    cmd.disable_version_flag(true).arg(
        Arg::new("version")
            .short('V')
            .long("version")
            .action(ArgAction::Version)
            .help(t!(l, "version-flag"))
            .help_heading(t!(l, "heading-options")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    /// Message ids (with their attribute names) of one language, read from its embedded files.
    fn catalog(lang: &str) -> BTreeMap<String, BTreeSet<String>> {
        let l = loader_for(&[lang.parse().unwrap()]);
        let id: LanguageIdentifier = lang.parse().unwrap();
        l.with_message_iter(&id, |msgs| {
            msgs.map(|m| {
                let attrs = m.attributes.iter().map(|a| a.id.name.to_string());
                (m.id.name.to_string(), attrs.collect())
            })
            .collect()
        })
    }

    #[test]
    fn language_set_is_exact() {
        let langs: BTreeSet<String> = Assets::iter()
            .filter_map(|p| p.split('/').next().map(str::to_string))
            .collect();
        let want = ["en", "es", "fr", "pt", "it", "de"].map(String::from);
        assert_eq!(langs, BTreeSet::from(want));
    }

    #[test]
    fn every_locale_has_the_same_messages_as_en() {
        let en = catalog("en");
        assert!(!en.is_empty());
        for lang in ["es", "fr", "pt", "it", "de"] {
            assert_eq!(catalog(lang), en, "{lang} differs from en");
        }
    }
}
