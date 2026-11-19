use super::*;
pub(super) fn notebook_language(notebook: &serde_json::Value) -> &'static str {
    for key in ["/metadata/kernelspec/language", "/metadata/language_info/name", "/metadata/kernelspec/name"] {
        if let Some(value) = notebook.pointer(key).and_then(|v| v.as_str()) {
            let name = value.to_ascii_lowercase();
            if name == "r" || name == "ir" || name.starts_with("ir-") { return "r"; }
            if name.contains("python") { return "python"; }
        }
    }
    "unknown"
}
pub(super) fn is_r_source(rel: &str, source: &str) -> bool {
    let name = rel.to_ascii_lowercase();
    if [".r", ".rmd", ".rscript"].iter().any(|ext| name.ends_with(ext)) { return true; }
    let code = source.to_ascii_lowercase();
    code.contains("library(") || code.contains("require(") || code.contains("<-")
        || code.contains("read.table(") || code.contains("write.table(")
}
pub(super) fn cell_language(source: &str, default: &'static str) -> &'static str {
    let first = source.trim_start();
    if first.starts_with("%%R") || first.starts_with("%%r\n") { "r" }
    else if first.starts_with("%%python") || first.starts_with("%%bash") || first.starts_with("%%sh") { "python" }
    else if default == "unknown" && is_r_source("", source) { "r" }
    else if default == "unknown" { "python" }
    else { default }
}
pub(crate) fn parse(db:&Connection,root:&Path,rel:&str,paths:&HashSet<String>)->Result<usize>{
    let rules=load_rules(root)?;
    let plots=load_plot_rules(root)?;
    let content=std::fs::read_to_string(root.join(rel))?;
    if rel.ends_with(".ipynb") {
        let notebook:serde_json::Value=serde_json::from_str(&content)?;
        let mut env=Environment::default();let mut imports=HashMap::new();let mut count=0;
        let default_language=notebook_language(&notebook);
        if let Some(cells)=notebook["cells"].as_array() {
            for (idx,cell) in cells.iter().enumerate() {
                if cell["cell_type"]!="code" {continue;}
                let src=if let Some(a)=cell["source"].as_array(){a.iter().filter_map(|v|v.as_str()).collect::<String>()}else{cell["source"].as_str().unwrap_or("").to_string()};
                let language=cell_language(&src, default_language);
                count+=analyze_source(db,rel,&src,idx+1,paths,&mut env,&rules,&mut imports,&plots,language)?;
            }
        }
        Ok(count)
    } else {
        let shell=rel.ends_with(".sh")||rel.ends_with(".bash");
        let lines:Vec<(usize,String)>=content.lines().enumerate().map(|(i,s)|(i+1,s.to_owned())).collect();
        analyze_lines(db,rel,&lines,paths,shell,&rules,&plots)
    }
}

