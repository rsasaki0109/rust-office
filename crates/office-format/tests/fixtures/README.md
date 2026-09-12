# Format fixtures (golden files)

Checked-in JSON and ODT XML snapshots for `office-format` regression tests.

## Layout

- `json/*.roffice.json` — native document snapshots
- `odt/*.content.xml` — deterministic `content.xml` from `build_content_xml`
- `odt/*.styles.xml` — `styles.xml` when the sample has header/footer
- `docx/*.document.xml` — deterministic `word/document.xml` from DOCX writer
- `json/legacy_v2.roffice.json` — older schema still accepted on load

## Updating

```bash
UPDATE_GOLDEN=1 cargo test -p office-format --test golden
```

Review the diff carefully before committing.
