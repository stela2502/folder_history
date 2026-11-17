<!-- FOLDERHISTORY-GENERATED-DOCUMENTATION v1 -->

# Your Research Data and Analysis History

This directory contains research data, analysis scripts, Jupyter notebooks, figures and results from your research projects.

The directory has been indexed using **[FolderHistory](https://github.com/stela2502/folder_history)** to help navigate files and trace the relationships between them.

## What does FolderHistory do?

FolderHistory identifies which input files are used by analysis scripts and which output files those scripts generate.

For example, you can start with a figure or results table and trace it back to the script or Jupyter notebook that produced it, along with the input data used in the analysis.

**FolderHistory does not attempt to explain the scientific analysis itself.** Its purpose is to connect input data, analysis scripts and output files so that you can find the relevant code.

## Understanding your analyses with AI

Once you have located a script or notebook, you can ask an AI assistant to explain its code, statistical methods, parameters or software dependencies, or help adapt an old analysis to a newer environment.

For unpublished or sensitive research data, check your institution's data-sharing policies before uploading files to an external AI service. Sharing analysis code without underlying research data is often sufficient.



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

Open the **Spectator** tab (or click **⌂ Analyses**). It organizes files into expandable levels: **analysis group → experiment → sample/run → workflow → files**. Expand a heading to see its contents; the counts beside headings describe the files in that group. The grouping is a navigation aid, not a guarantee that every file belongs to one uniquely established analysis.

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

