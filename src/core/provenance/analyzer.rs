use super::*;
pub(super) fn analyze_source(db:&Connection,rel:&str,source:&str,line_base:usize,paths:&HashSet<String>,env:&mut Environment,rules:&[FunctionRule], imports:&mut HashMap<String,String>, plots:&[PlotRule], operation_rules:&[OperationRule], language:&str)->Result<usize> {
    let assign=Regex::new(r#"^\s*(?:export\s+)?([A-Za-z_][\w.]*)\s*(?:<-|=)\s*(.*?)\s*$"#)?;
    let mut count=0;
    let linkers = LinkerRegistry::new(rules, language);
    imports.extend(python_imports(source));
    let ops=operations(source,rules,imports,plots,operation_rules,language);
    // Assignments are tracked in source order, but unresolved assignments
    // invalidate earlier values rather than retaining stale provenance.
    let mut assignments:Vec<(usize,String,String)>=Vec::new();
    
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
            env.assign(name, expr, language);

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
            let mut matched_typed_output = false;
            if language != "r" {
                count += plot_outputs(db, rel, line_base+line-1, &canonical(label, imports), args, env, paths, plots)?;
            }
            let name=if language == "r" { label.as_str() } else { label.rsplit('.').next().unwrap_or(label) };
            for rule in operation_rules.iter().filter(|r|r.language==language && operation_name_matches(&r.name, label, imports, language)) {
                let (matches, typed_output) =
                    apply_function_operation(db, rel, line_base+line-1, args, rule, env, paths)?;
                count += matches;
                matched_typed_output |= typed_output;
            }
            // Keep the operation evidence used to apply the declared file type.
            if matched_typed_output {
                continue;
            }
            for rule in rules.iter().filter(|r|r.name==name && r.language==language) {
                count+=apply_rule(db,rel,line_base+line-1,args,rule,env,paths)?;
            }
        }
        // Every logic module returns symbolic Variables. One shared inventory
        // matcher records the resulting relationships.
        // Resolve `for item in collection` within comprehensions without
        // interpreting the called function. The alias is local to this operation.
        let mut scoped = Environment { variables: env.variables.clone() };
        let loop_binding = Regex::new(r"\bfor\s+([A-Za-z_]\w*)\s+in\s+([A-Za-z_]\w*)")?;
        let operation_line = source[..*start].rsplit('\n').next().unwrap_or("");
        let operation_context = source[*start..].lines().next().unwrap_or("");
        let local_statement = format!("{operation_line}{operation_context}");
        if let Some(c) = loop_binding.captures(&local_statement) {
            if let Some(value) = env.variables.get(&c[2]) {
                scoped.variables.insert(c[1].to_owned(), value.clone());
            }
        }
        // YAML adds explicit argument semantics; it must never disable the
        // existing symbolic matcher (including notebook wildcard propagation).
        let module_links = if balanced { linkers.matches(&format!("{label}({args})"), &scoped) } else { Vec::new() };
        count += record_logic_links(db, rel, line_base+line-1, &module_links, paths)?;
        // Compatibility path for malformed calls and specialized legacy cases.
        // Do not record a second, contradictory reference for a handled call.
        // Preserve the original argument for specialized 10x MEX expansion.
        let needs_mex_expansion = label.to_ascii_lowercase().ends_with("read_10x_mtx");
        let mut candidates = if module_links.is_empty() || needs_mex_expansion { filename_args(label,args,env) } else { Vec::new() };
        // Only use the fragment fallback when the existing exact resolver
        // found no filename argument. Candidates stay in Build / Repair.
        if candidates.is_empty() && balanced && module_links.is_empty() {
            fragment_candidates(db,rel,line_base+line-1,label,args,env,paths)?;
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
                            && Variable::resolve_exact(literal,env).is_some() {
                            candidates.push((literal.to_owned(),label.to_owned()));
                        }
                        break;
                    }
                }
            }
        }
        for (expr,hint) in &candidates {
            let Some(path)=Variable::resolve_exact(expr,env) else {continue};
            let (resolved,_,_)=resolve_reference(&path,rel,paths);
            if !balanced && resolved.is_none() {continue;}
            count+=match_candidate(db,rel,line_base+line-1,expr,hint,env,paths,!balanced)?;
        }
        if label.to_ascii_lowercase().ends_with("read_10x_mtx") {
            if let Some((expr,_))=candidates.first() {
                if let Some(dir)=Variable::resolve_exact(expr,env) {
                    for name in ["matrix.mtx.gz","features.tsv.gz","barcodes.tsv.gz"] {
                        let path=format!("{dir}/{name}");
                        count+=match_candidate(db,rel,line_base+line-1,&format!("'{path}'"),"read",env,paths,!balanced)?;
                    }
                }
            }
        }
    }
    // Assignments at the end of a cell still matter to subsequent cells.
    while ai<assignments.len() {
        let (_,name,expr)=&assignments[ai];
        env.assign(name, expr, language);

        ai+=1;
    }
    Ok(count)
}
// Expand only explicitly enumerated shell loop values. Dynamic loops remain
// symbolic and must not be treated as confirmed writes.
pub(super) fn shell_loop_values(line: &str) -> Option<(String, Vec<String>)> {
    let line = line.trim();
    let rest = line.strip_prefix("for ")?;
    let (name, values) = rest.split_once(" in ")?;
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || name.is_empty() {
        return None;
    }
    let values = values.trim().trim_end_matches("; do").trim_end_matches(" do").trim();
    let items: Vec<String> = values.split_whitespace().map(str::to_owned).collect();
    if items.is_empty() || items.len() > 30 || items.iter().any(|v| v.chars().any(|c| matches!(c, '$' | '`' | '*' | '?' | '{' | '}'))) {
        return None;
    }
    Some((name.to_owned(), items))
}

pub(super) fn shell_expand(text: &str, env: &Environment) -> Variable {
    let mut parts = Vec::new();
    let mut literal = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '$' { literal.push(ch); continue; }
        let mut name = String::new();
        if chars.peek() == Some(&'{') {
            chars.next();
            for c in chars.by_ref() { if c == '}' { break; } name.push(c); }
        } else {
            while chars.peek().is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_') {
                name.push(chars.next().unwrap());
            }
        }
        if name.is_empty() { literal.push('$'); continue; }
        if !literal.is_empty() { parts.push(PathPart::Literal(std::mem::take(&mut literal))); }
        match env.variables.get(&name) {
            Some(Variable::Exact(value)) => parts.push(PathPart::Literal(value.clone())),
            Some(Variable::Parts(path)) => parts.extend(path.parts.iter().cloned()),
            _ => parts.push(PathPart::Wildcard),
        }
    }
    if !literal.is_empty() { parts.push(PathPart::Literal(literal)); }
    Variable::Parts(FilePath::new(parts))
}

pub(super) fn analyze_lines(db:&Connection,rel:&str,lines:&[(usize,String)],paths:&HashSet<String>,shell:bool,rules:&[FunctionRule],plots:&[PlotRule],operation_rules:&[OperationRule])->Result<usize> {
    let mut env=Environment::default();let mut count=0;
    if shell {
        let mut loop_variable: Option<(String, Vec<String>)> = None;
        let mut logical_lines = Vec::<(usize, String)>::new();
        let mut continued: Option<(usize, String)> = None;
        for (line, raw) in lines {
            let trimmed = raw.trim_end();
            if let Some(part) = trimmed.strip_suffix('\\') {
                let (first, mut text) = continued.take().unwrap_or((*line, String::new()));
                text.push_str(part);
                text.push(' ');
                continued = Some((first, text));
            } else if let Some((first, mut text)) = continued.take() {
                text.push_str(trimmed);
                logical_lines.push((first, text));
            } else { logical_lines.push((*line, raw.clone())); }
        }
        if let Some(unfinished) = continued { logical_lines.push(unfinished); }
        let mut cwd: Option<String> = None;
        for (line,raw) in &logical_lines {
            let raw = raw.trim();
            if let Some(values) = shell_loop_values(raw) {
                loop_variable = Some(values);
                continue;
            }
            if raw == "done" { loop_variable = None; continue; }
            // A simple shell assignment can refer to an enumerated loop variable.
            if let Some((name, value)) = raw.split_once('=') {
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    let template = value.trim().trim_matches('"').trim_matches('\'');
                    let variants: Vec<Option<String>> = match &loop_variable {
                        Some((_, values)) => values.iter().cloned().map(Some).collect(),
                        None => vec![None],
                    };
                    let mut resolved = Vec::new();
                    for variant in variants {
                        let mut local = Environment { variables: env.variables.clone() };
                        if let (Some((loop_name, _)), Some(value)) = (&loop_variable, variant) {
                            local.variables.insert(loop_name.clone(), Variable::Exact(value));
                        }
                        resolved.push(shell_expand(template, &local));
                    }
                    // Only preserve an exact value when all iterations agree.
                    env.variables.insert(name.to_owned(), if resolved.len() == 1 {
                        resolved.remove(0)
                    } else { Variable::Unknown });
                    // Explicit loop outputs are handled below by re-expanding
                    // the assignment for each enumerated loop value.
                    continue;
                }
            }
            let tokens=shell_tokens(raw);
            if tokens.first().map(String::as_str) == Some("cd") {
                cwd = tokens.get(1).cloned();
                continue;
            }
            let (segments, redirects) = shell_pipeline(raw);
            for segment in &segments {
                count += apply_shell_operation(db, rel, *line, segment, operation_rules, &env, paths, cwd.as_deref())?;
            }
            // Shell redirections are not tool-specific. A pipe connects the
            // operations without inventing a physical intermediate artifact.
            for (operator, path) in &redirects {
                let hint = if operator.contains('<') { "read" } else { "write" };
                count += match_candidate(db, rel, *line, &format!("'{path}'"), hint, &env, paths, false)?;
            }
            if tokens.first().map(String::as_str)==Some("cp") && tokens.len()>=3 {
                for src in &tokens[1..tokens.len()-1] {count+=match_candidate(db,rel,*line,&format!("'{src}'"),"read",&env,paths,false)?;}
                count+=match_candidate(db,rel,*line,&format!("'{}'",tokens.last().unwrap()),"write",&env,paths,false)?;
            }
            if let Some(op) = extract_shell(raw) {
                for output in op.writes {
                    let output = output.trim_matches('"').trim_matches('\'');
                    if let Some((loop_name, values)) = &loop_variable {
                        for value in values {
                            let mut local = Environment { variables: env.variables.clone() };
                            local.variables.insert(loop_name.clone(), Variable::Exact(value.clone()));
                            // Re-evaluate the preceding assignment under this
                            // iteration, rather than matching a broad wildcard.
                            for (_, previous) in lines.iter().take_while(|(n, _)| n < line) {
                                if let Some((name, expr)) = previous.trim().split_once('=') {
                                    if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                                        let expr = expr.trim().trim_matches('"').trim_matches('\'');
                                        local.variables.insert(name.to_owned(), shell_expand(expr, &local));
                                    }
                                }
                            }
                            if let Variable::Parts(path) = shell_expand(output, &local) {
                                for target in paths.iter().filter(|p| path.matches(p)) {
                                    super::super::history_db::DbProvenance::record_match(db, rel, *line, output, target, "writes", &format!("{rel}: line {line}: shell loop redirection"))?;
                                    count += 1;
                                }
                            }
                        }
                    } else if let Variable::Parts(path) = shell_expand(output, &env) {
                        for target in paths.iter().filter(|p| path.matches(p)) {
                            super::super::history_db::DbProvenance::record_match(db, rel, *line, output, target, "writes", &format!("{rel}: line {line}: shell redirection"))?;
                            count += 1;
                        }
                    }
                }
            }
        }
        return Ok(count)
    }
    let mut source=String::new();
    for (_,line) in lines {source.push_str(line);source.push('\n');}
    let language=if is_r_source(rel, &source) {"r"} else {"python"};
    count+=analyze_source(db,rel,&source,1,paths,&mut env,rules,&mut HashMap::new(),plots,operation_rules,language)?;
    Ok(count)
}
// Notebook files are containers: the kernel, not .ipynb, determines syntax.
