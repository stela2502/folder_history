//! Conservative, language-light provenance candidate extraction.
//! No script execution or dependency on imported helper implementations.
use super::*;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct RuleSet {
    version: u32,
    functions: Vec<FunctionRule>,
    #[serde(default)]
    plotting: Vec<PlotRule>,
}
#[derive(Debug, Clone, Deserialize)]
struct PlotRule {
    module: String,
    save_argument: String,
    directory_setting: String,
    default_directory: String,
    format_setting: String,
    default_format: String,
    #[serde(default)]
    prefix_overrides: HashMap<String, String>,
}
#[derive(Debug, Deserialize)]
struct FunctionRule {
    name: String,
    language: String,
    directory_arg: Option<usize>,
    output_arg: Option<usize>,
    default_output: Option<String>,
    #[serde(default)] files: Vec<String>,
    #[serde(default)] patterns: Vec<String>,
    #[serde(default)] source: String,
}

// Layered rule libraries: built-in < configured external < archive-local.
// Existing provenance_rules.yaml remains supported for compatibility.
fn rule_layers(root: &Path) -> Result<Vec<RuleSet>> {
    let builtin: RuleSet = serde_yaml::from_str(include_str!("../../rules/scientific_functions.yaml"))?;
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

fn read_rule_set(path: &Path) -> Result<RuleSet> {
    let rules: RuleSet = serde_yaml::from_str(&std::fs::read_to_string(path)?)
        .with_context(|| format!("Parsing provenance rules {}", path.display()))?;
    if rules.version != 1 {
        anyhow::bail!("Unsupported provenance rules version in {}", path.display());
    }
    Ok(rules)
}

fn load_rules(root: &Path) -> Result<Vec<FunctionRule>> {
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

fn load_plot_rules(root: &Path) -> Result<Vec<PlotRule>> {
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

fn rule_output(db: &Connection, rel: &str, line: usize, candidate: &str, rule: &FunctionRule,
               paths: &HashSet<String>) -> Result<usize> {
    let (resolved, external, ambiguous) = resolve_reference(candidate, rel, paths);
    if external {
        record_issue(db, rel, line, &reference_name(candidate), "external")?;
        return Ok(0);
    }
    if ambiguous {
        record_issue(db, rel, line, &reference_name(candidate), "ambiguous")?;
        return Ok(0);
    }
    if let Some(target) = resolved {
        let evidence = format!("{}: cell/line {}: function rule {} ({})", rel, line, rule.name, rule.source);
        let is_directory = paths.iter().any(|p| p.starts_with(&format!("{target}/")));
        add_relation(db, rel, &target, if is_directory {"writes_directory"} else {"writes"}, &evidence, false)?;
        return Ok(1);
    }
    Ok(0)
}

fn apply_rule(db: &Connection, rel: &str, line: usize, args: &str, rule: &FunctionRule,
              vars: &HashMap<String,String>, paths: &HashSet<String>) -> Result<usize> {
    let arguments = split_args(args);
    let arg_value = |n: usize, names: &[&str]| -> Option<String> {
        if n == 0 { return None; }
        // Named arguments may appear in any position, including R's path='data'.
        for arg in &arguments {
            if let Some((key, expr)) = arg.split_once('=') {
                if names.contains(&key.trim()) { return value(expr, vars); }
            }
        }
        // Positional arguments exclude named arguments.
        arguments.iter().filter(|a| !a.contains('=')).nth(n-1).and_then(|a| value(a, vars))
    };
    let mut count = 0;
    if let Some(n) = rule.output_arg {
        if let Some(target) = arg_value(n, &["output_file", "out_csv", "ofile", "file", "filename"]).or_else(|| {
            if arguments.len() < n { rule.default_output.clone() } else { None }
        }) {
            count += rule_output(db, rel, line, &target, rule, paths)?;
        }
    }
    if let Some(n) = rule.directory_arg {
        if let Some(dir) = arg_value(n, &["path", "output_dir", "outpath", "directory"]) {
            // The directory itself is a useful edge even if no child matches.
            count += rule_output(db, rel, line, &dir, rule, paths)?;
            let (resolved, external, ambiguous) = resolve_reference(&dir, rel, paths);
            if !external && !ambiguous {
                if let Some(base) = resolved {
                    for filename in &rule.files {
                        count += rule_output(db, rel, line, &format!("{base}/{filename}"), rule, paths)?;
                    }
                    for pattern in &rule.patterns {
                        if let Some(suffix) = pattern.strip_prefix('*') {
                            let prefix = format!("{base}/");
                            for path in paths {
                                if let Some(rest) = path.strip_prefix(&prefix) {
                                    if !rest.contains('/') && rest.ends_with(suffix) {
                                        count += rule_output(db, rel, line, path, rule, paths)?;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    } else {
        for filename in &rule.files {
            count += rule_output(db, rel, line, filename, rule, paths)?;
        }
    }
    Ok(count)
}


fn direction(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    let tokens: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric()).collect();
    if tokens.iter().any(|t| ["write", "save", "export", "dump", "output", "outfile", "ofile", "to"].contains(t))
        || ["write", "save", "export", "dump"].iter().any(|p| lower.starts_with(p))
        || lower.starts_with("to_") { "writes" }
    else if tokens.iter().any(|t| ["read", "load", "scan", "source", "input", "ifile", "from"].contains(t))
        || ["read", "load", "scan", "source"].iter().any(|p| lower.starts_with(p)) { "reads" }
    else { "references" }
}
fn split_args(s: &str) -> Vec<String> {
    let mut out=Vec::new(); let mut start=0; let mut depth=0i32; let mut quote=None; let mut escaped=false;
    for (i,c) in s.char_indices() {
        if escaped { escaped=false; continue; }
        if c=='\\' && quote.is_some() { escaped=true; continue; }
        if let Some(q)=quote { if c==q {quote=None;} continue; }
        if c=='\'' || c=='"' {quote=Some(c);continue;}
        if c=='(' || c=='[' || c=='{' {depth+=1;} else if c==')'||c==']'||c=='}' {depth-=1;}
        if c==',' && depth==0 {out.push(s[start..i].trim().to_string());start=i+1;}
    }
    out.push(s[start..].trim().to_string()); out
}
fn value(expr: &str, vars: &HashMap<String,String>) -> Option<String> {
    let s=expr.trim();
    if let Some(v)=vars.get(s) {return Some(v.clone());}
    if s.len()>=2 && ((s.starts_with('"')&&s.ends_with('"'))||(s.starts_with('\'')&&s.ends_with('\''))) {
        return Some(s[1..s.len()-1].to_string());
    }
    if (s.starts_with("f\"") && s.ends_with('"')) || (s.starts_with("f'") && s.ends_with('\'')) {
        if s.len() < 3 { return None; }
        let mut result=s[2..s.len()-1].to_string();
        for (k,v) in vars {result=result.replace(&format!("{{{k}}}"),v);}
        if !result.contains('{') {return Some(result);} return None;
    }
    for prefix in ["os.path.join(","file.path(","Path(","pathlib.Path("] {
        if let Some(rest)=s.strip_prefix(prefix).and_then(|x|x.strip_suffix(')')) {
            let parts=split_args(rest); let vals=parts.iter().map(|p|value(p,vars)).collect::<Option<Vec<_>>>()?;
            if prefix.ends_with("Path(") {return vals.into_iter().next();}
            let mut path=std::path::PathBuf::new(); for v in vals {path.push(v);} return Some(normalized(&path));
        }
    }
    if s.contains(" / ") || s.contains(" + ") {
        let separator=if s.contains(" / ") {" / "} else {" + "};
        let vals=s.split(separator).map(|x|value(x,vars)).collect::<Option<Vec<_>>>()?;
        return Some(if separator==" / " {vals.join("/")} else {vals.concat()});
    }
    None
}
fn match_candidate(db:&Connection, rel:&str, line:usize, expr:&str, label:&str, vars:&HashMap<String,String>, paths:&HashSet<String>, malformed:bool) -> Result<usize> {
    let Some(v)=value(expr,vars) else {return Ok(0)};
    let (resolved,external,ambiguous)=resolve_reference(&v,rel,paths);
    let filename=reference_name(&v);
    if external {record_issue(db,rel,line,&filename,"external")?;}
    if ambiguous {record_issue(db,rel,line,&filename,"ambiguous")?;}
    if let Some(target)=resolved {
        let mut kind=direction(label);
        // The path variable may convey direction even if the called function is unfamiliar.
        if kind=="references" {kind=direction(expr);}
        let directory = paths.iter().any(|p| p.starts_with(&format!("{target}/")));
        if directory && kind=="writes" { kind="writes_directory"; }
        let evidence = if malformed {
            format!("{}: line/cell {}: uncertain/malformed source candidate via {}",rel,line,label)
        } else {
            format!("{}: line/cell {}: candidate via {}",rel,line,label)
        };
        add_relation(db,rel,&target,kind,&evidence,false)?;
        return Ok(1);
    }
    if !external && !ambiguous && !filename.is_empty() {record_issue(db,rel,line,&filename,"unresolved")?;}
    Ok(0)
}
// A small lexical pass: find file operations without requiring balanced calls.
// Deliberately recover at a new operation even inside an unfinished call.
fn file_operation(label: &str) -> bool {
    let lower=label.to_ascii_lowercase();
    direction(&lower)!="references" || lower.ends_with("to_csv") || lower.ends_with("to_parquet")
}
//fn is_identifier(c:char)->bool {c.is_ascii_alphanumeric() || c=='_' || c=='.'}
// Resolve Python import aliases to canonical module/function paths.
fn python_imports(source: &str) -> HashMap<String, String> {
    let mut aliases = HashMap::new();
    for line in source.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if let Some(rest) = line.strip_prefix("import ") {
            for entry in rest.split(',') {
                let words: Vec<_> = entry.split_whitespace().collect();
                if let Some(module) = words.first() {
                    if words.len() == 1 {
                        let root = module.split('.').next().unwrap_or(module);
                        aliases.insert(root.to_string(), root.to_string());
                    } else if words.len() == 3 && words[1] == "as" {
                        aliases.insert(words[2].to_string(), (*module).to_string());
                    }
                }
            }
        } else if let Some(rest) = line.strip_prefix("from ") {
            if let Some((module, names)) = rest.split_once(" import ") {
                for entry in names.split(',') {
                    let words: Vec<_> = entry.split_whitespace().collect();
                    if let Some(item) = words.first() {
                        if *item == "*" { continue; }
                        let alias = if words.len() == 3 && words[1] == "as" { words[2] } else { item };
                        aliases.insert(alias.to_string(), format!("{module}.{item}"));
                    }
                }
            }
        }
    }
    aliases
}
fn canonical(label: &str, imports: &HashMap<String, String>) -> String {
    let (head, rest) = label.split_once('.').unwrap_or((label, ""));
    match imports.get(head) {
        Some(module) if rest.is_empty() => module.clone(),
        Some(module) => format!("{module}.{rest}"),
        None => label.to_string(),
    }
}
fn plot_outputs(
    db: &Connection, rel: &str, line: usize, canonical_call: &str,
    args: &str, vars: &HashMap<String, String>, paths: &HashSet<String>,
    plots: &[PlotRule],
) -> Result<usize> {
    let mut count = 0;
    for rule in plots {
        let Some(function) = canonical_call.strip_prefix(&(rule.module.clone() + ".")) else { continue };
        if function.contains('.') { continue; }
        let Some(save_expr) = split_args(args).into_iter().find_map(|arg| {
            let (key, val) = arg.split_once('=')?;
            (key.trim() == rule.save_argument).then(|| val.trim().to_string())
        }) else { continue };
        if save_expr == "False" || save_expr == "None" { continue; }
        let suffix = if save_expr == "True" {
            String::new()
        } else if let Some(v) = value(&save_expr, vars) {
            if v.is_empty() { continue; }
            v
        } else { continue };
        let directory = vars.get(&rule.directory_setting)
            .cloned().unwrap_or_else(|| rule.default_directory.clone());
        let format = vars.get(&rule.format_setting)
            .cloned().unwrap_or_else(|| rule.default_format.clone());
        let prefix = rule.prefix_overrides.get(function).map(String::as_str).unwrap_or(function);
        let suffix = if suffix.rsplit('/').next().unwrap_or("").contains('.') {
            suffix
        } else {
            format!("{suffix}.{}", format.trim_start_matches('.'))
        };
        let candidate = format!("{directory}/{prefix}{suffix}");
        let (resolved, external, ambiguous) = resolve_reference(&candidate, rel, paths);
        if !external && !ambiguous {
            if let Some(target) = resolved {
                let evidence = format!("{rel}: line/cell {line}: inferred {} save output ({canonical_call})", rule.module);
                add_relation(db, rel, &target, "writes", &evidence, false)?;
                count += 1;
            }
        }
    }
    Ok(count)
}

fn operations(source:&str, rules:&[FunctionRule], imports:&HashMap<String,String>, plots:&[PlotRule], language:&str)->Vec<(usize,String,usize)> {
    let bytes=source.as_bytes(); let mut found=Vec::new();
    let mut i=0; let mut quote:Option<u8>=None; let mut escaped=false; let mut comment=false;
    while i<bytes.len() {
        let c=bytes[i];
        if c==b'\n' {comment=false; if quote.is_some() {quote=None;} i+=1;continue;}
        if comment {i+=1;continue;}
        if let Some(q)=quote {
            if escaped {escaped=false;} else if c==b'\\' {escaped=true;} else if c==q {quote=None;}
            i+=1;continue;
        }
        if c==b'#' {comment=true;i+=1;continue;}
        if c==b'\'' || c==b'"' {quote=Some(c);i+=1;continue;}
        if c.is_ascii_alphabetic() || c==b'_' {
            let start=i;i+=1;
            while i<bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i]==b'_' || bytes[i]==b'.') {i+=1;}
            let label=&source[start..i];
            let mut open=i;while open<bytes.len() && bytes[open].is_ascii_whitespace(){open+=1;}
            if open<bytes.len() && bytes[open]==b'(' && (file_operation(label) || rules.iter().any(|r| r.language == language && r.name == (if language == "r" {label} else {label.rsplit('.').next().unwrap_or(label)})) || plots.iter().any(|p| canonical(label, imports).starts_with(&(p.module.clone()+".")))) {
                let line=source[..start].bytes().filter(|b|*b==b'\n').count()+1;
                found.push((line,label.to_owned(),open+1));
            }
            continue;
        }
        i+=1;
    }
    found
}
fn argument_end(s:&str)->(usize,bool) {
    let mut stack=vec!['('];let mut quote=None;let mut escaped=false;let mut comment=false;
    for (i,c) in s.char_indices() {
        if c=='\n' {comment=false;}
        if comment {continue;}
        if let Some(q)=quote {
            if escaped {escaped=false;} else if c=='\\' {escaped=true;} else if c==q {quote=None;}
            continue;
        }
        if c=='#' {comment=true;continue;}
        if c=='\''||c=='"' {quote=Some(c);continue;}
        match c {
            '('|'['|'{' => stack.push(c),
            ')'|']'|'}' => {
                if let Some(last)=stack.last() {
                    if matches!((*last,c),('(',')')|('[',']')|('{','}')) {
                        stack.pop();if stack.is_empty(){return(i,true)}
                    }
                }
            },_=>{}
        }
    }
    (s.len(),false)
}
fn filename_args(label:&str, s:&str, vars:&HashMap<String,String>)->Vec<(String,String)> {
    let mut found=Vec::new();
    let args=split_args(s);
    let lower=label.to_ascii_lowercase();
    let mut first_positional=true;
    for arg in args {
        let arg=arg.trim();if arg.is_empty(){continue;}
        let kv=arg.split_once('=').filter(|(k,_)| k.trim().chars().all(|c|c.is_ascii_alphanumeric()||c=='_'));
        let (key,expr)=if let Some((k,v))=kv {(Some(k.trim()),v.trim())} else {(None,arg)};
        let name=key.unwrap_or("").to_ascii_lowercase();
        let known=matches!(name.as_str(),"file"|"filename"|"filepath"|"path"|"output"|"outfile"|"input"|"input_file"|"output_file"|"fname");
        // For write.csv(df, file=...) and to_csv(path), the first positional
        // argument has different meanings. R writeH5AD(sce, filename) is similar.
        let positional_ok=first_positional && !(lower.ends_with("write.csv")||lower.ends_with("write.table")||lower.ends_with("writeh5ad") );
        let second_positional= !first_positional && (lower.ends_with("write.csv")||lower.ends_with("write.table")||lower.ends_with("writeh5ad"));
        if (known || (key.is_none() && (positional_ok || second_positional))) && value(expr,vars).is_some() {
            found.push((expr.to_owned(),if known && direction(&name)!="references" {key.unwrap().to_owned()} else {label.to_owned()}));
        }
        // Scientific helper writers commonly take (data, output_directory).
        // Accept a second positional argument only when it resolves to an
        // existing directory, never an arbitrary string from options.
        if key.is_none() && !first_positional && direction(label)=="writes" {
            if let Some(path)=value(expr,vars) {
                if path.len()>0 && !path.contains('.') && !path.contains(' ') {
                    found.push((expr.to_owned(),label.to_owned()));
                }
            }
        }
        if key.is_none() {first_positional=false;}
    }
    found
}
// A deliberately language-neutral fallback: literal fragments in a filename
// expression must occur in order in an inventoried basename. No evaluation of
// paste(), f-strings, loops, separators, or scientific functions.
fn ordered_literals(expr: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut chars = expr.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\'' && c != '"' { continue; }
        let mut literal = String::new();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                if let Some(next) = chars.next() { literal.push(next); }
            } else if ch == c { break; }
            else { literal.push(ch); }
        }
        // Tiny fragments such as paste(sep="_") cannot identify a file.
        if literal.len() >= 4 { result.push(literal); }
    }
    result
}
fn ordered_fragment_match(name: &str, fragments: &[String]) -> bool {
    let mut rest = name;
    for fragment in fragments {
        let Some(pos) = rest.find(fragment) else { return false; };
        rest = &rest[pos + fragment.len()..];
    }
    true
}
fn file_expression(args: &str, label: &str) -> Option<String> {
    let parts = split_args(args);
    // Explicit file/path arguments are preferred; never scan unrelated
    // strings in a call (e.g. titles, columns, or plotting parameters).
    for part in &parts {
        if let Some((key, expression)) = part.split_once('=') {
            if matches!(key.trim().to_ascii_lowercase().as_str(),
                "file" | "filename" | "filepath" | "path" | "outfile" | "output_file" | "input_file") {
                return Some(expression.trim().to_string());
            }
        }
    }
    let positional: Vec<&str> = parts.iter().filter(|p| !p.contains('=')).map(String::as_str).collect();
    let lower = label.to_ascii_lowercase();
    let index = if lower.ends_with("write.table") || lower.ends_with("write.csv") { 1 } else { 0 };
    positional.get(index).map(|s| s.trim().to_string())
}
fn fragment_candidates(db: &Connection, rel: &str, line: usize, label: &str,
    args: &str, expressions: &HashMap<String,String>, paths: &HashSet<String>) -> Result<usize> {
    let Some(arg) = file_expression(args, label) else { return Ok(0); };
    let expression = expressions.get(&arg).map(String::as_str).unwrap_or(&arg);
    let fragments = ordered_literals(expression);
    // Require either two substantial fragments or one distinctive filename
    // fragment. This deliberately avoids suggesting every *.tsv in a folder.
    if fragments.is_empty() || (fragments.len() == 1 && fragments[0].len() < 12) {
        return Ok(0);
    }
    let mut matches: Vec<&String> = paths.iter().filter(|path| {
        let basename = path.rsplit('/').next().unwrap_or(path);
        ordered_fragment_match(basename, &fragments)
    }).collect();
    matches.sort();
    // Cap candidate explosions: a broad match is not useful evidence.
    if matches.len() > 30 { return Ok(0); }
    let kind = match direction(label) {
        "writes" => "possible_output",
        "reads" => "possible_input",
        _ => return Ok(0),
    };
    for path in &matches {
        db.execute("INSERT OR IGNORE INTO issues(notebook,cell,filename,category,note)
                    VALUES (?1,?2,?3,?4,?5)",
            params![rel, line as i64, path, kind,
                format!("Candidate via {label}({arg}); ordered fragments: {}", fragments.join(" | "))])?;
    }
    Ok(matches.len())
}
fn analyze_source(db:&Connection,rel:&str,source:&str,line_base:usize,paths:&HashSet<String>,vars:&mut HashMap<String,String>,rules:&[FunctionRule], imports:&mut HashMap<String,String>, plots:&[PlotRule], language:&str)->Result<usize> {
    let assign=Regex::new(r#"^\s*(?:export\s+)?([A-Za-z_][\w.]*)\s*(?:<-|=)\s*(.*?)\s*$"#)?;
    let mut count=0;
    imports.extend(python_imports(source));
    let ops=operations(source,rules,imports,plots,language);
    // Assignments are tracked in source order, but unresolved assignments
    // invalidate earlier values rather than retaining stale provenance.
    let mut assignments:Vec<(usize,String,String)>=Vec::new();
    let mut expressions:HashMap<String,String>=HashMap::new();
    let mut offset=0;
    for line in source.lines() {
        if let Some(c)=assign.captures(line) {
            assignments.push((offset,c[1].to_owned(),c[2].to_owned()));
        }
        offset+=line.len()+1;
    }
    let mut ai=0;
    for (idx,(line,label,start)) in ops.iter().enumerate() {
        while ai<assignments.len() && assignments[ai].0<*start {
            let (_,name,expr)=&assignments[ai];
            expressions.insert(name.clone(), expr.clone());
            if let Some(v)=value(expr,vars){vars.insert(name.clone(),v);}else{vars.remove(name);}
            ai+=1;
        }
        let remaining=&source[*start..];
        let (end,balanced)=argument_end(remaining);
        // A later file operation is a recovery boundary only when the current
        // call cannot close before it. Valid nested calls remain independently visible.
        let next=ops.get(idx+1).map(|op| {
            let next_start=op.2.saturating_sub(op.1.len()+1);
            next_start.saturating_sub(*start)
        });
        let limit=if !balanced {next.map_or(end,|n|end.min(n))}else{end};
        let args=&remaining[..limit.min(remaining.len())];
        if balanced {
            if language != "r" {
                count += plot_outputs(db, rel, line_base+line-1, &canonical(label, imports), args, vars, paths, plots)?;
            }
            let name=if language == "r" { label.as_str() } else { label.rsplit('.').next().unwrap_or(label) };
            for rule in rules.iter().filter(|r|r.name==name && r.language==language) {
                count+=apply_rule(db,rel,line_base+line-1,args,rule,vars,paths)?;
            }
        }
        let mut candidates=filename_args(label,args,vars);
        // Only use the fragment fallback when the existing exact resolver
        // found no filename argument. Candidates stay in Build / Repair.
        if candidates.is_empty() && balanced {
            fragment_candidates(db,rel,line_base+line-1,label,args,&expressions,paths)?;
        }
        // A malformed call may have a complete filename literal followed by
        // another argument without a comma. Preserve that literal as evidence
        // only when the call is incomplete and the referenced file exists.
        if !balanced && candidates.is_empty() {
            let trimmed=args.trim_start();
            if let Some(q)=trimmed.chars().next().filter(|c|*c=='\'' || *c=='"') {
                let mut escaped=false;
                for (offset,c) in trimmed.char_indices().skip(1) {
                    if escaped {escaped=false;continue;}
                    if c=='\\' {escaped=true;continue;}
                    if c==q {
                        let literal=&trimmed[..offset+c.len_utf8()];
                        let tail=trimmed[offset+c.len_utf8()..].trim_start();
                        if (tail.is_empty() || tail.starts_with(',') || tail.contains('\n'))
                            && value(literal,vars).is_some() {
                            candidates.push((literal.to_owned(),label.to_owned()));
                        }
                        break;
                    }
                }
            }
        }
        for (expr,hint) in &candidates {
            let Some(path)=value(expr,vars) else {continue};
            let (resolved,_,_)=resolve_reference(&path,rel,paths);
            if !balanced && resolved.is_none() {continue;}
            count+=match_candidate(db,rel,line_base+line-1,expr,hint,vars,paths,!balanced)?;
        }
        if label.to_ascii_lowercase().ends_with("read_10x_mtx") {
            if let Some((expr,_))=candidates.first() {
                if let Some(dir)=value(expr,vars) {
                    for name in ["matrix.mtx.gz","features.tsv.gz","barcodes.tsv.gz"] {
                        let path=format!("{dir}/{name}");
                        count+=match_candidate(db,rel,line_base+line-1,&format!("'{path}'"),"read",vars,paths,!balanced)?;
                    }
                }
            }
        }
    }
    // Assignments at the end of a cell still matter to subsequent cells.
    while ai<assignments.len() {
        let (_,name,expr)=&assignments[ai];
        expressions.insert(name.clone(), expr.clone());
        if let Some(v)=value(expr,vars){vars.insert(name.clone(),v);}else{vars.remove(name);}
        ai+=1;
    }
    Ok(count)
}
fn analyze_lines(db:&Connection,rel:&str,lines:&[(usize,String)],paths:&HashSet<String>,shell:bool,rules:&[FunctionRule],plots:&[PlotRule])->Result<usize> {
    let mut vars=HashMap::new();let mut count=0;
    if shell {
        for (line,raw) in lines {
            let tokens=shell_tokens(raw.trim());
            if tokens.first().map(String::as_str)==Some("cp") && tokens.len()>=3 {
                for src in &tokens[1..tokens.len()-1] {count+=match_candidate(db,rel,*line,&format!("'{src}'"),"read",&vars,paths,false)?;}
                count+=match_candidate(db,rel,*line,&format!("'{}'",tokens.last().unwrap()),"write",&vars,paths,false)?;
            }
        }
        return Ok(count)
    }
    let mut source=String::new();
    for (_,line) in lines {source.push_str(line);source.push('\n');}
    let language=if is_r_source(rel, &source) {"r"} else {"python"};
    count+=analyze_source(db,rel,&source,1,paths,&mut vars,rules,&mut HashMap::new(),plots,language)?;
    Ok(count)
}
// Notebook files are containers: the kernel, not .ipynb, determines syntax.
fn notebook_language(notebook: &serde_json::Value) -> &'static str {
    for key in ["/metadata/kernelspec/language", "/metadata/language_info/name", "/metadata/kernelspec/name"] {
        if let Some(value) = notebook.pointer(key).and_then(|v| v.as_str()) {
            let name = value.to_ascii_lowercase();
            if name == "r" || name == "ir" || name.starts_with("ir-") { return "r"; }
            if name.contains("python") { return "python"; }
        }
    }
    "unknown"
}
fn is_r_source(rel: &str, source: &str) -> bool {
    let name = rel.to_ascii_lowercase();
    if [".r", ".rmd", ".rscript"].iter().any(|ext| name.ends_with(ext)) { return true; }
    let code = source.to_ascii_lowercase();
    code.contains("library(") || code.contains("require(") || code.contains("<-")
        || code.contains("read.table(") || code.contains("write.table(")
}
fn cell_language(source: &str, default: &'static str) -> &'static str {
    let first = source.trim_start();
    if first.starts_with("%%R") || first.starts_with("%%r\n") { "r" }
    else if first.starts_with("%%python") || first.starts_with("%%bash") || first.starts_with("%%sh") { "python" }
    else if default == "unknown" && is_r_source("", source) { "r" }
    else if default == "unknown" { "python" }
    else { default }
}
pub(super) fn parse(db:&Connection,root:&Path,rel:&str,paths:&HashSet<String>)->Result<usize>{
    let rules=load_rules(root)?;
    let plots=load_plot_rules(root)?;
    let content=std::fs::read_to_string(root.join(rel))?;
    if rel.ends_with(".ipynb") {
        let notebook:serde_json::Value=serde_json::from_str(&content)?;
        let mut vars=HashMap::new();let mut imports=HashMap::new();let mut count=0;
        let default_language=notebook_language(&notebook);
        if let Some(cells)=notebook["cells"].as_array() {
            for (idx,cell) in cells.iter().enumerate() {
                if cell["cell_type"]!="code" {continue;}
                let src=if let Some(a)=cell["source"].as_array(){a.iter().filter_map(|v|v.as_str()).collect::<String>()}else{cell["source"].as_str().unwrap_or("").to_string()};
                let language=cell_language(&src, default_language);
                count+=analyze_source(db,rel,&src,idx+1,paths,&mut vars,&rules,&mut imports,&plots,language)?;
            }
        }
        Ok(count)
    } else {
        let shell=rel.ends_with(".sh")||rel.ends_with(".bash");
        let lines:Vec<(usize,String)>=content.lines().enumerate().map(|(i,s)|(i+1,s.to_owned())).collect();
        analyze_lines(db,rel,&lines,paths,shell,&rules,&plots)
    }
}

#[cfg(test)] mod tests {
 use super::*;
 #[test] fn ordered_fragments_are_language_neutral() {
     let pieces = ordered_literals(r#"paste(sep="_", "DESeq2_result", doi, "_condition_Cell.line.tsv")"#);
     assert_eq!(pieces, vec!["DESeq2_result", "_condition_Cell.line.tsv"]);
     assert!(ordered_fragment_match("DESeq2_result_D1__condition_Cell.line.tsv", &pieces));
     assert!(!ordered_fragment_match("_condition_Cell.line.tsv_DESeq2_result", &pieces));
 }

 #[test] fn r_notebook_language_and_dotted_calls() {
     let notebook: serde_json::Value = serde_json::json!({"metadata":{"kernelspec":{"language":"R","name":"ir"}}});
     assert_eq!(notebook_language(&notebook), "r");
     assert_eq!(cell_language("write.table(x, file='out.tsv')", notebook_language(&notebook)), "r");
     assert_eq!(cell_language("%%python\nprint(1)", "r"), "python");
     assert!(is_r_source("script.R", "x <- 1"));
     let ops=operations("read.table('in.tsv')\nwrite.table(x, file='out.tsv')", &[], &HashMap::new(), &[], "r");
     assert_eq!(ops.iter().map(|(_, name, _)|name.as_str()).collect::<Vec<_>>(), vec!["read.table", "write.table"]);
 }
 #[test] fn python_import_aliases_are_canonical() {
     let imports=python_imports("import scanpy as sc\nfrom scanpy import pl as plotting\nfrom scvelo.pl import velocity_embedding_stream as ves\n");
     assert_eq!(canonical("sc.pl.umap", &imports), "scanpy.pl.umap");
     assert_eq!(canonical("plotting.umap", &imports), "scanpy.pl.umap");
     assert_eq!(canonical("ves", &imports), "scvelo.pl.velocity_embedding_stream");
 }
 #[test]
 fn archive_extensions_override_builtin_function_rule() -> Result<()> {
     let temp = std::env::temp_dir().join(format!("folder-history-rule-test-{}", std::process::id()));
     let folder = temp.join(".folder_history");
     std::fs::create_dir_all(&folder)?;
     std::fs::write(folder.join("extensions.yaml"), "version: 1\nfunctions:\n  - name: export2cellexalvr\n    language: r\n    directory_arg: 2\n    patterns: ['*.tsv']\n    source: test override\n")?;
     let result = load_rules(&temp);
     let _ = std::fs::remove_dir_all(&temp);
     let rules = result?;
     let matching: Vec<_> = rules.iter().filter(|r| r.name == "export2cellexalvr").collect();
     assert_eq!(matching.len(), 1);
     assert_eq!(matching[0].patterns, vec!["*.tsv"]);
     Ok(())
 }

 #[test] fn builtin_plot_rules_exist() {
     let rules=load_plot_rules(Path::new("/nonexistent-folderhistory-rule-test")).unwrap();
     assert!(rules.iter().any(|r| r.module=="scanpy.pl"));
     assert!(rules.iter().any(|r| r.module=="scvelo.pl"));
 }
 // Regression: malformed source code must never crash the provenance scanner.
 // This test is intentionally RED against the overnight-run implementation.
 #[test] fn malformed_fstrings_do_not_panic() {
  let vars=HashMap::new();
  for expression in ["f\"", "f'", "f\"unfinished", "f'unfinished", "f\"wrong'", "f'wrong\""] {
   let outcome=std::panic::catch_unwind(|| value(expression,&vars));
   assert!(outcome.is_ok(), "parser panicked on {expression:?}");
   assert_eq!(outcome.unwrap(),None,"malformed f-string accepted: {expression:?}");
  }
 }
 #[test] fn value_handles_unicode_and_nested_argument_boundaries() {
  let mut vars=HashMap::new();
  vars.insert("base".into(),"data".into());
  assert_eq!(value("f\"{base}/é.tsv\"",&vars).as_deref(),Some("data/é.tsv"));
  assert_eq!(split_args("'a,b', os.path.join('x', 'y'), 'é.tsv'"),
   vec!["'a,b'", "os.path.join('x', 'y')", "'é.tsv'"]);
 }
 #[test] fn variables_and_signals(){
  let mut vars=HashMap::new();vars.insert("base".into(),"data".into());
  assert_eq!(value("f\"{base}/a.h5ad\"",&vars).as_deref(),Some("data/a.h5ad"));
  assert_eq!(value("os.path.join(base, 'a.h5ad')",&vars).as_deref(),Some("data/a.h5ad"));
  assert_eq!(direction("write_my_fancy_super_duper_stats_tables"),"writes");
  assert_eq!(direction("thread_count"),"references");
  for malformed in ["f\"", "f'", "f", "f\"unterminated", "f'unterminated"] {
   assert_eq!(value(malformed,&vars),None,"malformed expression: {malformed:?}");
  }
 }
}

#[cfg(test)]
mod nasty_programming_errors_tests {
    use super::*;
    #[test]
    fn multiline_cells_and_broken_cells_use_existing_files_as_evidence() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/nasty_programming_errors");
        let rel = "notebooks/nasty_programming_errors.ipynb";
        let paths: HashSet<String> = [
            rel,
            "results/balanced_multiline.tsv",
            "results/broken_but_exists.tsv",
            "results/after_error.tsv",
            "results/nested_multiline.tsv",
        ].into_iter().map(str::to_owned).collect();
        let db = Connection::open_in_memory()?;
        schema(&db)?;
        parse(&db, &root, rel, &paths)?;
        let mut stmt = db.prepare("SELECT target, kind, evidence FROM relations WHERE source=?1")?;
        let rows: Vec<(String,String,String)> = stmt.query_map([rel], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?
            .collect::<std::result::Result<_,_>>()?;
        let targets: HashSet<&str> = rows.iter().map(|(t,_,_)| t.as_str()).collect();
        for expected in ["results/balanced_multiline.tsv", "results/after_error.tsv", "results/nested_multiline.tsv"] {
            assert!(targets.contains(expected), "balanced multiline call must link to {expected}; got {targets:?}");
        }
        assert!(targets.contains("results/broken_but_exists.tsv"),
            "broken cell with an existing output should retain a tentative candidate");
        assert!(!targets.contains("results/broken_and_missing.tsv"),
            "broken cell without an existing output must not create a link");
        assert!(!targets.contains("results/ghost.tsv"), "comments/strings must not produce links");
        assert!(rows.iter().any(|(target,kind,evidence)|
            target == "results/broken_but_exists.tsv"
            && (kind.contains("candidate") || kind.contains("tentative") || evidence.contains("uncertain") || evidence.contains("malformed"))),
            "broken-cell evidence must be explicitly marked tentative/uncertain");
        let missing_issues: i64 = db.query_row(
            "SELECT count(*) FROM issues WHERE filename='broken_and_missing.tsv'", [], |r| r.get(0))?;
        assert_eq!(missing_issues, 0, "broken code cannot justify a missing-output warning");
        Ok(())
    }
}


#[cfg(test)]
mod bioinformatics_nasty_provenance_tests {
    use super::*;
    fn fixture_root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/nasty_programming_errors")
    }

    fn paths() -> HashSet<String> {
        fn walk(dir: &Path, root: &Path, found: &mut HashSet<String>) {
            for entry in std::fs::read_dir(dir).expect("read fixture directory") {
                let path = entry.expect("read entry").path();
                if path.is_dir() { walk(&path, root, found); }
                else {
                    found.insert(path.strip_prefix(root).unwrap()
                        .to_string_lossy().replace('\\', "/"));
                }
            }
        }
        let root = fixture_root();
        let mut found = HashSet::new();
        walk(&root, &root, &mut found);
        found
    }

    fn assert_case(name: &str) -> Result<()> {
        let root = fixture_root();
        let rel = format!("notebooks/{name}.ipynb");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("expectations.json"))?)?;
        let case = &manifest[name];
        assert!(case.is_object(), "missing expectations for {name}");
        let db = Connection::open_in_memory()?;
        schema(&db)?;
        parse(&db, &root, &rel, &paths())?;
        let mut statement = db.prepare(
            "SELECT target, kind FROM relations WHERE source=?1")?;
        let found: HashSet<(String,String)> = statement.query_map([&rel], |r| {
            Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?))
        })?.collect::<std::result::Result<_,_>>()?;
        let mut missing = Vec::new();
        for edge in case["expected_edges"].as_array().unwrap() {
            let target = edge["target"].as_str().unwrap();
            let kind = edge["kind"].as_str().unwrap();
            if !found.contains(&(target.to_owned(),kind.to_owned())) {
                missing.push(format!("{kind} -> {target}"));
            }
        }
        let mut forbidden = Vec::new();
        for target in case["forbidden_targets"].as_array().unwrap() {
            let target = target.as_str().unwrap();
            if found.iter().any(|(t,_)| t.ends_with(target)) {
                forbidden.push(target.to_string());
            }
            let basename = target.rsplit('/').next().unwrap();
            let issues: i64 = db.query_row(
                "SELECT count(*) FROM issues WHERE notebook=?1 AND filename=?2",
                params![&rel,basename], |r| r.get(0))?;
            if issues != 0 {
                forbidden.push(format!("unexpected issue for {basename}"));
            }
        }
        assert!(missing.is_empty() && forbidden.is_empty(),
            "{name}: missing={missing:?}, forbidden={forbidden:?}, actual={found:?}");
        Ok(())
    }

    macro_rules! bio_case {
        ($test:ident, $name:literal) => {
            #[test]
            fn $test() -> Result<()> { assert_case($name) }
        };
    }
    bio_case!(valid_multiline_read_write, "01_valid_multiline_read_write");
    bio_case!(broken_read_followed_by_write_same_cell, "02_broken_read_followed_by_write_same_cell");
    bio_case!(broken_write_followed_by_read_same_cell, "03_broken_write_followed_by_read_same_cell");
    bio_case!(broken_write_missing_output, "04_broken_write_missing_output");
    bio_case!(broken_read_missing_input, "05_broken_read_missing_input");
    bio_case!(broken_then_next_cell, "06_broken_then_next_cell");
    bio_case!(nested_multiline, "07_nested_multiline");
    bio_case!(filename_keyword_late, "08_filename_keyword_late");
    bio_case!(variable_assignment_and_reassignment, "09_variable_assignment_and_reassignment");
    bio_case!(comments_strings_magics, "10_comments_strings_magics");
    bio_case!(fstring_and_path_join, "11_fstring_and_path_join");
    bio_case!(read_10x_mex, "12_read_10x_mex");
    bio_case!(realistic_scrna_pipeline, "13_realistic_scRNA_pipeline");
    bio_case!(r_and_seurat, "14_r_and_seurat");
    bio_case!(double_broken_calls, "15_double_broken_calls");
    bio_case!(quoted_parentheses_and_braces, "16_quoted_parentheses_and_braces");
    bio_case!(missing_fstring_recovery, "17_missing_fstring_recovery");
    bio_case!(valid_missing_output, "18_valid_missing_output");

    #[test]
    fn invalid_notebook_files_do_not_abort_directory_scan() -> Result<()> {
        let root = fixture_root();
        let mut db = Connection::open_in_memory()?;
        schema(&db)?;
        scan(&mut db, &root, false, &std::sync::atomic::AtomicBool::new(false), |_| {})?;
        let count: i64 = db.query_row(
            "SELECT count(*) FROM relations WHERE source='notebooks/01_valid_multiline_read_write.ipynb'",
            [], |r| r.get(0))?;
        assert!(count >= 2, "scan should retain valid notebook links despite malformed neighbors");
        Ok(())
    }
    #[test]
    fn scientific_function_rules_link_implicit_outputs() -> Result<()> {
        let db = Connection::open_in_memory()?;
        schema(&db)?;
        let rules = load_rules(Path::new("/nonexistent-folderhistory-rule-test"))?;
        let paths: HashSet<String> = [
            "analysis/export.R", "analysis/vr", "analysis/vr/cellexalObj.RData",
            "analysis/vr/database.sqlite", "analysis/vr/umap.mds",
            "analysis/stats", "analysis/stats/leiden_cluster_0.csv",
        ].into_iter().map(str::to_owned).collect();
        let mut vars = HashMap::new();
        analyze_source(&db, "analysis/export.R", "export2cellexalvr(obj, path='vr')", 1, &paths, &mut vars, &rules, &mut HashMap::new(), &[], "r")?;
        analyze_source(&db, "analysis/export.py", "write_stats_tables(adata, 'stats')", 2, &paths, &mut vars, &rules, &mut HashMap::new(), &[], "python")?;
        let targets = db.prepare("SELECT target FROM relations")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<HashSet<_>>>()?;
        assert!(targets.contains("analysis/vr/cellexalObj.RData"));
        assert!(targets.contains("analysis/vr/database.sqlite"));
        assert!(targets.contains("analysis/vr/umap.mds"));
        assert!(targets.contains("analysis/stats/leiden_cluster_0.csv"));
        Ok(())
    }

}
