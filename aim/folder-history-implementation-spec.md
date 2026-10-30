# FolderHistory — Implementation Contract v0.2
Status: design specification, not a claim of implemented functionality.
Priority: correctness > provenance coverage > performance > presentation.
Platform: Rust native desktop (egui/eframe), Windows/MSVC, Linux, macOS; no AI, cloud, or runtime network dependencies.

## Invariants (MUST)
1. Archive root `R` is user-selected and may contain many experiments. Recursively inventory files below `R`. Never follow directory symlinks/junctions outside `R`, or scan external dependencies.
2. Durable index: `R/.folder-history/history.sqlite`. All stored file identities are normalized, archive-relative paths (forward-slash separators). Never store drive letters, usernames, hostnames, external absolute directory trees, or mount prefixes in DB, logs, README, or exported evidence. Sanitize notebook/script snippets before persistence.
3. Portable: moving/copying `R` preserves all resolved links. Validate containment using canonical paths for existing files; lexical normalization alone is insufficient. Treat symlinks pointing outside as external.
4. SSH/network mount safety: build/modify a local temporary SQLite working copy, then publish a consistent snapshot into `R/.folder-history/` using safe staging/rename where supported. Never assume atomic rename or SQLite locking on remote mounts; detect/report publication failures; retain local working copy for recovery. Do not overwrite an existing DB on failure.
5. Read-only archive: Search and Build/Repair inspection available; edits staged locally, publication requires write access. Display archive/index permissions and unpublished changes.
6. User-verified links override inferred links across rescans. Persist candidate alternatives, evidence, confidence, conflict/rejection history, and handover annotations.
7. Distinguish OBSERVED (file exists, parsed operation), INFERRED (candidate provenance), CONFIRMED (human or execution record), and REJECTED. Do not represent inference as fact.
8. Scans must be incremental, idempotent, cancellable, and must not overwrite research data. Ignore `.folder-history/`, target/, .git/, and known transient build caches by default, configurable.

## Data model (suggested normalized SQLite schema)
- `files(id, rel_path UNIQUE, kind, size, mtime_ns, fingerprint?, exists_now, scan_id)`; `kind` includes FASTQ, BAM, H5AD, 10x matrix, image, table, script, notebook, log, config, other.
- `datasets(id, kind, label)` + `dataset_members(dataset_id,file_id,role)` for FASTQ R1/R2, 10x triplets, multi-sample collections.
- `sources(id, file_id, language, content_fingerprint)`; `steps(id, source_id, ordinal, location, command_redacted, step_kind)`; notebook steps may be cell-based, and multiple sessions/stages can share a notebook.
- `references(id, step_id, raw_basename, sanitized_hint, direction, resolution_status)`; `direction` READ/WRITE/READ_WRITE; `resolution_status` RESOLVED/AMBIGUOUS/MISSING/EXTERNAL.
- `reference_candidates(reference_id,file_id,score,evidence,selected,decision_origin)`.
- `edges(id, from_node, to_node, edge_kind, evidence, confidence, status)`; data/file/dataset -> step -> output data/file/dataset; maintain alternatives.
- `issues(id, reference_id?, category, status, note, last_seen_scan)`; `category` EXTERNAL/MISSING/AMBIGUOUS/CONFLICT/UNSAFE_PATH.
- `scans(id, started_at, finished_at, root_fingerprint, summary, result)`.
Schema migrations via `PRAGMA user_version`. Stable IDs/keys across rescans; preserve human decisions.

## Scanner pipeline
1. Select `R`; load existing index if present; open local working copy; inventory metadata recursively. Do not hash huge FASTQs by default; use path+size+mtime, optional streaming fingerprint.
2. Classify files and collect scripts/config/logs. Recognize `.sh`, `.bash`, `.py`, `.R`, `.ipynb`, `.nf`, `nextflow.config`, `Snakefile`, `.smk`, `Makefile`, `.ps1`, `.yaml/.yml`, `.toml`, `.json`, `.log`, `.out`, `.err`; identify Rust CLI invocations in scripts and logs. Do not execute source code.
3. Parse into individual steps/commands/cells, not one node per script. Start with robust shell tokenization, redirects (`>`, `>>`, `<`), pipes, `-o`, `--input`, `--output`, `--outdir`, and known tool-specific flags; support `samtools`, STAR, Cell Ranger, Nextflow, `nelrune`, `nelrune-vdj`, Rust CLIs. Treat dynamic variables/globs as patterns, not guaranteed paths. Python/R AST or conservative pattern extraction; notebook JSON cells, including `raw=...`, `ofile=...`, read_h5ad/write_h5ad and figure exports. Variable declaration alone is NOT a read/write edge. Notebook reopened later via `read_h5ad(ofile)` begins a new logical stage; execution counts are not definitive.
4. Extract input/output references and provenance evidence. For a script with several commands, connect outputs of earlier steps to later inputs. Recognize directory outputs and produced children when corroborated; avoid asserting every file in a directory was produced by a command.
5. Resolve references ONLY to files inside `R`: exact normalized relative path; safe relocation suffix; basename plus directory context; metadata/fingerprint; execution logs. Old absolute paths may be used ephemerally to derive sanitized suffix candidates, but MUST NOT persist their external prefix. A referenced absolute path outside `R` remains flagged EXTERNAL even when a candidate internal relocation exists until explicitly resolved/confirmed.
6. Producer ranking: exact output path > execution records/logs > time compatibility > contextual match > newest plausible producer as tie-breaker. `mtime` is weak evidence (copies/edits change it). Preserve all plausible producers, scores, and evidence; surface ambiguity rather than silently selecting uncertain links.
7. Persist local DB, compute integrity counts, display graph and issues, publish archive DB only on explicit save or documented auto-publish policy after successful scan.

## GUI behavior
- Landing: **Analyze a folder** / **Open existing history**. Show archive root and index location. Search default once opened. Build/Repair ALWAYS viewable, including read-only.
- Scan progress: counts, active file, cancellation, errors, remote I/O warnings. Distinguish zero files from zero extracted relationships.
- Search by filename, sample, script, figure, output; navigate both directions. Graph: upstream ancestry and downstream descendants; click edge -> exact source script/command/notebook cell and evidence.
- Build/Repair issue groups: EXTERNAL, MISSING, AMBIGUOUS, CONFLICT; click issue -> source location, sanitized reference, candidate internal files; actions: confirm internal match, reject, intentionally external, missing, add/edit handover note. Persist decisions.
- Export provenance slice as portable Markdown: selected artifact -> upstream graph, exact relevant commands/cells (sanitized), input/output paths, evidence, confidence, missing dependencies, optional version/commit. No external paths or sensitive data by default.
- Export `R/README.html`: self-contained offline HTML/CSS, relative clickable file links, archive summary, issue list, notes, provenance entry points; HTML escape all user/source content. README is generated, DB is authoritative.

## Acceptance tests (minimum)
A. Move archive from `J:\\...\\Johan_Flygare` to `/mnt/archive/Johan_Flygare`: all resolved links still work; DB contains no original mount/user path.
B. Shell: `samtools sort -o results/a.sorted.bam input/a.bam` -> input -> command -> output.
C. Two candidate scripts produce same file: evidence ranking and newest plausible tie-breaker; alternatives retained and editable.
D. Notebook `raw='x.h5ad'; ofile='y.h5ad'; read_h5ad(raw); write_h5ad(ofile)` -> x -> stage -> y; later appended `read_h5ad(ofile)` -> distinct stage.
E. Reference `/home/olduser/project/out.h5ad` absent from archive -> EXTERNAL/MISSING issue with only sanitized basename/suffix stored; never traverse original path.
F. Read-only archive: searchable, Build/Repair accessible, no archive writes; corrections staged locally.
G. Repeated scan leaves human-confirmed decisions and notes unchanged; scan interruption leaves prior published DB intact.
H. `README.html` opens offline and uses only relative links; all injected strings escaped.
I. SSH-mounted archive publication failure does not corrupt previously published SQLite DB.

## Implementation order
P0: archive-relative inventory + safe DB publication + progress + zero-count diagnostics.
P1: shell command parser + file READ/WRITE edges + matching + issue categories.
P2: notebook stage semantics, Python/R/Nextflow, tool-specific extractors, log evidence.
P3: human correction GUI + stable rescans + HTML/Markdown exports.
P4: deeper Rust CLI schema inference, directory outputs, content fingerprints, richer graph UI.

## Non-goals
Do not execute arbitrary scripts, infer clinical findings, depend on Ollama/LLMs, copy raw data into SQLite, auto-follow references outside the archive, or claim provenance certainty based only on timestamps.
