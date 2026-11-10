//! Translation of the texts that the Rust code shows.
//!
//! The texts are English and translated through Qt, like the QML texts.
//! `scripts/update-translations.sh` collects them with lupdate, which only
//! finds literal calls of the form `translate("Klickmeister", "Text")`.

use std::fmt::Display;

#[cxx::bridge]
mod ffi {
    unsafe extern "C++" {
        include!("klickmeister/src/i18n.h");
        include!("cxx-qt-lib/qstring.h");

        type QString = cxx_qt_lib::QString;

        #[rust_name = "install_translator"]
        fn klickmeisterInstallTranslator();

        #[rust_name = "translate_qt"]
        fn klickmeisterTranslate(context: &str, source: &str) -> QString;
    }
}

/// Load the translation for the user's language. Without one, the English
/// texts stay. Call on the Qt thread once the application exists.
pub fn install_translator() {
    ffi::install_translator();
}

/// Translate `source` in `context`. Without a loaded translation, for example
/// in tests, this returns `source`.
pub fn translate(context: &str, source: &str) -> String {
    ffi::translate_qt(context, source).to_string()
}

/// Replace the placeholders `%1` to `%9` in a translated text, in one pass,
/// so that an argument containing `%2` stays as it is.
pub fn fill(text: &str, args: &[&dyn Display]) -> String {
    let mut filled = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        filled.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let argument = after
            .chars()
            .next()
            .and_then(|digit| digit.to_digit(10))
            .and_then(|digit| args.get(usize::try_from(digit).ok()?.checked_sub(1)?));
        match argument {
            Some(argument) => {
                filled.push_str(&argument.to_string());
                rest = &after[1..];
            }
            None => {
                filled.push('%');
                rest = after;
            }
        }
    }
    filled.push_str(rest);
    filled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_replaces_each_placeholder_once() {
        assert_eq!(fill("%1 · %2 · %1", &[&"a", &3]), "a · 3 · a");
        assert_eq!(fill("Details: %1", &[&"50%2 done"]), "Details: 50%2 done");
        assert_eq!(fill("100% %3 %", &[&"a"]), "100% %3 %");
    }

    #[test]
    fn translate_without_translation_keeps_the_source() {
        assert_eq!(translate("Klickmeister", "Ready"), "Ready");
    }
}
