use super::*;
pub(super) fn rule_output(db: &Connection, rel: &str, line: usize, candidate: &str, rule: &FunctionRule,
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

pub(super) fn apply_rule(db: &Connection, rel: &str, line: usize, args: &str, rule: &FunctionRule,
              env: &Environment, paths: &HashSet<String>) -> Result<usize> {
    let arguments = split_args(args);
    let arg_value = |n: usize, names: &[&str]| -> Option<String> {
        if n == 0 { return None; }
        // Named arguments may appear in any position, including R's path='data'.
        for arg in &arguments {
            if let Some((key, expr)) = arg.split_once('=') {
                if names.contains(&key.trim()) { return Variable::resolve_exact(expr, env); }
            }
        }
        // Positional arguments exclude named arguments.
        arguments.iter().filter(|a| !a.contains('=')).nth(n-1).and_then(|a| Variable::resolve_exact(a, env))
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


