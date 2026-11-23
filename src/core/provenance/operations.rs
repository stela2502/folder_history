use super::*;
pub(super) fn has_named_file_argument(after_open: &str) -> bool {
    let (end, balanced) = argument_end(after_open);
    if !balanced { return false; }
    split_args(&after_open[..end]).iter().any(|arg| {
        arg.split_once('=').is_some_and(|(key, _)| matches!(key.trim().to_ascii_lowercase().as_str(),
            "file" | "filename" | "filepath" | "outfile" | "output_file" | "input_file" | "fname"))
    })
}

pub(super) fn operations(source:&str, rules:&[FunctionRule], imports:&HashMap<String,String>, plots:&[PlotRule], operation_rules:&[OperationRule], language:&str)->Vec<(usize,String,usize)> {
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
            if open<bytes.len() && bytes[open]==b'(' && (operation_rules.iter().any(|r| r.language == language && operation_name_matches(&r.name, label, imports, language)) || file_operation(label) || rules.iter().any(|r| r.language == language && r.name == (if language == "r" {label} else {label.rsplit('.').next().unwrap_or(label)})) || plots.iter().any(|p| canonical(label, imports).starts_with(&(p.module.clone()+"."))) || has_named_file_argument(&source[open+1..])) {
                let line=source[..start].bytes().filter(|b|*b==b'\n').count()+1;
                found.push((line,label.to_owned(),open+1));
            }
            continue;
        }
        i+=1;
    }
    found
}
pub(super) fn argument_end(s:&str)->(usize,bool) {
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
pub(super) fn filename_args(label:&str, s:&str, env:&Environment)->Vec<(String,String)> {
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
        if (known || (key.is_none() && (positional_ok || second_positional))) && Variable::resolve_exact(expr,env).is_some() {
            found.push((expr.to_owned(),if known && direction(&name)!="references" {key.unwrap().to_owned()} else {label.to_owned()}));
        }
        // Scientific helper writers commonly take (data, output_directory).
        // Accept a second positional argument only when it resolves to an
        // existing directory, never an arbitrary string from options.
        if key.is_none() && !first_positional && direction(label)=="writes" {
            if let Some(path)=Variable::resolve_exact(expr,env) {
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
pub(super) fn ordered_literals(expr: &str) -> Vec<String> {
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
pub(super) fn file_expression(args: &str, label: &str) -> Option<String> {
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
pub(super) fn fragment_candidates(db: &Connection, rel: &str, line: usize, label: &str,
    args: &str, env: &Environment, paths: &HashSet<String>) -> Result<usize> {
    let Some(arg) = file_expression(args, label) else { return Ok(0); };
    let variable = Variable::from_script_slice(&arg, env, "");
    let fragments = variable.clone().into_filepath().ordered_fragments();
    // Require either two substantial fragments or one distinctive filename
    // fragment. This deliberately avoids suggesting every *.tsv in a folder.
    if fragments.is_empty() || (fragments.len() == 1 && fragments[0].len() < 12) {
        return Ok(0);
    }
    let mut matches: Vec<&String> = paths.iter().filter(|path| {
        let basename = path.rsplit('/').next().unwrap_or(path);
        variable.matches(basename)
    }).collect();
    matches.sort();
    // Cap candidate explosions: a broad match is not useful evidence.
    if matches.len() > 30 { return Ok(0); }
    let kind = match direction(label) {
        "writes" => "writes",
        "reads" => "reads",
        _ => return Ok(0),
    };
    // A single loop-generated expression may legitimately produce several files.
    // Record each inventory-backed match as an inferred relationship to this
    // script/cell, not merely as an issue that Spectator cannot display.
    for path in &matches {
        let evidence = format!(
            "{rel}: cell/line {line}: inferred {kind} via {label}({arg}); symbolic fragments: {}",
            fragments.join(" | ")
        );
        super::super::history_db::DbProvenance::record_match(db, rel, line, &arg, path, kind, &evidence)?;
    }
    Ok(matches.len())
}
