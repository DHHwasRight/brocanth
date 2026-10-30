use crate::i18n::{t, Locale};

/// A cart heading. The text is the catalog's; the template only says where
/// it goes, and an `&` or `<` in a translation is escaped like any value.
pub fn Cart(locale: Locale, count: u64, name: &str) -> Element {
    <section lang={locale}>
        <h1>{t::title(locale)}</h1>
        <p>{t::greeting(locale, name)}</p>
        <p class="count">{t::cart_items(locale, count)}</p>
    </section>
}
