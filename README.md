<!-- FOLDERHISTORY-GENERATED-DOCUMENTATION v1 -->

# FolderHistory

Offline desktop software for tracing research files to the scripts and notebooks that read or produce them.

See [External YAML structure](ExternalYamlStructure.md) to teach FolderHistory about your own analysis tools and file types.

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

## 2. Explore the indexed research directory

1. Start FolderHistory.
2. Choose **Open existing history** and select the top-level research directory containing `.folder_history/history.sqlite`.
3. Search for a dataset, notebook, script, figure or result.
4. Select a file to inspect its recorded connections, then explore the **Relationships** view.

If the index is absent, **Analyze a folder** can create one when you have write permission.

FolderHistory identifies file relationships; it does not run scripts or interpret their scientific conclusions.

## 3. Open Jupyter notebooks

JupyterLab is optional: it is needed for viewing or running `.ipynb` notebooks, not for using FolderHistory.

### Windows

Install Python from [python.org](https://www.python.org/downloads/). Open PowerShell:

```powershell
py --version
py -m pip install jupyterlab
py -m jupyter lab
```

JupyterLab opens in your browser. Navigate to the notebook using its file browser.

### macOS and Linux

Install Python 3, including the `venv` package if your Linux distribution requires it. In Terminal:

```bash
python3 -m venv ~/jupyterlab-env
~/jupyterlab-env/bin/python -m pip install jupyterlab
cd /path/to/your/research/folder
~/jupyterlab-env/bin/jupyter lab
```

Browse to a `.ipynb` notebook in JupyterLab.

**Opening a notebook does not execute it.** Running an old analysis may require additional packages, kernels and data. Make a working copy before running code that may overwrite results, and only execute trusted code.

## 4. Keep the history with the data

Keep `.folder_history/` with the research directory when copying or moving it. Its SQLite database contains the recorded file relationships and annotations. Archive-relative paths help keep the index portable.

## Appendix A — Windows: open `.ipynb` by double-clicking (optional)

Windows may not let you select a `.bat` file as the default notebook application. The following PowerShell setup creates a launcher and registers it under **Open with** for the current user. First install JupyterLab as described above.

```powershell
New-Item -ItemType Directory -Force C:\Tools | Out-Null
@'
param([Parameter(Mandatory=$true)][string]$Notebook)
$Notebook = (Resolve-Path -LiteralPath $Notebook).Path
$Directory = Split-Path -Parent $Notebook
$Filename = Split-Path -Leaf $Notebook
Set-Location -LiteralPath $Directory
py -m jupyter lab $Filename
'@ | Set-Content -Encoding UTF8 C:\Tools\Open-JupyterNotebook.ps1
@'
@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "C:\Tools\Open-JupyterNotebook.ps1" "%~1"
'@ | Set-Content -Encoding ASCII C:\Tools\Open-JupyterNotebook.bat
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

Right-click an `.ipynb` file → **Open with** → **Choose another app** → **JupyterLab Notebook** → **Always**. You may need to restart Explorer or sign out and back in.

Each double-click can start another JupyterLab server. For many notebooks, it may be simpler to keep one server open and navigate within it.

## Appendix B — Troubleshooting

- **`py` is not recognized:** Check the Windows Python installation and reopen PowerShell.
- **JupyterLab module not found:** Install it in the same Python environment used to start it.
- **Notebook cannot run:** The original analysis may need additional packages, kernels or files.
- **Index missing:** Select the research root containing `.folder_history/history.sqlite`.
- **Missing links:** Inspect the referenced scripts and any external dependencies.



## Developer notes

The Rust source contains the GUI, provenance scanner and batch CLI. Build and test with `cargo test -r` and `cargo build -r --bins`.

Batch scan: `folder-history-cli sub-folders-of /path/to/NAS --resume --write-readme`.

Generate documentation for an already indexed archive: `folder-history-cli generate-readme /path/to/Researcher`.
