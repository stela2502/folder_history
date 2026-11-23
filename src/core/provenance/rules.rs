//! Conservative, language-light provenance candidate extraction.
//! No script execution or dependency on imported helper implementations.
use super::*;
use serde::Deserialize;
use crate::core::ArtifactRole;

#[derive(Debug, Deserialize)]
pub(super) struct RuleSet {
    pub(super) version: u32,
    #[serde(default)]
    pub(super) functions: Vec<FunctionRule>,
    #[serde(default)]
    pub(super) operations: Vec<OperationRule>,
    #[serde(default)]
    pub(super) plotting: Vec<PlotRule>,
    #[serde(default)] pub(super) file_types: Vec<FileTypeRule>,
    #[serde(default)] pub(super) file_groups: Vec<FileGroupRule>,
}
#[derive(Debug, Clone, Deserialize)]
pub(super) struct FileTypeRule {
    pub(super) pattern: String,
    pub(super) file_type: ArtifactRole,
}
#[derive(Debug, Clone, Deserialize)]
pub(super) struct FileGroupRule {
    #[allow(dead_code)] // Retained as part of the YAML rule schema.
    pub(super) name: String,
    pub(super) required: Vec<String>,
    pub(super) file_type: ArtifactRole,
    #[serde(default)] pub(super) members_file_type: Option<ArtifactRole>,
}
#[derive(Debug, Clone, Deserialize)]
pub(super) struct PlotRule {
    pub(super) module: String,
    pub(super) save_argument: String,
    pub(super) directory_setting: String,
    pub(super) default_directory: String,
    pub(super) format_setting: String,
    pub(super) default_format: String,
    #[serde(default)]
    pub(super) prefix_overrides: HashMap<String, String>,
}
#[derive(Debug, Deserialize)]
pub(super) struct FunctionRule {
    pub(super) name: String,
    pub(super) language: String,
    pub(super) directory_arg: Option<usize>,
    pub(super) output_arg: Option<usize>,
    pub(super) default_output: Option<String>,
    #[serde(default)] pub(super) files: Vec<String>,
    #[serde(default)] pub(super) patterns: Vec<String>,
    #[serde(default)] pub(super) source: String,
}

// One operation grammar for function calls and shell commands. Legacy v1
// function rules are retained as a compatibility adapter.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct OperationRule {
    pub(super) name: String,
    pub(super) language: String,
    #[serde(default)] pub(super) subcommand: Option<String>,
    #[serde(default)] pub(super) arguments: Vec<OperationArgument>,
    #[allow(dead_code)] // Descriptive YAML metadata, not an import requirement.
    #[serde(default)] pub(super) source: String,
}
#[derive(Debug, Clone, Deserialize)]
pub(super) struct OperationArgument {
    #[serde(default)] pub(super) name: String,
    pub(super) role: String,
    #[serde(default)] pub(super) kind: Option<String>,
    #[serde(default)] pub(super) cardinality: Option<String>,
    #[serde(default)] pub(super) position: Option<serde_yaml::Value>,
    #[serde(default)] pub(super) through: Option<String>,
    #[allow(dead_code)] // Reserved YAML operation setting; do not silently discard it.
    #[serde(default)] pub(super) descendants: bool,
    #[serde(default)] pub(super) expected_files: Vec<String>,
    #[serde(default)] pub(super) default: Option<String>,
    #[serde(default)] pub(super) file_type: Option<ArtifactRole>,
    #[serde(default)] pub(super) descendant_file_type: Option<ArtifactRole>,
}

pub(super) fn load_operation_rules(root: &Path) -> Result<Vec<OperationRule>> {
    let mut combined = Vec::<OperationRule>::new();
    for layer in rule_layers(root)? {
        for rule in layer.operations {
            if rule.name.trim().is_empty() || !matches!(rule.language.as_str(), "bash" | "python" | "r") {
                anyhow::bail!("Invalid operation rule {}: language", rule.name);
            }
            for arg in &rule.arguments {
                if !arg.expected_files.is_empty() && (arg.role != "output" || arg.kind.as_deref() != Some("directory")) {
                    anyhow::bail!("expected_files requires an output directory in operation {}", rule.name);
                }
                if (arg.name.trim().is_empty() && arg.position.is_none())
                    || !matches!(arg.role.as_str(), "input" | "output" | "parameter")
                    || !matches!(arg.kind.as_deref().unwrap_or("file"), "file" | "directory" | "value")
                    || !matches!(arg.cardinality.as_deref().unwrap_or("one"), "one" | "many" | "zero")
                    || arg.position.as_ref().is_some_and(|p| p.as_u64() == Some(0)) {
                    anyhow::bail!("Invalid argument {} in operation {}", arg.name, rule.name);
                }
            }
            combined.retain(|r| !(r.name == rule.name && r.language == rule.language && r.subcommand == rule.subcommand));
            combined.push(rule);
        }
    }
    Ok(combined)
}

// Layered rule libraries: built-in < configured external < archive-local.
// Existing provenance_rules.yaml remains supported for compatibility.
pub(super) fn rule_layers(root: &Path) -> Result<Vec<RuleSet>> {
    let builtin: RuleSet = serde_yaml::from_str(include_str!("../../../rules/scientific_functions.yaml"))?;
    let defaults: RuleSet = serde_yaml::from_str(include_str!("../../../rules/file_types.yaml"))?;
    let mut layers = vec![builtin, defaults];
    // Per-user configuration is portable across projects and never edits built-ins.
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    if let Some(home) = home {
        let folder = Path::new(&home).join(".folder_history");
        for name in ["file_types.yaml", "operations.yaml"] {
            let path = folder.join(name);
            if path.is_file() { layers.push(read_rule_set(&path)?); }
        }
    }
    if let Some(configured) = std::env::var_os("FOLDER_HISTORY_RULES") {
        for path in std::env::split_paths(&configured) {
            if !path.is_file() {
                anyhow::bail!("Configured provenance rule file does not exist: {}", path.display());
            }
            layers.push(read_rule_set(&path)?);
        }
    }
    for name in ["provenance_rules.yaml", "extensions.yaml", "file_types.yaml", "operations.yaml"] {
        let path = root.join(".folder_history").join(name);
        if path.is_file() { layers.push(read_rule_set(&path)?); }
    }
    Ok(layers)
}

pub(super) fn read_rule_set(path: &Path) -> Result<RuleSet> {
    let rules: RuleSet = serde_yaml::from_str(&std::fs::read_to_string(path)?)
        .with_context(|| format!("Parsing provenance rules {}", path.display()))?;
    if rules.version != 1 && rules.version != 2 {
        anyhow::bail!("Unsupported provenance rules version in {}", path.display());
    }
    Ok(rules)
}

pub(super) fn load_rules(root: &Path) -> Result<Vec<FunctionRule>> {
    let mut combined: Vec<FunctionRule> = Vec::new();
    for layer in rule_layers(root)? {
        for rule in layer.functions {
            if rule.name.is_empty() || !matches!(rule.language.as_str(), "r" | "python")
                || rule.directory_arg == Some(0) || rule.output_arg == Some(0)
                || (rule.directory_arg.is_none() && rule.output_arg.is_none()) {
                anyhow::bail!("Invalid provenance function rule: {}", rule.name);
            }
            combined.retain(|r| !(r.name == rule.name && r.language == rule.language));
            combined.push(rule);
        }
    }
    Ok(combined)
}

pub(super) fn load_plot_rules(root: &Path) -> Result<Vec<PlotRule>> {
    let mut combined: Vec<PlotRule> = Vec::new();
    for layer in rule_layers(root)? {
        for plot in layer.plotting {
            if plot.module.is_empty() || plot.save_argument.is_empty()
                || plot.default_directory.is_empty() || plot.default_format.is_empty() {
                anyhow::bail!("Invalid plotting rule: {}", plot.module);
            }
            combined.retain(|p| p.module != plot.module);
            combined.push(plot);
        }
    }
    Ok(combined)
}

