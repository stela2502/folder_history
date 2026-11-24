# Scientific provenance rules

The scanner bundles `scientific_functions.yaml` at build time. Add or
override an operation for a scanned archive in
`<research-root>/.folder_history/provenance_rules.yaml`.

```yaml
operations:
  - name: my_export
    language: python
    arguments:
      - position: 2
        aliases: [path, output_dir]
        role: output
        kind: directory
        expected_files: [summary.csv, '*.tsv']
```

Positions are 1-based. An output directory links to files listed by
`expected_files`. Omit that field to link the directory and all indexed
files below it. Use `kind: file` for a single-file output and `default`
for a literal path used when the argument is absent.

Rescan after editing a rule. A rule describes what a call can produce,
while saved execution evidence comes from the archive.
