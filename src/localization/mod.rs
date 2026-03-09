use crate::handler::Data;
use anyhow::anyhow;
use fluent::FluentArgs;
use include_dir::{Dir, include_dir};
use std::collections::HashMap;

type Error = anyhow::Error;
type FluentBundle = fluent::bundle::FluentBundle<
    fluent::FluentResource,
    intl_memoizer::concurrent::IntlLangMemoizer,
>;

const TTS_LOCALES: Dir = include_dir!("$CARGO_MANIFEST_DIR/locales/tts");
const DISCORD_LOCALES: Dir = include_dir!("$CARGO_MANIFEST_DIR/locales/discord");

pub fn load_tts_locales(fallback: &str) -> Result<Locales, Error> {
    load_from_static_dir(
        TTS_LOCALES,
        LocaleSearchPolicy::new_cascading(fallback.to_owned(), '-'),
    )
}

pub fn load_discord_locales(fallback: &str) -> Result<Locales, Error> {
    load_from_static_dir(
        DISCORD_LOCALES,
        LocaleSearchPolicy::new_exact(fallback.to_string()),
    )
}

fn load_from_static_dir(dir: Dir, policy: LocaleSearchPolicy) -> Result<Locales, Error> {
    let mut bundles = HashMap::new();

    for file in dir.files() {
        let locale = file
            .path()
            .file_stem()
            .ok_or(anyhow!("Invalid file name: '{}'", file.path().display()))?
            .to_str()
            .ok_or(anyhow!("Invalid unicode filename"))?;

        let resource = fluent::FluentResource::try_new(
            file.contents_utf8()
                .ok_or(anyhow!("Invalid file contents"))?
                .to_owned(),
        )
        .map_err(|(_, e)| anyhow!("failed to parse {:?}: {:?}", file.path(), e))?;

        let mut bundle = FluentBundle::new_concurrent(vec![
            locale
                .parse()
                .map_err(|e| anyhow!("invalid locale `{}`: {}", locale, e))?,
        ]);
        bundle
            .add_resource(resource)
            .map_err(|e| anyhow!("failed to add resource to bundle: {:?}", e))?;

        bundles.insert(locale.to_owned(), bundle);
    }

    Locales::new_with_bundles(policy, bundles)
}

enum LocaleMatchingMode {
    Exact,
    Cascading { delimiter: char },
}

pub struct LocaleSearchPolicy {
    fallback: String,
    mode: LocaleMatchingMode,
}

impl LocaleSearchPolicy {
    fn new_cascading(fallback: String, delimiter: char) -> Self {
        Self {
            fallback,
            mode: LocaleMatchingMode::Cascading { delimiter },
        }
    }

    fn new_exact(fallback: String) -> Self {
        Self {
            fallback,
            mode: LocaleMatchingMode::Exact,
        }
    }

    fn generate_candidates<'a>(&'a self, locale: &'a str) -> impl Iterator<Item = &'a str> {
        let mut candidates = Vec::new();
        candidates.push(locale);

        if let LocaleMatchingMode::Cascading { delimiter } = self.mode
            && let Some((language, _)) = locale.split_once(delimiter)
        {
            candidates.push(language)
        }

        if !candidates.contains(&self.fallback.as_str()) {
            candidates.push(self.fallback.as_str());
        }

        candidates.into_iter()
    }
}

pub struct Locales {
    search_policy: LocaleSearchPolicy,
    bundles: HashMap<String, FluentBundle>,
}

impl Locales {
    pub fn new_with_bundles(
        search_policy: LocaleSearchPolicy,
        bundles: HashMap<String, FluentBundle>,
    ) -> Result<Self, Error> {
        if !bundles.contains_key(&search_policy.fallback) {
            return Err(anyhow!(
                "fallback locale {} not found",
                &search_policy.fallback
            ));
        }

        Ok(Self {
            search_policy,
            bundles,
        })
    }

    /// Resolves a localized message by searching through candidates according to the configured search policy.
    /// When attr exists, resolves the specified attribute of the message instead of the message value itself.
    ///
    /// For a given locale (e.g., "ja-JP") and a defined fallback (e.g., "en"),
    /// the search candidates are based on the search policy (cascading or exact).
    /// The first successfully resolved message is returned.
    pub fn resolve(
        &self,
        locale: &str,
        id: &str,
        attr: Option<&str>,
        args: Option<&FluentArgs>,
    ) -> Result<String, Error> {
        for candidate in self.search_policy.generate_candidates(locale) {
            let bundle = match self.bundles.get(candidate) {
                Some(bundle) => bundle,
                None => continue, // skips if no match
            };

            let message = match bundle.get_message(id) {
                Some(message) => message,
                None => continue, // skips if no match
            };

            let pattern = match attr {
                Some(attribute) => message.get_attribute(attribute).map(|attr| attr.value()),
                None => message.value(),
            };

            let pattern = match pattern {
                Some(pattern) => pattern,
                None => continue, // skips if no match
            };

            let formatted = bundle.format_pattern(pattern, args, &mut vec![]);

            return Ok(formatted.into_owned());
        }

        Err(anyhow!("no fallback found for id '{}'", id))
    }

    // The following code: format, apply are derived from serenity-rs/poise.
    // https://github.com/serenity-rs/poise/blob/518ff0564865bca2abf01ae8995b77340f439ef9/examples/fluent_localization/translation.rs
    //
    // Copyright (c) 2022 kangalioo
    // Licensed under the MIT License.
    // https://github.com/serenity-rs/poise/blob/main/LICENSE
    fn format(
        bundle: &FluentBundle,
        id: &str,
        attr: Option<&str>,
        args: Option<&FluentArgs<'_>>,
    ) -> Option<String> {
        let message = bundle.get_message(id)?;
        let pattern = match attr {
            Some(attribute) => message.get_attribute(attribute)?.value(),
            None => message.value()?,
        };
        let formatted = bundle.format_pattern(pattern, args, &mut vec![]);
        Some(formatted.into_owned())
    }

    pub fn apply(&self, commands: &mut [poise::Command<Data, Error>]) -> Result<(), Error> {
        for command in &mut *commands {
            // recursively apply
            self.apply(&mut command.subcommands)?;

            // real-apply
            for (locale, bundle) in &self.bundles {
                // Insert localized command name and description
                let localized_command_name =
                    match Self::format(bundle, &command.identifying_name, None, None) {
                        Some(x) => x,
                        None => continue, // no localization entry => skip localization
                    };

                command
                    .name_localizations
                    .insert(locale.clone(), localized_command_name);
                command.description_localizations.insert(
                    locale.clone(),
                    Self::format(bundle, &command.identifying_name, Some("description"), None)
                        .ok_or(anyhow!(
                            "failed to format command description {}",
                            command.identifying_name
                        ))?,
                );

                for parameter in &mut command.parameters {
                    // Insert localized parameter name and description
                    parameter.name_localizations.insert(
                        locale.clone(),
                        Self::format(
                            bundle,
                            &command.identifying_name,
                            Some(&parameter.name),
                            None,
                        )
                        .ok_or(anyhow!(
                            "failed to format parameter {} for command {}",
                            parameter.name,
                            command.identifying_name
                        ))?,
                    );
                    parameter.description_localizations.insert(
                        locale.clone(),
                        Self::format(
                            bundle,
                            &command.identifying_name,
                            Some(&format!("{}-description", parameter.name)),
                            None,
                        )
                        .ok_or(anyhow!(
                            "failed to format parameter description {} for command {}",
                            parameter.name,
                            command.identifying_name
                        ))?,
                    );

                    // If this is a choice parameter, insert its localized variants
                    for choice in &mut parameter.choices {
                        choice.localizations.insert(
                            locale.clone(),
                            Self::format(bundle, &choice.name, None, None).ok_or(anyhow!(
                                "failed to format choice {} for command {}",
                                choice.name,
                                command.identifying_name
                            ))?,
                        );
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{FluentBundle, LocaleMatchingMode, LocaleSearchPolicy, Locales};
    use fluent::FluentResource;
    use std::collections::HashMap;

    fn create_policy(fallback: &str, mode: LocaleMatchingMode) -> LocaleSearchPolicy {
        LocaleSearchPolicy {
            fallback: fallback.to_string(),
            mode,
        }
    }

    #[test]
    fn test_cascading_with_region_match() {
        let policy = create_policy("ja", LocaleMatchingMode::Cascading { delimiter: '-' });
        let candidates: Vec<_> = policy.generate_candidates("en-US").collect();

        assert_eq!(candidates, vec!["en-US", "en", "ja"]);
    }

    #[test]
    fn test_cascading_base_language_only() {
        let policy = create_policy("ja", LocaleMatchingMode::Cascading { delimiter: '-' });
        let candidates: Vec<_> = policy.generate_candidates("en").collect();

        assert_eq!(candidates, vec!["en", "ja"]);
    }

    #[test]
    fn test_cascading_locale_is_ultimate_fallback() {
        let policy = create_policy("en", LocaleMatchingMode::Cascading { delimiter: '-' });
        let candidates: Vec<_> = policy.generate_candidates("en").collect();

        assert_eq!(candidates, vec!["en"]);
    }

    #[test]
    fn test_cascading_needs_fallback_no_prefix() {
        let policy = create_policy("en", LocaleMatchingMode::Cascading { delimiter: '-' });
        let candidates: Vec<_> = policy.generate_candidates("fr").collect();

        assert_eq!(candidates, vec!["fr", "en"]);
    }

    #[test]
    fn test_exact_match_no_prefix_added() {
        let policy = create_policy("en", LocaleMatchingMode::Exact);
        let candidates: Vec<_> = policy.generate_candidates("zh-Hans").collect();

        assert_eq!(candidates, vec!["zh-Hans", "en"]);
    }

    #[test]
    fn test_exact_match_locale_is_ultimate_fallback() {
        let policy = create_policy("ja", LocaleMatchingMode::Exact);
        let candidates: Vec<_> = policy.generate_candidates("ja").collect();

        assert_eq!(candidates, vec!["ja"]);
    }

    struct TestContext {
        locales: Locales,
    }

    impl TestContext {
        fn new(search_policy: LocaleSearchPolicy, source: &str) -> Self {
            let mut bundles = HashMap::new();
            let mut bundle = FluentBundle::new_concurrent(vec![
                search_policy.fallback.parse().expect("must be valid"),
            ]);
            bundle
                .add_resource(FluentResource::try_new(source.to_owned()).expect("must parse"))
                .expect("must add resource");
            bundles.insert(search_policy.fallback.to_owned(), bundle);

            Self {
                locales: Locales {
                    search_policy,
                    bundles,
                },
            }
        }

        fn add_bundle(mut self, locale: &str, source: &str) -> Self {
            let mut bundle =
                FluentBundle::new_concurrent(vec![locale.parse().expect("must be valid")]);
            bundle
                .add_resource(FluentResource::try_new(source.to_owned()).expect("must parse"))
                .expect("must add resource");
            self.locales.bundles.insert(locale.to_owned(), bundle);
            self
        }
    }

    #[test]
    fn resolve_exact_one_first_if_key_existing_on_cascading() {
        let ctx = TestContext::new(
            LocaleSearchPolicy::new_cascading("fallback".to_string(), '-'),
            "key = value-fallback",
        )
        .add_bundle("en", "key = value-en")
        .add_bundle("en-US", "key = value-en-US");

        let result = ctx.locales.resolve("en-US", "key", None, None);

        assert_eq!(result.ok(), Some("value-en-US".into()));
    }

    #[test]
    fn resolve_prefixed_one_second_if_key_existing_on_cascading() {
        let ctx = TestContext::new(
            LocaleSearchPolicy::new_cascading("fallback".to_string(), '-'),
            "key = value-fallback",
        )
        .add_bundle("en", "key = value-en")
        .add_bundle("en-US", "key = value-en-US");

        let result = ctx.locales.resolve("en-GB", "key", None, None);

        assert_eq!(result.ok(), Some("value-en".into()));
    }

    #[test]
    fn resolve_fallback_finally_if_key_existing_on_cascading() {
        let ctx = TestContext::new(
            LocaleSearchPolicy::new_cascading("fallback".to_string(), '-'),
            "key = value-fallback",
        )
        .add_bundle("en", "key = value-en")
        .add_bundle("en-US", "key = value-en-US");

        let result = ctx.locales.resolve("ja", "key", None, None);

        assert_eq!(result.ok(), Some("value-fallback".into()));
    }

    #[test]
    fn fail_to_resolve_if_nonexistent_key_on_cascading() {
        let ctx = TestContext::new(
            LocaleSearchPolicy::new_cascading("fallback".to_string(), '-'),
            "key = value-fallback",
        )
        .add_bundle("en", "key = value-en")
        .add_bundle("en-US", "key = value-en-US");

        let result = ctx.locales.resolve("en-US", "no-key", None, None);

        assert!(result.is_err());
    }

    #[test]
    fn resolve_attr_on_cascading() {
        let ctx = TestContext::new(
            LocaleSearchPolicy::new_cascading("fallback".to_string(), '-'),
            "key = value-fallback",
        )
        .add_bundle(
            "en",
            r#"key = value-en
                                .attr = attr-en
                "#,
        )
        .add_bundle("en-US", "key = value-en-US");

        let result = ctx.locales.resolve("en-US", "key", Some("attr"), None);

        assert_eq!(result.ok(), Some("attr-en".into()));
    }

    #[test]
    fn fail_to_resolve_if_nonexistent_attr_on_cascading() {
        let ctx = TestContext::new(
            LocaleSearchPolicy::new_cascading("fallback".to_string(), '-'),
            "key = value-fallback",
        )
        .add_bundle("en", "key = value-en")
        .add_bundle("en-US", "key = value-en-US");

        let result = ctx.locales.resolve("en-US", "key", Some("no-attr"), None);

        assert!(result.is_err());
    }
}
