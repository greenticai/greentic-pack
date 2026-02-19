# Internationalise a Pack Howto

## Goal

Make pack UX text locale-aware across:

- pack display metadata
- component QA prompts
- pack-level QA prompts

## 1) Start with wizard locale scaffolding

Use wizard commands with a locale so the pack starts with i18n structure:

```bash
greentic-pack wizard new-app <PACK_ID> --out <DIR> --locale en --name "My Pack"
greentic-pack wizard new-extension <PACK_ID> --kind <KIND> --out <DIR> --locale en --name "My Extension"
```

This creates:

- `assets/i18n/en.json`

## 2) Add locale bundles

Create one file per locale:

- `assets/i18n/en.json`
- `assets/i18n/es.json`
- `assets/i18n/de.json`

Example:

```json
{
  "pack.name": "Weather Assistant",
  "qa.title.setup": "Setup",
  "qa.question.region.label": "Region",
  "qa.question.region.help": "Choose where this pack runs"
}
```

## 3) Use i18n keys in QA specs

QA prompts resolve through `I18nText` keys. Define question titles/labels/help with stable keys and provide translations in each locale file.

`greentic-pack qa` resolves text from:

- `assets/i18n/<locale>.json`
- `--locale <tag>` flag at runtime

If a key is missing, QA falls back to inline default text when available.

## 4) Pack-level QA i18n

Pack-level QA is read from `pack.cbor` metadata key `greentic.qa` and usually points to canonical CBOR files:

- `qa/pack/default.cbor`
- `qa/pack/setup.cbor`
- `qa/pack/update.cbor`
- `qa/pack/remove.cbor`

Those specs can use i18n keys the same way component QA does.

## 5) Validate and test locales

Run the full loop:

```bash
greentic-pack lint --in <DIR>
greentic-pack qa --pack <DIR> --mode setup --locale en
greentic-pack qa --pack <DIR> --mode setup --locale es
greentic-pack build --in <DIR>
greentic-pack inspect --in <DIR>/dist/pack.gtpack --json
```

What to verify:

- prompts/questions render in the selected locale
- no missing-key placeholders in interactive QA output
- built pack still passes lint/inspect/doctor

## 6) Recommended conventions

- Keep keys stable and namespaced (`pack.*`, `qa.*`, `component.<id>.*`).
- Treat `en` as baseline and require parity checks for added locales.
- Keep locale files sorted deterministically to reduce diff noise.
- Add CI checks that run QA with at least one non-default locale.

## Troubleshooting

- `failed to load i18n bundle`: ensure `assets/i18n/<locale>.json` exists and is valid JSON.
- untranslated text appears: missing key in selected locale; add translation or fallback default.
- QA mismatch errors: verify locale text keys did not alter schema/question IDs.
