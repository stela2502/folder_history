# FolderHistory 0.1.0

An offline, deterministic, native desktop application for searching research archives and reconstructing notebook-to-data relationships.

**Status: first working scaffold, not yet a complete provenance reconstruction engine.**

## Windows — start here

Install [Rust](https://rustup.rs/) and the required Microsoft C++ build tools for the MSVC Rust target. In PowerShell, from this extracted directory:

```powershell
cargo run --release
```

To build an executable to distribute:

```powershell
cargo build --release
```

The executable is `target\release\folder-history.exe`. This initial package is source code, **not** a precompiled Windows binary.

## Linux and macOS

With a supported Rust toolchain and native dependencies for eframe/rfd installed:

```bash
cargo run --release
```

The GUI code is cross-platform, but builds are only expected on desktop OS targets supported by `eframe` and `rfd`. Linux MUSL static linking and cross-compilation are not yet validated.

## First use

1. Click **Open for editing**, select a research folder, then select **Build / Repair**.
2. Click **Scan folder and extract notebook references**. The database is created at `.archeo/folder-history.sqlite` within that folder.
3. Use **Search** to find files. Click a result to see direct notebook read/write relationships.
4. In **Build / Repair**, add a manual relationship where needed; manual links survive rescans.
5. Colleagues can click **Open archive** to open the database in read-only mode, without rescanning.

## What the first parser does

- Recursively indexes regular files and stores relative paths in SQLite.
- Parses `.ipynb` JSON source cells without executing them.
- Recognizes `raw = "..."` and `ofile = "..."` assignments and simple `read_h5ad(...)`, `read_10x_mtx(...)`, `read_10x_h5(...)`, `write_h5ad(...)` and `write(...)` calls.
- Resolves relative paths and uniquely matching basenames; records unmatched references explicitly.
- Saves evidence identifying the notebook cell and call.
- Preserves manually curated links across rescans.

## Important limitations in v0.1.0

- **No complete FASTQ → UMAP chain yet.** Cell Ranger, sample-sheet, figures, variable-flow and notebook session parsing are future work.
- Relationships are direct and searchable; recursive graph traversal and provenance-slice export are not implemented yet.
- A filename-only match is a heuristic and should be manually checked. The first version does not model historical file versions or directory relocation mappings.
- Notebook parsing is deliberately narrow. It does not yet understand dynamic paths, Python variable propagation, aliases, `scanpy` figure filenames or `ScanpyAutoAnalyser` implementations.
- Scan is synchronous and can temporarily freeze the GUI on large archives. For initial testing, use a small representative folder.
- Editing requires write access to the database and its containing folder. SQLite may create journal files during writes; read-only use should be tested on the destination network share.
- No AI, Ollama, network calls, or telemetry are used by the application itself.

## Next milestones

1. Robust notebook-stage extraction, output plots, and path relocation evidence.
2. Recursive FASTQ-to-result and result-to-FASTQ traversal, including sample-sheet and Cell Ranger dataset nodes.
3. Manual graph editor, uncertainty review, and read-only published graph.
4. Focused Markdown provenance-slice export with source cells and evidence.
5. Windows/Linux release automation, and MUSL or AppImage validation.
