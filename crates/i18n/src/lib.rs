//! `city-i18n` : tout texte montré au joueur passe par ici.
//!
//! # Fichiers de langue
//! Une langue = un dossier `locales/<code>/` (code BCP 47 : `en`, `fr`,
//! `pt-BR`…) contenant des fichiers [Fluent](https://projectfluent.org)
//! (`*.ftl`), découpés par domaine : `common.ftl`, `simulation.ftl`,
//! `errors.ftl`, `interface.ftl`. Fluent gère les pluriels, les nombres et
//! l'ordre des mots propre à chaque langue : le code ne concatène jamais de
//! phrases.
//!
//! # Anglais de référence
//! L'anglais est **embarqué** dans le binaire et sert de repli : une clé
//! absente d'une traduction s'affiche en anglais plutôt qu'en clé brute. Les
//! tests vérifient que chaque clé utilisée par le jeu existe en anglais
//! (`city-ui`), et qu'une traduction ne contient aucune clé inconnue de
//! l'anglais (`orphan_keys`).
//!
//! # Règles
//! - Aucun texte destiné au joueur en dur dans le code : la simulation expose
//!   des *clés* (`ZoneType::message_key`…), l'interface les traduit.
//! - Les clés sont en anglais, en minuscules avec tirets, préfixées par leur
//!   domaine (`zone-`, `overlay-`, `error-`, `hud-`…).

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

/// Langue de référence, embarquée.
pub const REFERENCE_LANGUAGE: &str = "en";

/// Fichiers anglais embarqués (nom, contenu).
pub const ENGLISH_FILES: [(&str, &str); 5] = [
    ("common.ftl", include_str!("../../../locales/en/common.ftl")),
    ("simulation.ftl", include_str!("../../../locales/en/simulation.ftl")),
    ("errors.ftl", include_str!("../../../locales/en/errors.ftl")),
    ("interface.ftl", include_str!("../../../locales/en/interface.ftl")),
    ("roads.ftl", include_str!("../../../locales/en/roads.ftl")),
];

/// Argument d'un message (`{ $name }` dans le fichier Fluent).
#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    Text(String),
    Number(f64),
}

impl From<&str> for Arg {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<String> for Arg {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

macro_rules! number_arg {
    ($($t:ty),*) => {$(
        impl From<$t> for Arg {
            fn from(value: $t) -> Self {
                Self::Number(value as f64)
            }
        }
    )*};
}
number_arg!(i8, i16, i32, i64, u8, u16, u32, u64, usize, f32, f64);

/// Erreur de chargement d'une langue.
#[derive(Debug)]
pub enum I18nError {
    /// Code de langue invalide.
    BadLanguage(String),
    /// Dossier ou fichier illisible.
    Io(PathBuf, std::io::Error),
    /// Erreur de syntaxe Fluent (fichier, message).
    Syntax(String, String),
    /// Clé définie deux fois.
    Duplicate(String, String),
}

impl fmt::Display for I18nError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadLanguage(code) => write!(f, "invalid language code: {code}"),
            Self::Io(path, e) => write!(f, "cannot read {}: {e}", path.display()),
            Self::Syntax(file, e) => write!(f, "syntax error in {file}: {e}"),
            Self::Duplicate(file, e) => write!(f, "duplicate message in {file}: {e}"),
        }
    }
}

impl std::error::Error for I18nError {}

type Bundle = FluentBundle<FluentResource>;

/// Une langue chargée, avec l'anglais en repli.
pub struct Localizer {
    language: String,
    /// Langue demandée puis anglais (absent si la langue demandée est l'anglais).
    bundles: Vec<Bundle>,
    keys: BTreeSet<String>,
}

impl fmt::Debug for Localizer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Localizer")
            .field("language", &self.language)
            .field("keys", &self.keys.len())
            .finish_non_exhaustive()
    }
}

impl Localizer {
    /// Anglais embarqué. Ne peut pas échouer : les tests valident les fichiers.
    #[must_use]
    pub fn english() -> Self {
        let (bundle, keys) = build_bundle(REFERENCE_LANGUAGE, ENGLISH_FILES.iter().map(|(n, s)| ((*n).to_owned(), (*s).to_owned())))
            .expect("embedded English files are valid (checked by tests)");
        Self { language: REFERENCE_LANGUAGE.to_owned(), bundles: vec![bundle], keys }
    }

    /// Charge `dir/<language>/*.ftl`, avec l'anglais embarqué en repli.
    ///
    /// # Errors
    /// Code de langue invalide, dossier illisible, fichier Fluent invalide.
    pub fn from_dir(dir: &Path, language: &str) -> Result<Self, I18nError> {
        if language == REFERENCE_LANGUAGE {
            return Ok(Self::english());
        }
        let folder = dir.join(language);
        let mut files = Vec::new();
        let entries = std::fs::read_dir(&folder).map_err(|e| I18nError::Io(folder.clone(), e))?;
        for entry in entries {
            let path = entry.map_err(|e| I18nError::Io(folder.clone(), e))?.path();
            if path.extension().is_some_and(|e| e == "ftl") {
                let text = std::fs::read_to_string(&path).map_err(|e| I18nError::Io(path.clone(), e))?;
                files.push((path.file_name().unwrap_or_default().to_string_lossy().into_owned(), text));
            }
        }
        files.sort();
        let (bundle, keys) = build_bundle(language, files)?;
        let english = Self::english();
        let mut bundles = vec![bundle];
        bundles.extend(english.bundles);
        Ok(Self { language: language.to_owned(), bundles, keys })
    }

    /// Code de la langue chargée.
    #[must_use]
    pub fn language(&self) -> &str {
        &self.language
    }

    /// Clés définies par la langue chargée (sans le repli anglais).
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.keys.iter().map(String::as_str)
    }

    /// Vrai si la clé existe (dans la langue ou en repli).
    #[must_use]
    pub fn has(&self, key: &str) -> bool {
        self.bundles.iter().any(|b| b.has_message(key))
    }

    /// Texte d'un message sans argument.
    #[must_use]
    pub fn text(&self, key: &str) -> String {
        self.format(key, &[])
    }

    /// Texte d'un message avec arguments. Une clé inconnue s'affiche entre
    /// crochets (`[key]`) : visible en jeu, mais sans planter.
    #[must_use]
    pub fn format(&self, key: &str, args: &[(&str, Arg)]) -> String {
        let mut fluent_args = FluentArgs::new();
        for (name, value) in args {
            match value {
                Arg::Text(text) => fluent_args.set(*name, FluentValue::from(text.as_str())),
                Arg::Number(n) => fluent_args.set(*name, FluentValue::from(*n)),
            }
        }
        for bundle in &self.bundles {
            let Some(message) = bundle.get_message(key) else { continue };
            let Some(pattern) = message.value() else { continue };
            let mut errors = Vec::new();
            let text = bundle.format_pattern(pattern, Some(&fluent_args), &mut errors);
            return text.into_owned();
        }
        format!("[{key}]")
    }

    /// Clés de `self` absentes de la langue de référence (fautes de frappe
    /// des traducteurs, clés supprimées de l'anglais).
    #[must_use]
    pub fn orphan_keys(&self, reference: &Self) -> Vec<String> {
        self.keys.difference(&reference.keys).cloned().collect()
    }
}

/// Langues disponibles dans `dir` (un sous-dossier par code), anglais compris.
#[must_use]
pub fn available_languages(dir: &Path) -> Vec<String> {
    let mut languages: BTreeSet<String> = BTreeSet::from([REFERENCE_LANGUAGE.to_owned()]);
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                languages.insert(entry.file_name().to_string_lossy().into_owned());
            }
        }
    }
    languages.into_iter().collect()
}

fn build_bundle(language: &str, files: impl IntoIterator<Item = (String, String)>) -> Result<(Bundle, BTreeSet<String>), I18nError> {
    let id: LanguageIdentifier = language.parse().map_err(|_| I18nError::BadLanguage(language.to_owned()))?;
    let mut bundle = Bundle::new_concurrent(vec![id]);
    // Pas de marques d'isolation Unicode autour des arguments : inutiles pour
    // les langues gauche-à-droite, et elles s'affichent comme des carrés
    // dans certaines polices.
    bundle.set_use_isolating(false);
    let mut keys = BTreeSet::new();
    for (name, text) in files {
        let resource = FluentResource::try_new(text).map_err(|(_, errors)| {
            I18nError::Syntax(name.clone(), errors.iter().map(|e| format!("{e:?}")).collect::<Vec<_>>().join("; "))
        })?;
        for entry in resource.entries() {
            if let fluent_syntax::ast::Entry::Message(message) = entry {
                keys.insert(message.id.name.to_owned());
            }
        }
        bundle
            .add_resource(resource)
            .map_err(|errors| I18nError::Duplicate(name.clone(), errors.iter().map(|e| format!("{e:?}")).collect::<Vec<_>>().join("; ")))?;
    }
    Ok((bundle, keys))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_files_parse_and_have_no_duplicates() {
        for (name, text) in ENGLISH_FILES {
            assert!(FluentResource::try_new(text.to_owned()).is_ok(), "{name} invalide");
        }
        let english = Localizer::english();
        assert!(english.keys().count() > 50);
    }

    #[test]
    fn formats_arguments_and_plurals() {
        let en = Localizer::english();
        assert_eq!(en.text("app-title"), "CityMayor");
        assert_eq!(en.format("unit-inhabitants", &[("count", 1.into()), ("formatted", "1".into())]), "1 inhabitant");
        assert_eq!(
            en.format("unit-inhabitants", &[("count", 1250.into()), ("formatted", "1,250".into())]),
            "1,250 inhabitants"
        );
    }

    #[test]
    fn unknown_keys_are_visible_but_harmless() {
        let en = Localizer::english();
        assert_eq!(en.text("no-such-key"), "[no-such-key]");
        assert!(!en.has("no-such-key"));
    }

    #[test]
    fn translations_fall_back_to_english_and_report_orphans() {
        let dir = std::env::temp_dir().join(format!("city-i18n-test-{}", std::process::id()));
        let fr = dir.join("fr");
        std::fs::create_dir_all(&fr).unwrap();
        std::fs::write(fr.join("common.ftl"), "app-title = CityMayor\nunknown-typo = Oups\n").unwrap();
        let loc = Localizer::from_dir(&dir, "fr").unwrap();
        assert_eq!(loc.language(), "fr");
        // Absente du français : repli anglais.
        assert_eq!(loc.text("zone-residential"), Localizer::english().text("zone-residential"));
        assert_eq!(loc.orphan_keys(&Localizer::english()), vec!["unknown-typo".to_owned()]);
        assert!(available_languages(&dir).contains(&"fr".to_owned()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn shipped_translations_only_use_english_keys() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../locales");
        let english = Localizer::english();
        for language in available_languages(&dir) {
            if language == REFERENCE_LANGUAGE {
                continue;
            }
            let loc = Localizer::from_dir(&dir, &language).unwrap();
            assert!(loc.orphan_keys(&english).is_empty(), "{language} : {:?}", loc.orphan_keys(&english));
        }
        // Le dossier anglais sur disque est bien celui qui est embarqué.
        for (name, text) in ENGLISH_FILES {
            assert_eq!(std::fs::read_to_string(dir.join("en").join(name)).unwrap(), text);
        }
    }
}
