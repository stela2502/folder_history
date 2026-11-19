//! Conservative, language-light provenance candidate extraction.
//! No script execution or dependency on imported helper implementations.
use super::*;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct RuleSet {
    pub(super) version: u32,
    pub(super) functions: Vec<FunctionRule>,
    #[serde(default)]
    pub(super) plotting: Vec<PlotRule>,
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

// Layered rule libraries: built-in < configured external < archive-local.
// Existing provenance_rules.yaml remains supported for compatibility.
pub(super) fn rule_layers(root: &Path) -> Result<Vec<RuleSet>> {
    let builtin: RuleSet = serde_yaml::from_str(include_str!("../../../rules/scientific_functions.yaml"))?;
    let mut layers = vec![builtin];
    if let Some(configured) = std::env::var_os("FOLDER_HISTORY_RULES") {
        for path in std::env::split_paths(&configured) {
            if !path.is_file() {
                anyhow::bail!("Configured provenance rule file does not exist: {}", path.display());
            }
            layers.push(read_rule_set(&path)?);
        }
    }
    for name in ["provenance_rules.yaml", "extensions.yaml"] {
        let path = root.join(".folder_history").join(name);
        if path.is_file() { layers.push(read_rule_set(&path)?); }
    }
    Ok(layers)
}

pub(super) fn read_rule_set(path: &Path) -> Result<RuleSet> {
    let rules: RuleSet = serde_yaml::from_str(&std::fs::read_to_string(path)?)
        .with_context(|| format!("Parsing provenance rules {}", path.display()))?;
    if rules.version != 1 {
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

