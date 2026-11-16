# Scientific function provenance rules

`scientific_functions.yaml` is bundled into the scanner at build time. The source paths
listed in each rule identify the implementation used to verify the contract.

To add or override rules for one scanned research folder, create
`<research-root>/.folder_history/provenance_rules.yaml` with the same schema:

```yaml
version: 1
functions:
  - name: my_export
    language: python  # python or r
    directory_arg: 2  # 1-based positional argument, or named path/output_dir
    files: [summary.csv]
    patterns: ['*.tsv']  # only a leading * wildcard is supported
    source: 'my laboratory function, version 1'
```

For single-file writers use `output_arg: 2`, optionally with
`default_output: output.csv`. Named output arguments supported include
`output_file`, `out_csv`, `ofile`, `file`, and `filename`.

The scanner only creates links to indexed, unambiguously resolved targets.
An absolute path outside the scanned root is recorded as an external issue,
not silently associated with an unrelated file. Rules describe possible
outputs of calls, **not evidence that the code was executed**.

Rescan after changing a rule. Existing manual resolutions are retained.
