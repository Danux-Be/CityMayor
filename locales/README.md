# Language files

All player-facing text lives here. The code holds only *keys* (for example `zone-residential`) and never displays raw text.

```
locales/
  en/                 reference language, embedded in the game
    common.ftl        app name, units, calendar, generic actions
    simulation.ftl    zones, buildings, overlays, statistics, events
    errors.ftl        messages shown when an action is refused
    interface.ftl     window, HUD, tools
  <code>/             a translation (BCP 47 code: fr, de, pt-BR…)
```

The files use the [Fluent](https://projectfluent.org/fluent/guide/) format. It handles plurals, numbers and word order for each language:

```ftl
unit-inhabitants = { $count ->
    [one] { $count } inhabitant
   *[other] { $count } inhabitants
}
```

## Adding a language

1. Copy `en/` to `<code>/`.
2. Translate the values only. Leave the keys and the `{ $variables }` untouched.
3. Run `cargo test -p city-i18n`. It rejects any key that doesn't exist in English (a typo, or a key removed from English).

A key that is missing from a translation falls back to English.

## Adding text to the game

1. Add the key to the right `en/*.ftl` file.
2. Use it through `Localizer::text("key")` or `Localizer::format("key", &[...])`, or through the `city_ui::text` helpers for simulation concepts.
3. `cargo test -p city-ui` scans the code. It fails if any key written in the code is missing from English.
