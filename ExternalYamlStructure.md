# Create a FolderHistory rule

Run the interactive rule creator from the project checkout.

```bash
cd ~/git_Projects/folder_history && cargo run -r --bin folder-history-rule-creator
```

If you already have the executable, run `folder-history-rule-creator` directly.
It saves confirmed rules to `~/.folder_history/operations.yaml`, which the
GUI and CLI load when you scan an archive under the same account.

## Try it with a pseudobulk stats call

| Prompt | Enter |
| --- | --- |
| Language | `python` |
| Function or command name | `write_pseudobulk_stats_tables` |
| Argument number, keyword, or flag | `2` |
| Read or write | `output` |
| Path type | `directory` |
| Literal fallback path | Press Enter |
| Limit to filenames or patterns | Press Enter to include all files beneath the folder |
| Next argument | Press Enter to finish the rule |

The creator prints the YAML. To see which paths it would link, enter an
archive directory, the notebook or script path relative to that directory,
and paste this call on one line:

```python
functions.write_pseudobulk_stats_tables(adata, "pseudobulk_leiden_stats", "leiden")
```

The preview shows matching paths in that archive. It uses the pasted call,
so use a literal path for this exercise. If the folder or tables do not
exist in the selected archive, the preview cannot show them. Press Enter
at the archive prompt to skip the preview.

Press Enter to save a new rule, or enter `n` to cancel. If a rule already
exists, the creator shows the changed lines and asks before replacing it.
Enter `y` to replace it or press Enter to keep the existing rule.
An identical rule needs no save.
After saving, rescan the archive in the GUI or CLI.

## Translate the same call by hand

Find the argument that points to a file or folder. In this Python call,
the second argument gives the folder where the function writes its tables.

```python
functions.write_pseudobulk_stats_tables(
    adata, "pseudobulk_leiden_stats", "leiden"
)
```

Write one operation for the call and describe that argument.

```yaml
operations:
  - name: write_pseudobulk_stats_tables
    language: python
    arguments:
      - position: 2
        role: output
        kind: directory
        file_type: StatisticalResults
```

Count arguments from **1**. Use `role: input` for a path the function reads
and `role: output` for a path it writes. Choose `kind: file` or
`kind: directory` to match the path. For a named argument such as
`output_dir="stats"`, use `name: output_dir`. Here, `adata` is an
in-memory object and `"leiden"` is a setting, so neither needs a path rule.

FolderHistory links the output folder and the indexed files beneath it.
`file_type` is optional and classifies the output as **Statistical results**.

Add more calls as entries under the same `operations:` list.
