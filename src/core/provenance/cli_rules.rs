use super::*;

// A command rule describes argument semantics; the scanner does not execute it.
// A directory output links to its inventoried descendants via writes_directory.
pub(super) fn apply_shell_operation(
    db: &Connection, rel: &str, line: usize, tokens: &[String],
    rules: &[OperationRule], env: &Environment, paths: &HashSet<String>, cwd: Option<&str>,
) -> Result<usize> {
    let Some(program) = tokens.first() else { return Ok(0) };
    let program = program.rsplit('/').next().unwrap_or(program);
    let mut count = 0;
    for rule in rules.iter().filter(|r| r.language == "bash" && r.name == program) {
        let offset = if let Some(sub) = &rule.subcommand {
            if tokens.get(1) != Some(sub) { continue; }
            2
        } else { 1 };
        let args = &tokens[offset..];
        let mut named = HashMap::<String,Vec<String>>::new();
        let mut positional = Vec::<String>::new();
        let mut i = 0;
        while i < args.len() {
            let token = &args[i];
            if token.starts_with('-') && token != "-" {
                let (flag, inline) = token.split_once('=').map_or((token.as_str(),None), |(f,v)| (f,Some(v)));
                if let Some(spec) = rule.arguments.iter().find(|a| a.name == flag) {
                    let mut values = Vec::new();
                    if let Some(v) = inline { values.push(v.to_owned()); }
                    else if spec.cardinality.as_deref() != Some("zero") {
                        while i+1 < args.len() && (!args[i+1].starts_with('-') || args[i+1] == "-") {
                            i += 1;
                            values.push(args[i].clone());
                            if spec.cardinality.as_deref() != Some("many") { break; }
                        }
                    }
                    named.entry(flag.to_owned()).or_default().extend(values);
                }
                // Unknown flags are not assumed to consume a value; positional
                // matches are conservative and must be checked against inventory.
            } else { positional.push(token.clone()); }
            i += 1;
        }
        for spec in &rule.arguments {
            let values = if let Some((start,end)) = positional_bounds(spec,positional.len()) {
                positional[start..end].to_vec()
            } else { named.get(&spec.name).cloned().unwrap_or_default() };
            let values = if values.is_empty() { spec.default.clone().into_iter().collect() } else { values };
            count += record_operation_values(db,rel,line,rule,spec,values,env,paths,cwd)?;
        }
    }
    Ok(count)
}

// Unified argument selection used by R/Python calls and shell invocations.
pub(super) fn positional_bounds(spec: &OperationArgument, len: usize) -> Option<(usize, usize)> {
    let position = spec.position.as_ref()?;
    let first = if position.as_str() == Some("last") { len.checked_sub(1)? }
        else { usize::try_from(position.as_u64()?).ok()?.checked_sub(1)? };
    if first >= len { return None; }
    let last = if spec.through.as_deref() == Some("last") { len } else { first + 1 };
    Some((first, last))
}

pub(super) fn operation_name_matches(rule: &str, called: &str, imports: &HashMap<String,String>, language: &str) -> bool {
    if rule == called { return true; }
    if language == "python" {
        let canonical_name = canonical(called, imports);
        if canonical_name == rule { return true; }
        // Instance methods (adata.write_h5ad) have no import alias.
        if !rule.contains('.') && called.rsplit('.').next() == Some(rule) { return true; }
    }
    false
}

// All operation arguments become symbolic links and enter the SAME inventory
// matcher as the legacy Python/R logic modules. No second path resolver.
fn record_operation_values(db: &Connection, rel: &str, line: usize, rule: &OperationRule,
    spec: &OperationArgument, values: Vec<String>, env: &Environment,
    paths: &HashSet<String>, cwd: Option<&str>) -> Result<usize> {
    if spec.role == "parameter" || spec.kind.as_deref() == Some("value") { return Ok(0); }
    let mut links = Vec::new();
    for value in values {
        if value.is_empty() || value == "-" { continue; }
        let mut variable = if rule.language == "bash" {
            if value.contains('$') { shell_expand(&value, env) } else { Variable::Exact(value.clone()) }
        } else {
            Variable::from_script_slice(&value, env, &rule.language)
        };
        if variable == Variable::Unknown { continue; }
        // A declared working directory is a candidate anchor, not a reason to
        // discard the original path. Preserve the symbolic value for relocation.
        if let (Some(dir), Variable::Exact(path)) = (cwd, &variable) {
            if !path.starts_with('/') && !path.starts_with('~') {
                let joined = format!("{}/{}", dir.trim_end_matches('/'), path);
                let test = FilePath::new(vec![PathPart::Literal(joined.clone())]);
                if matches!(test.find_matches(rel, paths), FileMatches::Strict(_)) {
                    variable = Variable::Exact(joined);
                }
            }
        }
        let kind = if spec.role == "output" { LinkType::Writes } else { LinkType::Reads };
        links.push(LogicLinks {
            kind,
            links: LinksToFiles { files: vec![variable] },
            expression: value,
            function: format!("{}{} [{}]", rule.name,
                rule.subcommand.as_ref().map(|s| format!(" {s}")).unwrap_or_default(), spec.name),
        });
    }
    // A constrained directory output must NOT record writes_directory: that
    // would implicitly attribute every descendant to the operation. Instead
    // resolve the directory using the shared FilePath matcher and record only
    // inventoried children matching expected_files (literal or symbolic).
    if spec.kind.as_deref() == Some("directory") && !spec.expected_files.is_empty() {
        let mut inventory = paths.clone();
        for path in paths {
            let mut end = path.rfind('/');
            while let Some(index) = end {
                inventory.insert(path[..index].to_owned());
                end = path[..index].rfind('/');
            }
        }
        let mut selected = Vec::new();
        for link in &links {
            for variable in &link.links.files {
                let directory = variable.clone().into_filepath();
                let matches = match directory.find_matches(rel, &inventory) {
                    FileMatches::Strict(found) | FileMatches::Funky(found) => found,
                    FileMatches::None => Vec::new(),
                };
                for base in matches {
                    if !paths.iter().any(|p| p.starts_with(&format!("{base}/"))) { continue; }
                    for pattern in &spec.expected_files {
                        let parts = pattern.split('*').enumerate().flat_map(|(i, part)| {
                            let mut result = Vec::new();
                            if i > 0 { result.push(PathPart::Wildcard); }
                            if !part.is_empty() { result.push(PathPart::Literal(part.to_owned())); }
                            result
                        }).collect();
                        let filename = FilePath::new(parts);
                        let matching: HashSet<String> = paths.iter().filter(|path| {
                            path.strip_prefix(&format!("{base}/"))
                                .is_some_and(|suffix| filename.matches(suffix))
                        }).cloned().collect();
                        // Exact target inventory prevents unrelated fuzzy matches;
                        // the normal shared recorder still creates the evidence.
                        for target in matching {
                            selected.push(LogicLinks {
                                kind: LinkType::Writes,
                                links: LinksToFiles { files: vec![Variable::Exact(target.clone())] },
                                expression: format!("{base}/{pattern}"),
                                function: format!("{} [{}]", rule.name, spec.name),
                            });
                        }
                    }
                }
            }
        }
        return record_logic_links(db, rel, line, &selected, paths);
    }
    // Directory paths may not be explicit inventory entries; derive them from
    // existing descendants and feed them through the SAME shared matcher.
    if spec.kind.as_deref() == Some("directory") {
        let mut inventory = paths.clone();
        for path in paths {
            let mut end = path.rfind('/');
            while let Some(index) = end {
                inventory.insert(path[..index].to_owned());
                end = path[..index].rfind('/');
            }
        }
        record_logic_links(db, rel, line, &links, &inventory)
    } else {
        record_logic_links(db, rel, line, &links, paths)
    }
}

pub(super) fn apply_function_operation(db: &Connection, rel: &str, line: usize, args: &str,
    rule: &OperationRule, env: &Environment, paths: &HashSet<String>) -> Result<usize> {
    let mut named = HashMap::<String,String>::new();
    let mut positional = Vec::<String>::new();
    for arg in split_args(args) {
        if let Some((key, value)) = arg.split_once('=') {
            if !key.trim().is_empty() && key.trim().chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
                named.insert(key.trim().to_owned(), value.trim().to_owned());
                continue;
            }
        }
        positional.push(arg);
    }
    let mut count = 0;
    for spec in &rule.arguments {
        let values = if let Some(expr) = named.get(&spec.name) {
            vec![expr.clone()]
        } else if let Some((start,end)) = positional_bounds(spec, positional.len()) {
            positional[start..end].to_vec()
        } else { Vec::new() };
        // YAML defaults are literal paths, not Python/R expressions.
        let values = if values.is_empty() {
            spec.default.as_ref().map(|value| vec![format!("{value:?}")]).unwrap_or_default()
        } else { values };
        count += record_operation_values(db,rel,line,rule,spec,values,env,paths,None)?;
    }
    Ok(count)
}

// Shell syntax extraction, independent of application YAML. Operators are
// recognized even without surrounding whitespace, but never inside quotes.
pub(super) fn shell_pipeline(line: &str) -> (Vec<Vec<String>>, Vec<(String,String)>) {
    let mut segments = Vec::new();
    let mut current = Vec::new();
    let mut redirects = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if escaped { word.push(c); escaped=false; continue; }
        if c == '\\' && quote != Some('\'') { escaped=true; continue; }
        if let Some(q) = quote {
            if c == q { quote=None; } else { word.push(c); }
            continue;
        }
        if c == '\'' || c == '"' { quote=Some(c); continue; }
        if c == '#' && word.is_empty() { break; }
        if c.is_whitespace() {
            if !word.is_empty() { current.push(std::mem::take(&mut word)); }
            continue;
        }
        if c == '|' && chars.peek() != Some(&'|') {
            if !word.is_empty() { current.push(std::mem::take(&mut word)); }
            if !current.is_empty() { segments.push(std::mem::take(&mut current)); }
            continue;
        }
        if c == '<' || c == '>' {
            let fd = if word == "1" || word == "2" { std::mem::take(&mut word) } else { String::new() };
            if !word.is_empty() { current.push(std::mem::take(&mut word)); }
            let mut op = format!("{fd}{c}");
            if c == '>' && chars.peek() == Some(&'>') { chars.next(); op.push('>'); }
            while chars.peek().is_some_and(|x| x.is_whitespace()) { chars.next(); }
            let mut filename = String::new();
            let mut fq = None;
            while let Some(&next) = chars.peek() {
                if let Some(q) = fq {
                    chars.next();
                    if next == q { fq=None; } else { filename.push(next); }
                } else if next == '\'' || next == '"' { fq=Some(next); chars.next(); }
                else if next.is_whitespace() || next == '|' || next == '<' || next == '>' { break; }
                else { filename.push(next); chars.next(); }
            }
            if !filename.is_empty() { redirects.push((op,filename)); }
            continue;
        }
        word.push(c);
    }
    if !word.is_empty() { current.push(word); }
    if !current.is_empty() { segments.push(current); }
    (segments, redirects)
}
