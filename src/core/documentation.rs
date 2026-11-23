//! Shared offline documentation for the repository and individual research archives.
use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use super::{Artifact, Relation, Issue, local_index_path, report};

const MARKER: &str = "FOLDERHISTORY-GENERATED-DOCUMENTATION v1";
const INTRO: &str = r#"# Your Research Data and Analysis History

This directory contains research data, analysis scripts, Jupyter notebooks, figures and results from your research projects.

The directory has been indexed using **[FolderHistory](https://github.com/stela2502/folder_history)** to help navigate files and trace the relationships between them.

## What does FolderHistory do?

FolderHistory identifies which input files are used by analysis scripts and which output files those scripts generate.

For example, you can start with a figure or results table and trace it back to the script or Jupyter notebook that produced it, along with the input data used in the analysis.

**FolderHistory does not attempt to explain the scientific analysis itself.** Its purpose is to connect input data, analysis scripts and output files so that you can find the relevant code.

## Understanding your analyses with AI

Once you have located a script or notebook, you can ask an AI assistant to explain its code, statistical methods, parameters or software dependencies, or help adapt an old analysis to a newer environment.

For unpublished or sensitive research data, check your institution's data-sharing policies before uploading files to an external AI service. Sharing analysis code without underlying research data is often sufficient.
"#;

const HELP: &str = r#"
## 1. Install FolderHistory

Download a precompiled application for your system from the [official releases](https://github.com/stela2502/folder_history/releases). You do not need Rust to use the application.

### Windows

Download the Windows release archive, extract it, and double-click `folder-history.exe`.

### macOS

Download the release matching your Mac (Apple Silicon ARM64 or Intel x86-64), extract it, and follow the instructions packaged with the release. Some releases contain a standalone executable rather than a `.app` bundle.

### Linux

Download and extract the Linux release. Open a terminal in its directory:

```bash
chmod +x folder-history
./folder-history
```

## 2. Open your research archive

**When to use this:** You have received a research directory and want to explore its contents.

1. Start FolderHistory. If your archive was already indexed, choose **Open existing history** → **Choose indexed folder...** and select the research directory. Recent folders can also be reopened from the welcome screen.
2. FolderHistory opens the existing index without rescanning or changing the archive. It normally starts in **Spectator**.
3. If there is no index, choose **Analyze a folder** → **Choose folder to analyze...** instead. This builds an index and enables the repair tools when a writable index is available.

The index is stored in `.folder_history/history.sqlite`. You do not need Python, JupyterLab or an internet connection to browse it.

## 3. Spectator — explore analyses and files

**When to use this:** You want an overview of what experiments, samples, scripts and results exist, but do not yet know a filename.

Open the **Spectator** tab (or click **⌂ Analyses**). It organizes files into expandable levels: **analysis group → inferred collection → folder / possible sample → workflow → files**. Expand a heading to see its contents; the counts beside headings describe the files in that group. The grouping is a navigation aid, not a guarantee that every file belongs to one uniquely established analysis.

The overview displays a total FASTQ count, and group and folder headings also report FASTQ counts (`.fastq`, `.fq` and their gzip-compressed forms). These are counts of indexed files, not inferred biological samples. Some group labels are inferred from directory names; **Folder / possible sample** does not assert that a verified sample or run was identified.

The simplified navigation legend uses **□ Input**, **● Script**, and **▶ Results**. The more detailed colored categories below describe file types and should not be mistaken for independently verified provenance. Scientist-confirmed classifications, when available, are displayed separately.

### Understanding file colors

Spectator colors indicate a file's **role**, not its quality, correctness or whether an analysis succeeded:

- **Light blue — Raw data:** original inputs such as sequencing files.
- **Purple — Processing:** scripts that prepare or transform data.
- **Darker blue — Processed data:** derived datasets. The classification may need verification.
- **Red — Analysis:** analysis scripts and notebooks.
- **Yellow — Tables:** result tables and metadata.
- **Green — Figures:** plots and documents.
- **Gray — References:** other or reference files.

Some summaries also use different shapes for roles, so the legend is useful even when colors are difficult to distinguish. A file's role is inferred from available information; inspect the file and relationships when unsure.

**Open a file:** Double-click its filename. The application asks your operating system to open it with its associated program. If the program is missing, see the Jupyter section below for notebooks. The file actions also allow opening the containing folder.

## 4. Search — find a particular file

**When to use this:** You remember a filename, sample identifier or part of a path.

1. Open **Search** and enter part of the name (for example, `volcano`, `sample_12` or `.ipynb`). Matching is case-insensitive.
2. Click a result to select it. Click **Relationships** beside the selected file to inspect its recorded connections.
3. Double-click a result, or use **Open file with default application**, to open it. Right-click for **Open file**, **Open containing folder**, or **Copy path**.

**Show all files (without search)** removes the need for a search term. The view limits the displayed result list, so searching by a distinctive name is usually easier for large archives.

## 5. Relationships — follow the history of a file

**When to use this:** You found a result and want to know which script, notebook or input files are connected to it.

Select a file in Search, then open **Relationships**. The **Direct relationships** section lists recorded links as **source —relationship type→ target** and shows the associated evidence. A link marked **[manual]** was added or confirmed manually. Use **Explore** on a connected file to continue following the chain.

For example, begin with a figure, inspect its relationship to a notebook, then explore the notebook's relationships to input datasets. FolderHistory reconstructs these connections from recorded evidence; it does not explain the statistical methods or prove that a particular script was executed in a specific run.

## 6. Build / Repair — investigate and correct the index

**When to use this:** A file relationship is missing, ambiguous or needs human review. Most researchers can explore their archive without changing anything here.

**Build / Repair** displays the archive and index locations, counts of files and links, and open issue occurrences. The **Analysis graph context** selector lets you focus on one analysis; where writing is enabled, you can save a clearer analysis name. Unassigned dependencies may have suggested analysis groups, but suggestions do not create provenance links automatically.

### Evidence and Resolution

Use **Evidence** to inspect unresolved references and the information FolderHistory found in scripts or notebooks. Selecting an issue exposes its **Resolution** controls. Review the candidates and the original code before recording a decision; do not guess merely to make a warning disappear. Manual decisions are stored with the index.

When the archive is opened read-only, evidence and previous decisions remain visible, but editing controls are unavailable. To make changes, open the directory through **Analyze a folder** and use a writable index.

### Re-scan versus RE-RESOLVE

- **Re-scan archive** (inside Build / Repair) reads the filesystem and refreshes automatically extracted information. Use it when files have changed or been added.
- **RE-RESOLVE** (the red button in the top toolbar) re-evaluates existing notebook and script-chain candidates from **stored evidence only**. It does **not** scan the filesystem again.

Manual relationships and resolutions are intended to survive rescanning; automatically extracted references can be refreshed. The **Generate README.md + HTML** action writes updated help/report files when the archive is writable, while preserving unrelated researcher-authored README files.

## 7. Worked example — where did this figure come from?

1. In **Spectator**, expand the relevant analysis group and look under figures, or use **Search** to find part of the figure's filename.
2. Select the figure and switch to **Relationships**.
3. Follow a recorded link to the analysis script or `.ipynb` notebook, using **Explore** to move between connected files.
4. Inspect that script's or notebook's relationships to find the input dataset and intermediate files.
5. Double-click the notebook or script to inspect the actual analysis in its associated application. If the link is missing, check **Build / Repair → Evidence** for an unresolved reference.

**FolderHistory finds the relevant code; the code itself explains the analysis.** An AI assistant can help interpret it, subject to your institution's data-sharing rules.

## 8. Keep the history with the data

When copying or moving a research archive, keep `.folder_history/` with it. Its SQLite index contains the recorded file relationships and annotations. Archive-relative paths help portability, but external dependencies may still need attention.

## 9. Open Jupyter notebooks

Jupyter notebooks (`.ipynb`) contain analysis code and may also include saved figures, results and explanations. **JupyterLab is optional**: you need it to inspect or run notebooks, not to use FolderHistory.

### Windows 11 — Install JupyterLab and add it to Open with

#### Step 1 — Install Python

1. Download Python from the [official Windows installer page](https://www.python.org/downloads/windows/).
2. Run the installer. If offered, select **Add Python to PATH** and install the Python launcher.
3. Open **PowerShell** from the Start menu and check the installation:

```powershell
py --version
```

If `py` is not recognized, check that the Python launcher is installed and reopen PowerShell.

#### Step 2 — Install and test JupyterLab

In PowerShell, run:

```powershell
py -m pip install jupyterlab
py -m jupyter lab
```

JupyterLab should open in your default browser. You can already browse to a notebook and read it. The next step makes opening `.ipynb` files from Windows Explorer easier.

#### Step 3 — Register JupyterLab Notebook as an Open with application

Windows 11 does not reliably allow a `.bat` launcher to be selected directly as a default application. The following **one-time PowerShell setup** creates a launcher and registers it as **JupyterLab Notebook** for your Windows account. Paste the complete block into PowerShell:

```powershell
# Create the directory for the launcher
New-Item -ItemType Directory -Force C:\Tools | Out-Null

# Launch JupyterLab in the selected notebook's directory
@'
param([Parameter(Mandatory=$true)][string]$Notebook)

$Notebook = (Resolve-Path -LiteralPath $Notebook).Path
$Directory = Split-Path -Parent $Notebook
$Filename = Split-Path -Leaf $Notebook

Set-Location -LiteralPath $Directory
py -m jupyter lab $Filename
'@ | Set-Content -Encoding UTF8 C:\Tools\Open-JupyterNotebook.ps1

# Windows batch wrapper
@'
@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "C:\Tools\Open-JupyterNotebook.ps1" "%~1"
'@ | Set-Content -Encoding ASCII C:\Tools\Open-JupyterNotebook.bat

# Register an Open with application for the current user
$launcher = "C:\Tools\Open-JupyterNotebook.bat"
$base = "HKCU:\Software\Classes"
$app = "$base\FolderHistory.JupyterNotebook"
$command = "`"$launcher`" `"%1`""

New-Item "$app\shell\open\command" -Force | Out-Null
Set-Item -Path "$app\shell\open\command" -Value $command
Set-Item -Path $app -Value "JupyterLab Notebook"
New-Item "$base\.ipynb\OpenWithProgids" -Force | Out-Null
New-ItemProperty -Path "$base\.ipynb\OpenWithProgids" -Name "FolderHistory.JupyterNotebook" -PropertyType String -Value "" -Force | Out-Null
```

This creates two small launcher files in `C:\Tools` and adds a per-user file association. It does **not** change the notebooks themselves.

#### Step 4 — Choose JupyterLab in Windows Explorer

1. Locate any `.ipynb` file in **File Explorer**.
2. Right-click it → **Open with** → **Choose another app**.
3. Select **JupyterLab Notebook**.
4. Select **Always** if you want double-clicking to open notebooks in JupyterLab.

If the application is not listed immediately, restart Explorer or sign out and back in. Opening several notebooks this way may start more than one JupyterLab server; you can also keep a single server open and browse within it.

### Linux — Install JupyterLab

Install Python 3 and its `venv` package if needed. In a terminal:

```bash
python3 -m venv ~/jupyterlab-env
~/jupyterlab-env/bin/python -m pip install jupyterlab
cd /path/to/your/research/folder
~/jupyterlab-env/bin/jupyter lab
```

JupyterLab opens in your browser; navigate to the notebook. Your desktop's **Open With** menu may also support notebook associations, but no system-wide file association is necessary.

### macOS — Install JupyterLab

Install Python from the [official macOS downloads](https://www.python.org/downloads/macos/). Open Terminal:

```bash
python3 -m venv ~/jupyterlab-env
~/jupyterlab-env/bin/python -m pip install jupyterlab
cd /path/to/your/research/folder
~/jupyterlab-env/bin/jupyter lab
```

JupyterLab opens in your browser; navigate to the notebook. Finder's **Open With** requires an appropriate application bundle and is not automatically provided by installing JupyterLab.

### Find an analysis using FolderHistory

1. Open FolderHistory and load your indexed research directory.
2. Search for a figure, results table or input dataset.
3. Follow the relationships to the associated script or notebook.
4. Open the notebook with JupyterLab (on Windows, using the **Open with** application configured above).
5. Read the code, parameters and saved results to understand the analysis.

**Opening a notebook does not execute its code.** Rerunning old analyses may require additional packages, kernels or data files. Work on a copy if code could overwrite results, and only execute trusted code. You can also share the analysis code with an AI assistant to help interpret it, subject to your institution's data-sharing policies.

## Appendix — Troubleshooting

- **`py` is not recognized:** Check the Windows Python installation and reopen PowerShell.
- **JupyterLab module not found:** Install it in the same Python environment used to start it.
- **Notebook cannot run:** The original analysis may need additional packages, kernels or files.
- **Index missing:** Select the research root containing `.folder_history/history.sqlite`.
- **Missing links:** Inspect the referenced scripts and any external dependencies.

"#;

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}
fn inline(s: &str) -> String {
    // Trusted built-in Markdown, never user input.
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '[' {
            let label: String = chars.by_ref().take_while(|&x| x != ']').collect();
            if chars.peek() == Some(&'(') {
                chars.next();
                let url: String = chars.by_ref().take_while(|&x| x != ')').collect();
                if url.starts_with("https://") {
                    out.push_str(&format!("<a href=\"{}\">{}</a>", escape_html(&url), escape_html(&label)));
                    continue;
                }
            }
            out.push('['); out.push_str(&escape_html(&label)); continue;
        }
        if c == '`' {
            let value: String = chars.by_ref().take_while(|&x| x != '`').collect();
            out.push_str(&format!("<code>{}</code>", escape_html(&value))); continue;
        }
        if c == '*' && chars.peek() == Some(&'*') {
            chars.next();
            let mut value = String::new();
            while let Some(x) = chars.next() {
                if x == '*' && chars.peek() == Some(&'*') { chars.next(); break; }
                value.push(x);
            }
            out.push_str(&format!("<strong>{}</strong>", escape_html(&value))); continue;
        }
        out.push_str(&escape_html(&c.to_string()));
    }
    out
}
pub fn html_from_markdown(md: &str) -> String {
    let mut out = format!("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>FolderHistory — Research Data and Analysis History</title><style>body{{font:16px/1.6 system-ui;max-width:980px;margin:2em auto;padding:0 1.5em;background:#16191e;color:#eee}}a{{color:#9ccaff}}pre{{background:#252a32;padding:1em;overflow-x:auto;border-radius:6px}}code{{overflow-wrap:anywhere}}article{{border:1px solid #555;padding:1em;margin:1em 0;border-radius:8px}}small{{color:#bbb}}details{{margin:1em 0}}summary{{cursor:pointer;font-weight:bold}}</style></head><body><!-- {MARKER} -->");
    let mut in_code = false;
    let mut in_list = false;
    let mut appendix = false;
    for line in md.lines() {
        if line.starts_with("```") {
            if in_list { out.push_str("</ul>"); in_list=false; }
            if in_code { out.push_str("</code></pre>"); } else { out.push_str("<pre><code>"); }
            in_code = !in_code; continue;
        }
        if in_code { out.push_str(&escape_html(line)); out.push('\n'); continue; }
        if line.starts_with("## Appendix") && !appendix { out.push_str("<details><summary>Advanced setup and troubleshooting</summary>"); appendix=true; }
        if let Some(heading) = line.strip_prefix("#### ") { out.push_str(&format!("<h4>{}</h4>", inline(heading))); continue; }
        if let Some(heading) = line.strip_prefix("### ") { out.push_str(&format!("<h3>{}</h3>", inline(heading))); continue; }
        if let Some(heading) = line.strip_prefix("## ") { out.push_str(&format!("<h2>{}</h2>", inline(heading))); continue; }
        if let Some(heading) = line.strip_prefix("# ") { out.push_str(&format!("<h1>{}</h1>", inline(heading))); continue; }
        if line.starts_with("- ") {
            if !in_list { out.push_str("<ul>"); in_list=true; }
            out.push_str(&format!("<li>{}</li>", inline(&line[2..]))); continue;
        }
        if in_list { out.push_str("</ul>"); in_list=false; }
        if line.trim().is_empty() { continue; }
        if line.starts_with("---") { out.push_str("<hr>"); continue; }
        out.push_str(&format!("<p>{}</p>", inline(line)));
    }
    if in_list { out.push_str("</ul>"); }
    if appendix { out.push_str("</details>"); }
    out.push_str("</body></html>");
    out
}
pub fn researcher_markdown(root: &Path, artifacts: &[Artifact], relations: &[Relation], issues: &[Issue]) -> String {
    let mut md = format!("<!-- {MARKER} -->\n\n{INTRO}\n\n## This research directory\n\n**{} indexed files · {} relationships · {} review items.**\n\n", artifacts.len(), relations.len(), issues.len());
    md.push_str("The index is stored in `.folder_history/history.sqlite`. The following findings are based on the recorded archive inventory and links.\n\n");
    md.push_str(&report::markdown(root, artifacts, relations, issues));
    md.push_str("\n---\n");
    md.push_str(HELP);
    md
}
/// Example of the help generated for a researcher, without fabricated archive findings.
pub fn example_local_help_markdown() -> String {
    format!("<!-- {MARKER} -->\n\n{INTRO}\n\n{HELP}")
}
pub fn repository_markdown() -> String {
    format!("<!-- {MARKER} -->\n\n# FolderHistory\n\nOffline desktop software for tracing research files to the scripts and notebooks that read or produce them.\n\n{INTRO}\n\n{HELP}\n\n## Developer notes\n\nThe Rust source contains the GUI, provenance scanner and batch CLI. Build and test with `cargo test -r` and `cargo build -r --bins`.\n\nBatch scan: `folder-history-cli sub-folders-of /path/to/NAS --resume --write-readme`.\n\nGenerate documentation for an already indexed archive: `folder-history-cli generate-readme /path/to/Researcher`.\n")
}
pub fn read_index(root: &Path) -> Result<(Vec<Artifact>, Vec<Relation>, Vec<Issue>)> {
    let current = local_index_path(root);
    let legacy = root.join(".folder-history/history.sqlite");
    let path = if current.is_file() { current } else { legacy };
    if !path.is_file() { bail!("No FolderHistory index found in {}", root.display()); }
    let db = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("Opening {}", path.display()))?;
    use super::history_db::DbArchiveView;
    db.archive_view()
}
fn is_managed(name: &str, existing: &str) -> bool {
    existing.contains(MARKER) || (name == "README.html"
        && existing.contains("<title>FolderHistory archive report</title>")
        && existing.contains("Archive handover findings"))
}
#[derive(Debug, Default)]
pub struct DocumentationOutcome {
    pub written: Vec<String>,
    pub skipped: Vec<String>,
}

impl DocumentationOutcome {
    pub fn summary(&self) -> String {
        let mut message = if self.written.is_empty() {
            "No documentation files written".to_owned()
        } else {
            format!("Generated {}", self.written.join(" and "))
        };
        if !self.skipped.is_empty() {
            message.push_str(&format!(
                "; WARNING: preserved existing researcher document(s): {}",
                self.skipped.join(", ")
            ));
        }
        message
    }
}

fn write_managed(path: &Path, content: &str) -> Result<bool> {
    let name = path.file_name().context("Missing filename")?.to_string_lossy();
    if path.exists() {
        let existing = std::fs::read_to_string(path)
            .with_context(|| format!("Reading {}", path.display()))?;
        if !is_managed(&name, &existing) {
            eprintln!("[WARN] Existing researcher document preserved: {}", path.display());
            return Ok(false);
        }
    }
    let temp = path.with_file_name(format!(".{name}.folderhistory.tmp"));
    std::fs::write(&temp, content)?;
    if let Err(e) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(e.into());
    }
    Ok(true)
}

pub fn generate(root: &Path, artifacts: &[Artifact], relations: &[Relation], issues: &[Issue]) -> Result<DocumentationOutcome> {
    let md = researcher_markdown(root, artifacts, relations, issues);
    let html = html_from_markdown(&md);
    let mut outcome = DocumentationOutcome::default();
    for (name, content) in [("README.md", md.as_str()), ("README.html", html.as_str())] {
        if write_managed(&root.join(name), content)? {
            outcome.written.push(name.to_owned());
        } else {
            outcome.skipped.push(name.to_owned());
        }
    }
    Ok(outcome)
}

/// Create the researcher-facing HTML only when it does not already exist.
/// Never overwrite an existing HTML document, whether generated or researcher-authored.
pub fn generate_missing_html(root: &Path, artifacts: &[Artifact], relations: &[Relation], issues: &[Issue]) -> Result<bool> {
    let path = root.join("README.html");
    if path.exists() { return Ok(false); }
    let html = html_from_markdown(&researcher_markdown(root, artifacts, relations, issues));
    // create_new prevents overwriting a file created concurrently by another process.
    use std::io::Write;
    let mut file = match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    if let Err(e) = file.write_all(html.as_bytes()) {
        let _ = std::fs::remove_file(&path);
        return Err(e.into());
    }
    Ok(true)
}

pub fn generate_from_index(root: &Path) -> Result<DocumentationOutcome> {
    let (a, r, i) = read_index(root)?;
    generate(root, &a, &r, &i)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_formats_and_preserve_unmanaged() {
        let dir=std::env::temp_dir().join(format!("fh_docs_test_{}",std::process::id()));
        let _=std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        generate(&dir,&[],&[],&[]).unwrap();
        assert!(std::fs::read_to_string(dir.join("README.html")).unwrap().contains("<details>"));
        assert!(std::fs::read_to_string(dir.join("README.md")).unwrap().contains("JupyterLab"));
        generate(&dir,&[],&[],&[]).unwrap();
        std::fs::write(dir.join("README.md"),"researcher notes").unwrap();
        let outcome = generate(&dir,&[],&[],&[]).unwrap();
        assert_eq!(outcome.skipped, vec!["README.md"]);
        assert_eq!(outcome.written, vec!["README.html"]);
        assert_eq!(std::fs::read_to_string(dir.join("README.md")).unwrap(), "researcher notes");
        let _=std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn escapes_html() {
        assert!(html_from_markdown("# <test>").contains("&lt;test&gt;"));
    }
}
