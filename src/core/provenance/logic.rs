use super::*;
// Symbolic output of a logic module. Inventory resolution happens later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LinkType { Reads, Writes, References }
impl LinkType {
    pub(super) fn as_str(self) -> &'static str {
        match self { Self::Reads => "reads", Self::Writes => "writes", Self::References => "references" }
    }
}
#[derive(Clone, Debug)]
pub(super) struct LinksToFiles { pub(super) files: Vec<Variable> }
#[derive(Clone, Debug)]
pub(super) struct LogicLinks { pub(super) kind: LinkType, pub(super) links: LinksToFiles, pub(super) expression: String, pub(super) function: String }

pub(super) trait LogicModule {
    fn has_function(&self, name: &str, language: &str) -> bool;
    fn apply(&self, name: &str, args: &str, env: &Environment, language: &str) -> Vec<LogicLinks>;
}

// Declarative function rules use exactly the same symbolic result as Rust modules.
pub(super) struct YamlLogicModule<'a> { pub(super) rule: &'a FunctionRule }
impl<'a> YamlLogicModule<'a> {
    pub(super) fn from_yaml(rule: &'a FunctionRule) -> Self { Self { rule } }
}
impl LogicModule for YamlLogicModule<'_> {
    fn has_function(&self, name: &str, language: &str) -> bool {
        self.rule.language == language && self.rule.name == name.rsplit('.').next().unwrap_or(name)
    }
    fn apply(&self, name: &str, args: &str, env: &Environment, language: &str) -> Vec<LogicLinks> {
        let parts = split_args(args);
        let mut out = Vec::new();
        for (index, keys) in [
            (self.rule.output_arg, &["output_file", "out_csv", "ofile", "file", "filename"][..]),
            (self.rule.directory_arg, &["path", "output_dir", "outpath", "directory"][..]),
        ] {
            let Some(index) = index else { continue };
            let expr = parts.iter().find_map(|part| {
                let (key, value) = part.split_once('=')?;
                keys.contains(&key.trim()).then(|| value.trim().to_owned())
            }).or_else(|| parts.iter().filter(|p| !p.contains('=')).nth(index - 1).cloned());
            if let Some(expr) = expr {
                let variable = Variable::from_script_slice(&expr, env, language);
                if variable != Variable::Unknown {
                    out.push(LogicLinks { kind: LinkType::Writes, links: LinksToFiles { files: vec![variable] }, expression: expr, function: name.to_owned() });
                }
            }
        }
        out
    }
}

pub(super) struct GenericFunctionModule;
impl LogicModule for GenericFunctionModule {
    fn has_function(&self, _name: &str, _language: &str) -> bool { true }
    fn apply(&self, name: &str, args: &str, env: &Environment, language: &str) -> Vec<LogicLinks> {
        let mut result = Vec::new();
        let inferred = direction(name);
        let kind = match inferred {
            "reads" => LinkType::Reads, "writes" => LinkType::Writes,
            _ => LinkType::References,
        };
        // Unknown calls are references, never automatically writes.
        // R graphics devices are explicit output producers.
        let kind = if language == "r" && matches!(name.rsplit("::").next().unwrap_or(name),
            "png" | "jpeg" | "jpg" | "tiff" | "bmp" | "svg" | "pdf" | "postscript") {
            LinkType::Writes
        } else { kind };
        for (expr, _) in filename_args(name, args, env) {
            let variable = Variable::from_script_slice(&expr, env, language);
            if variable != Variable::Unknown {
                result.push(LogicLinks { kind, links: LinksToFiles { files: vec![variable] }, expression: expr, function: name.to_owned() });
            }
        }
        // Symbolic arguments must pass through the same matching stage.
        if result.is_empty() {
            if let Some(expr) = file_expression(args, name) {
                let variable = Variable::from_script_slice(&expr, env, language);
                if variable != Variable::Unknown {
                    result.push(LogicLinks { kind, links: LinksToFiles { files: vec![variable] }, expression: expr, function: name.to_owned() });
                }
            }
        }
        result
    }
}

// Each matcher recognizes one expression. A Match can carry several symbolic
// file candidates (e.g. a function with multiple output arguments).
#[derive(Debug)]
pub(super) struct Match {
    pub(super) links: Vec<LogicLinks>,
}

pub(super) trait LinkMatcher {
    fn matches(&self, expression: &str, context: &Environment) -> Option<Match>;
}

fn call_parts(expression: &str) -> Option<(&str, &str)> {
    let (name, args) = expression.split_once('(')?;
    let args = args.strip_suffix(')')?;
    Some((name, args))
}

struct RuleMatcher<'a> {
    module: YamlLogicModule<'a>,
    language: &'a str,
}

impl LinkMatcher for RuleMatcher<'_> {
    fn matches(&self, expression: &str, context: &Environment) -> Option<Match> {
        let (name, args) = call_parts(expression)?;
        if !self.module.has_function(name, self.language) { return None; }
        let links = self.module.apply(name, args, context, self.language);
        (!links.is_empty()).then_some(Match { links })
    }
}

struct GenericMatcher<'a> {
    language: &'a str,
}

impl LinkMatcher for GenericMatcher<'_> {
    fn matches(&self, expression: &str, context: &Environment) -> Option<Match> {
        let (name, args) = call_parts(expression)?;
        let module = GenericFunctionModule;
        if !module.has_function(name, self.language) { return None; }
        let links = module.apply(name, args, context, self.language);
        (!links.is_empty()).then_some(Match { links })
    }
}

// Registry owns the matchers for one analysis, and runs them in order.
// Existing YAML and generic inference logic is preserved behind the interface.
pub(super) struct LinkerRegistry<'a> {
    matchers: Vec<Box<dyn LinkMatcher + 'a>>,
}

impl<'a> LinkerRegistry<'a> {
    pub(super) fn new(rules: &'a [FunctionRule], language: &'a str) -> Self {
        let mut matchers: Vec<Box<dyn LinkMatcher + 'a>> = rules.iter()
            .map(|rule| Box::new(RuleMatcher {
                module: YamlLogicModule::from_yaml(rule),
                language,
            }) as Box<dyn LinkMatcher + 'a>)
            .collect();
        matchers.push(Box::new(GenericMatcher { language }));
        Self { matchers }
    }

    pub(super) fn matches(&self, expression: &str, context: &Environment) -> Vec<LogicLinks> {
        self.matchers.iter()
            .filter_map(|matcher| matcher.matches(expression, context))
            .flat_map(|matched| matched.links)
            .collect()
    }
}

// Compatibility entry point for existing unit tests and callers.
// The analyzer itself keeps one registry for the entire source.
#[cfg(test)]
pub(super) fn logic_links(
    name: &str, args: &str, env: &Environment,
    rules: &[FunctionRule], language: &str,
) -> Vec<LogicLinks> {
    LinkerRegistry::new(rules, language).matches(&format!("{name}({args})"), env)
}

pub(super) fn record_logic_links(db: &Connection, rel: &str, line: usize, results: &[LogicLinks], paths: &HashSet<String>) -> Result<usize> {
    let mut count = 0;
    for result in results {
        for variable in &result.links.files {
            let (expression, candidates): (String, Vec<String>) = match variable {
                Variable::Exact(path) => {
                    let (resolved, _, _) = resolve_reference(path, rel, paths);
                    let found = if let Some(target) = resolved { vec![target] } else {
                        let matcher = FilePath::new(vec![PathPart::Literal(path.clone())]);
                        match matcher.find_matches(rel, paths) {
                            FileMatches::Strict(found) | FileMatches::Funky(found) => found,
                            FileMatches::None => Vec::new(),
                        }
                    };
                    (path.clone(), found)
                }
                Variable::Parts(path) => {
                    let expression = path.parts.iter().map(|part| match part {
                        PathPart::Literal(s) => s.clone(),
                        PathPart::Wildcard => "*".to_owned(),
                    }).collect::<String>();
                    // A fully expanded shell variable is a concrete path, not a
                    // fuzzy expression. Resolve it against the source directory
                    // using the same inventory-aware resolver as literal links.
                    // This also handles `./` and trailing directory separators.
                    if !path.contains_wildcards() {
                        let (resolved, _, _) = resolve_reference(&expression, rel, paths);
                        (expression, resolved.into_iter().collect())
                    } else {
                        let fragments = path.ordered_fragments();
                        let wildcard_filename = path.parts.iter().any(|part| matches!(part, PathPart::Literal(s) if s.contains('.')));
                        if !wildcard_filename && (fragments.is_empty() || (fragments.len() == 1 && fragments[0].len() < 12)) { continue; }
                        let found = match path.find_matches(rel, paths) {
                            FileMatches::Strict(found) | FileMatches::Funky(found) => found,
                            FileMatches::None => Vec::new(),
                        };
                        (expression, found)
                    }
                }
                Variable::Unknown => continue,
            };
            if candidates.len() > 30 { continue; }
            for target in candidates {
                // Empty directories have no descendants; the scanner indexes
                // them explicitly as directory artifacts before parsing scripts.
                let indexed_directory = db.query_row(
                    "SELECT kind='directory' FROM artifacts WHERE path=?1",
                    [&target], |row| row.get::<_, bool>(0),
                ).unwrap_or(false);
                let is_dir = indexed_directory || paths.iter().any(|p| p.starts_with(&format!("{target}/")));
                let kind = if is_dir && result.kind == LinkType::Writes { "writes_directory" } else { result.kind.as_str() };
                if kind == "references" { continue; }
                let evidence = format!("{rel}: cell/line {line}: {} via {}({})", if matches!(variable, Variable::Parts(_)) { "inferred" } else { "explicit" }, result.function, result.expression);
                super::super::history_db::DbProvenance::record_match(db, rel, line, &expression, &target, kind, &evidence)?;
                count += 1;
            }
        }
    }
    Ok(count)
}

