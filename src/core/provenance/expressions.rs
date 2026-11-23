use super::*;
pub(super) fn direction(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    let tokens: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric()).collect();
    if tokens.iter().any(|t| ["write", "save", "export", "dump", "output", "outfile", "ofile", "to"].contains(t))
        || ["write", "save", "export", "dump"].iter().any(|p| lower.starts_with(p))
        || lower.starts_with("to_") { "writes" }
    else if tokens.iter().any(|t| ["read", "load", "scan", "source", "input", "ifile", "from"].contains(t))
        || ["read", "load", "scan", "source"].iter().any(|p| lower.starts_with(p)) { "reads" }
    else { "references" }
}
pub(super) fn split_args(s: &str) -> Vec<String> {
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
impl Variable {
    pub(super) fn resolve_exact(expr: &str, env: &Environment) -> Option<String> {
    let s=expr.trim();
    if let Some(v)=env.exact(s) {return Some(v.to_owned());}
    if s.len()>=2 && ((s.starts_with('"')&&s.ends_with('"'))||(s.starts_with('\'')&&s.ends_with('\''))) {
        return Some(s[1..s.len()-1].to_string());
    }
    if (s.starts_with("f\"") && s.ends_with('"')) || (s.starts_with("f'") && s.ends_with('\'')) {
        if s.len() < 3 { return None; }
        let mut result=s[2..s.len()-1].to_string();
        for (k,v) in &env.variables {
            if let Variable::Exact(exact) = v { result=result.replace(&format!("{{{k}}}"),exact); }
        }
        if !result.contains('{') {return Some(result);} return None;
    }
    for prefix in ["os.path.join(","file.path(","Path(","pathlib.Path("] {
        if let Some(rest)=s.strip_prefix(prefix).and_then(|x|x.strip_suffix(')')) {
            let parts=split_args(rest); let vals=parts.iter().map(|p|Self::resolve_exact(p,env)).collect::<Option<Vec<_>>>()?;
            if prefix.ends_with("Path(") {return vals.into_iter().next();}
            let mut path=std::path::PathBuf::new(); for v in vals {path.push(v);} return Some(normalized(&path));
        }
    }
    if s.contains(" / ") || s.contains(" + ") {
        let separator=if s.contains(" / ") {" / "} else {" + "};
        let vals=s.split(separator).map(|x|Self::resolve_exact(x,env)).collect::<Option<Vec<_>>>()?;
        return Some(if separator==" / " {vals.join("/")} else {vals.concat()});
    }
    None
}
}

pub(super) fn match_candidate(db:&Connection, rel:&str, line:usize, expr:&str, label:&str, env:&Environment, paths:&HashSet<String>, malformed:bool) -> Result<usize> {
    let Some(v)=Variable::resolve_exact(expr,env) else {return Ok(0)};
    let (resolved,_external,_ambiguous)=resolve_reference(&v,rel,paths);
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
        if kind != "references" {
            super::super::history_db::DbProvenance::record_match(db,rel,line,&v,&target,kind,&evidence)?;
        }
        return Ok(1);
    }
    Ok(0)
}
// A small lexical pass: find file operations without requiring balanced calls.
// Deliberately recover at a new operation even inside an unfinished call.
pub(super) fn file_operation(label: &str) -> bool {
    let lower=label.to_ascii_lowercase();
    direction(&lower)!="references" || lower.ends_with("to_csv") || lower.ends_with("to_parquet")
}
//fn is_identifier(c:char)->bool {c.is_ascii_alphanumeric() || c=='_' || c=='.'}
// Resolve Python import aliases to canonical module/function paths.
pub(super) fn python_imports(source: &str) -> HashMap<String, String> {
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
pub(super) fn canonical(label: &str, imports: &HashMap<String, String>) -> String {
    let (head, rest) = label.split_once('.').unwrap_or((label, ""));
    match imports.get(head) {
        Some(module) if rest.is_empty() => module.clone(),
        Some(module) => format!("{module}.{rest}"),
        None => label.to_string(),
    }
}
pub(super) fn plot_outputs(
    db: &Connection, rel: &str, line: usize, canonical_call: &str,
    args: &str, env: &Environment, paths: &HashSet<String>,
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
        } else if let Some(v) = Variable::resolve_exact(&save_expr, env) {
            if v.is_empty() { continue; }
            v
        } else { continue };
        let directory = env.exact(&rule.directory_setting)
            .map(str::to_owned).unwrap_or_else(|| rule.default_directory.clone());
        let format = env.exact(&rule.format_setting)
            .map(str::to_owned).unwrap_or_else(|| rule.default_format.clone());
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
                super::super::history_db::DbProvenance::record_match(db, rel, line, &candidate, &target, "writes", &evidence)?;
                count += 1;
            }
        }
    }
    Ok(count)
}

