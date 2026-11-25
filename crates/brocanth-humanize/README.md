# brocanth-humanize

Readable values for a page, in the request's language:

```rust
use brocanth_humanize::Humanize;

let h = Humanize::for_tag("en");
assert_eq!(h.ago(180), "3 minutes ago");
assert_eq!(h.ago(-7200), "in 2 hours");
assert_eq!(h.file_size(1_500_000), "1.5 MB");
assert_eq!(h.ordinal(22), "22nd");
assert_eq!(h.number(1234567), "1,234,567");

let fr = Humanize::for_tag("fr-CA");
assert_eq!(fr.ago(180), "il y a 3 minutes");
assert_eq!(fr.ordinal(1), "1er");
```

English, French, German and Spanish have their own phrases. Any other tag
falls back to English. With the `i18n` feature,
`Humanize::with_formats(locale.formats())` groups numbers by CLDR, as the
`i18n` catalogs do.

A generated project reaches it with `crate::plugins::humanize::of(ctx)`
(`docs/modules/humanize.md`).
