use super::*;
use super::cli_rules::positional_bounds;

#[cfg(test)] mod notebook_glob_red_tests {
    use super::*;

    #[test]
    fn notebook_ifiles_glob_to_adatas_four_real_h5_reads_red_proof() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
    	  "folder_history_ifiles_red_{}",
          std::process::id()
        ));
        std::fs::create_dir_all(&root)?;
        let rel = "Analyze_scanpy_by_Stefan_updated_demux10x.ipynb";
        let notebook = serde_json::json!({
            "metadata": {"kernelspec": {"language": "python"}},
            "cells": [
                {"cell_type": "code", "source": [
                    "import glob\n",
                    "ifiles = glob.glob('*/*.h5')\n",
                    "ifiles\n"
                ]},
                {"cell_type": "code", "source": [
                    "adatas = [ scanpy.read_10x_h5( f ) for f in ifiles ]\n",
                    "adatas\n"
                ]}
            ]
        });
        std::fs::write(root.join(rel), serde_json::to_vec(&notebook)?)?;
        let expected: HashSet<String> = [
            "adult-spring/filtered_feature_bc_matrix_DONT_USE_ADTS_OR_HTOS.h5",
            "adult-summer/filtered_feature_bc_matrix_DONT_USE_ADTS_OR_HTOS.h5",
            "trajectory/filtered_feature_bc_matrix_DONT_USE_ADTS_OR_HTOS.h5",
            "embryo/filtered_feature_bc_matrix_DONT_USE_ADTS_OR_HTOS.h5",
        ].into_iter().map(str::to_owned).collect();
        let mut inventory = expected.clone();
        inventory.insert(rel.to_owned());
        inventory.insert("adult-spring/deeper/incorrect.h5".to_owned());
        inventory.insert("adult-spring/not_an_h5.h5ad".to_owned());
        let db = Connection::open_in_memory()?;
        schema(&db)?;
        let parsed = parse(&db, &root, rel, &inventory);
        let _ = std::fs::remove_dir_all(&root);
        parsed?;
        let mut stmt = db.prepare(
            "SELECT a.path FROM file_matches m JOIN artifacts s ON s.id=m.source_id JOIN artifacts a ON a.id=m.artifact_id WHERE s.path=?1 AND m.direction='reads'"
        )?;
        let found: HashSet<String> = stmt.query_map([rel], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        // Diagnose the real symbolic Variable matcher, not a separate regex.
        let mut diagnostic_env = Environment::default();
        diagnostic_env.assign("ifiles", "glob.glob('*/*.h5')", "python");
        diagnostic_env.assign("adatas", "[ scanpy.read_10x_h5( f ) for f in ifiles ]", "python");
        for name in ["ifiles", "adatas"] {
            let variable = diagnostic_env.variables.get(name).expect("assigned variable");
            eprintln!("IFILES_VARIABLE: {name} = {variable:?}");
            for path in expected.iter().map(String::as_str).chain([
                "adult-spring/deeper/incorrect.h5",
                "adult-spring/not_an_h5.h5ad",
            ]) {
                eprintln!("IFILES_MATCH: {name}.match_as_str({path:?}) = {}", variable.match_as_str(path));
            }
        }
        eprintln!("IFILES_GLOB_TEST_RAN: found {found:?}; expected {expected:?}");
        assert_eq!(found, expected, "the notebook must link exactly four real H5 reads");
        Ok(())
    }
}

#[cfg(test)] mod tests {
 use super::*;
 #[test] fn symbolic_filepath_matches_and_normalizes() {
     let path = FilePath::new(vec![PathPart::Literal("results/".into()),
         PathPart::Literal("DESeq2_".into()), PathPart::Wildcard,
         PathPart::Literal(".tsv".into())]);
     assert!(path.matches("results/DESeq2_WT.tsv"));
     assert!(path.matches("results/DESeq2_MUT.tsv"));
     assert!(!path.matches("other/DESeq2_WT.tsv"));
     assert!(!path.matches("results/DESeq2_WT.csv"));
     assert_eq!(Variable::Exact("a.tsv".into()).into_filepath().parts,
         vec![PathPart::Literal("a.tsv".into())]);
 }
 #[test] fn wildcard_is_one_directory_component() {
     let mut env = Environment::default();
     env.assign("ifiles", "anything('*/*.h5')", "python");
     let value = &env.variables["ifiles"];
     assert!(value.matches("adult-spring/data.h5"));
     assert!(!value.matches("adult-spring/deeper/data.h5"));
     assert!(!value.matches("adult-spring/data.h5ad"));
 }
 #[test] fn generic_assignment_propagates_symbolic_file_dependency() {
     let mut env = Environment::default();
     env.assign("ifiles", "glob.glob('*/*.h5')", "python");
     env.assign("adatas", "[scanpy.read_10x_h5(f) for f in ifiles]", "python");
     assert!(env.variables["adatas"].matches("adult-spring/sample.h5"));
     assert!(!env.variables["adatas"].matches("adult-spring/sample.h5ad"));
 }
 #[test] fn wildcard_filename_from_arbitrary_function_assignment() -> Result<()> {
     let mut env = Environment::default();
     env.assign("ifiles", "glob.glob('*/*.h5')", "python");
     let glob = env.variables["ifiles"].clone();
     assert!(glob.matches("adult-spring/sample.h5"));
     assert!(!glob.matches("adult-spring/sample.h5ad"));
     env.assign("other", "my_weird_function('*/*.h5')", "python");
     assert_eq!(env.variables["other"], glob);
     env.assign("noise", "some_function('separator', 'title')", "python");
     assert!(!env.variables["noise"].matches("adult-spring/sample.h5"));
     let paths: HashSet<String> = ["adult-spring/sample.h5", "embryo/sample.h5",
         "adult-spring/sample.h5ad"].into_iter().map(str::to_owned).collect();
     let db = Connection::open_in_memory()?;
     schema(&db)?;
     let links = vec![LogicLinks { kind: LinkType::Reads,
         links: LinksToFiles { files: vec![glob] },
         expression: "ifiles".into(), function: "read_10x_h5".into() }];
     assert_eq!(record_logic_links(&db, "analysis.ipynb", 2, &links, &paths)?, 2);
     let mut stmt = db.prepare("SELECT a.path FROM file_matches m JOIN artifacts s ON s.id=m.source_id JOIN artifacts a ON a.id=m.artifact_id WHERE s.path='analysis.ipynb' AND m.direction='reads'")?;
     let found: HashSet<String> = stmt.query_map([], |row| row.get(0))?
         .collect::<std::result::Result<_, _>>()?;
     assert_eq!(found, ["adult-spring/sample.h5".to_owned(), "embryo/sample.h5".to_owned()].into_iter().collect());
     Ok(())
 }
 #[test] fn environment_resolves_assignments_in_order() {
     let mut env = Environment::default();
     env.assign("base", "'results'", "python");
     env.assign("file", "os.path.join(base, 'first.tsv')", "python");
     assert_eq!(env.exact("file"), Some("results/first.tsv"));
     env.assign("file", "os.path.join(base, 'second.tsv')", "python");
     assert_eq!(env.exact("file"), Some("results/second.tsv"));
     env.assign("file", "paste0('DESeq2_', condition, '.tsv')", "r");
     assert!(env.exact("file").is_none());
     assert!(env.variables["file"].matches("DESeq2_WT.tsv"));
 }

 #[test] fn python_dynamic_h5ad_filename_preserves_literal_boundaries() {
     let env = Environment::default();
     let expression = r#""analyzed_cellranger_h5_" + adatas[0].obs["sname"][0] + ".h5ad""#;
     let actual = Variable::from_script_slice(expression, &env, "python").into_filepath();
     let expected = FilePath::new(vec![
         PathPart::Literal("analyzed_cellranger_h5_".into()),
         PathPart::Wildcard,
         PathPart::Literal(".h5ad".into()),
     ]);
     assert_eq!(actual, expected, "dynamic filename must preserve exact prefix and suffix");
     assert!(actual.matches("analyzed_cellranger_h5_embryo.h5ad"));
     assert!(actual.matches("analyzed_cellranger_h5_adult-spring.h5ad"));
     assert!(!actual.matches("backup_analyzed_cellranger_h5_embryo.h5ad"));
     assert!(!actual.matches("analyzed_cellranger_h5_embryo.h5ad.tmp"));
 }

 #[test] fn ordered_fragments_are_language_neutral() {
     let pieces = ordered_literals(r#"paste(sep="_", "DESeq2_result", doi, "_condition_Cell.line.tsv")"#);
     assert_eq!(pieces, vec!["DESeq2_result", "_condition_Cell.line.tsv"]);
     let path = FilePath::new(vec![
         PathPart::Wildcard,
         PathPart::Literal(pieces[0].clone()),
         PathPart::Wildcard,
         PathPart::Literal(pieces[1].clone()),
         PathPart::Wildcard,
     ]);
     assert!(path.matches("DESeq2_result_D1__condition_Cell.line.tsv"));
     assert!(!path.matches("_condition_Cell.line.tsv_DESeq2_result"));
 }

 #[test] fn r_notebook_language_and_dotted_calls() {
     let notebook: serde_json::Value = serde_json::json!({"metadata":{"kernelspec":{"language":"R","name":"ir"}}});
     assert_eq!(notebook_language(&notebook), "r");
     assert_eq!(cell_language("write.table(x, file='out.tsv')", notebook_language(&notebook)), "r");
     assert_eq!(cell_language("%%python\nprint(1)", "r"), "python");
     assert!(is_r_source("script.R", "x <- 1"));
     let ops=operations("read.table('in.tsv')\nwrite.table(x, file='out.tsv')", &[], &HashMap::new(), &[], &[], "r");
     assert_eq!(ops.iter().map(|(_, name, _)|name.as_str()).collect::<Vec<_>>(), vec!["read.table", "write.table"]);
 }
 #[test] fn python_import_aliases_are_canonical() {
     let imports=python_imports("import scanpy as sc\nfrom scanpy import pl as plotting\nfrom scvelo.pl import velocity_embedding_stream as ves\n");
     assert_eq!(canonical("sc.pl.umap", &imports), "scanpy.pl.umap");
     assert_eq!(canonical("plotting.umap", &imports), "scanpy.pl.umap");
     assert_eq!(canonical("ves", &imports), "scvelo.pl.velocity_embedding_stream");
 }
 #[test]
 fn archive_extensions_override_builtin_operation_rule() -> Result<()> {
     let temp = std::env::temp_dir().join(format!("folder-history-rule-test-{}", std::process::id()));
     let folder = temp.join(".folder_history");
     std::fs::create_dir_all(&folder)?;
     std::fs::write(folder.join("extensions.yaml"), "operations:\n  - name: export2cellexalvr\n    language: r\n    arguments:\n      - {position: 2, role: output, kind: directory, expected_files: ['*.tsv']}\n")?;
     let result = super::rules::load_operation_rules(&temp);
     let _ = std::fs::remove_dir_all(&temp);
     let rules = result?;
     let matching: Vec<_> = rules.iter().filter(|r| r.name == "export2cellexalvr").collect();
     assert_eq!(matching.len(), 1);
     assert_eq!(matching[0].arguments[0].expected_files, vec!["*.tsv"]);
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
  let vars=Environment::default();
  for expression in ["f\"", "f'", "f\"unfinished", "f'unfinished", "f\"wrong'", "f'wrong\""] {
   let outcome=std::panic::catch_unwind(|| Variable::resolve_exact(expression,&vars));
   assert!(outcome.is_ok(), "parser panicked on {expression:?}");
   assert_eq!(outcome.unwrap(),None,"malformed f-string accepted: {expression:?}");
  }
 }
 #[test] fn value_handles_unicode_and_nested_argument_boundaries() {
  let mut vars=Environment::default();
  vars.assign("base", "'data'", "python");
  assert_eq!(Variable::resolve_exact("f\"{base}/é.tsv\"",&vars).as_deref(),Some("data/é.tsv"));
  assert_eq!(split_args("'a,b', os.path.join('x', 'y'), 'é.tsv'"),
   vec!["'a,b'", "os.path.join('x', 'y')", "'é.tsv'"]);
 }
 #[test] fn variables_and_signals(){
  let mut vars=Environment::default();vars.assign("base", "'data'", "python");
  assert_eq!(Variable::resolve_exact("f\"{base}/a.h5ad\"",&vars).as_deref(),Some("data/a.h5ad"));
  assert_eq!(Variable::resolve_exact("os.path.join(base, 'a.h5ad')",&vars).as_deref(),Some("data/a.h5ad"));
  assert_eq!(direction("write_my_fancy_super_duper_stats_tables"),"writes");
  assert_eq!(direction("thread_count"),"references");
  for malformed in ["f\"", "f'", "f", "f\"unterminated", "f'unterminated"] {
   assert_eq!(Variable::resolve_exact(malformed,&vars),None,"malformed expression: {malformed:?}");
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
        let mut stmt = db.prepare("SELECT a.path, m.direction, m.evidence FROM file_matches m JOIN artifacts s ON s.id=m.source_id JOIN artifacts a ON a.id=m.artifact_id WHERE s.path=?1")?;
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
        assert!(rows.iter().any(|(target,_kind,evidence)|
            target == "results/broken_but_exists.tsv"
            && (evidence.contains("uncertain") || evidence.contains("malformed") || evidence.contains("tentative"))),
            "broken-cell evidence must be explicitly marked tentative/uncertain");

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
            "SELECT a.path, m.direction FROM file_matches m JOIN artifacts s ON s.id=m.source_id JOIN artifacts a ON a.id=m.artifact_id WHERE s.path=?1")?;
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
            "SELECT count(*) FROM file_matches m JOIN artifacts s ON s.id=m.source_id WHERE s.path='notebooks/01_valid_multiline_read_write.ipynb'",
            [], |r| r.get(0))?;
        assert!(count >= 2, "scan should retain valid notebook links despite malformed neighbors");
        Ok(())
    }
    #[test]
    fn scientific_function_rules_link_implicit_outputs() -> Result<()> {
        let db = Connection::open_in_memory()?;
        schema(&db)?;
        let root = Path::new("/nonexistent-folderhistory-rule-test");
        let operation_rules = super::super::rules::load_operation_rules(root)?;
        let paths: HashSet<String> = [
            "analysis/export.R", "analysis/vr", "analysis/vr/cellexalObj.RData",
            "analysis/vr/database.sqlite", "analysis/vr/umap.mds",
            "analysis/stats", "analysis/stats/leiden_cluster_0.csv",
        ].into_iter().map(str::to_owned).collect();
        let mut env = Environment::default();
        analyze_source(&db, "analysis/export.R", "export2cellexalvr(obj, path='vr')", 1, &paths, &mut env, &[], &mut HashMap::new(), &[], &operation_rules, "r")?;
        analyze_source(&db, "analysis/export.py", "write_stats_tables(adata, 'stats')", 2, &paths, &mut env, &[], &mut HashMap::new(), &[], &operation_rules, "python")?;
        let targets = db.prepare("SELECT a.path FROM file_matches m JOIN artifacts a ON a.id=m.artifact_id")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<HashSet<_>>>()?;
        assert!(targets.contains("analysis/vr/cellexalObj.RData"));
        assert!(targets.contains("analysis/vr/database.sqlite"));
        assert!(targets.contains("analysis/vr/umap.mds"));
        assert!(targets.contains("analysis/stats/leiden_cluster_0.csv"));
        Ok(())
    }

}

// Regression for real loop-generated filenames. These are deliberately end-to-end:
// a matcher-only test would miss assignment and write-call extraction failures.
#[cfg(test)]
mod loop_generated_output_regressions {
    use super::*;

    const OUTPUTS: [&str; 3] = [
        "DESeq2_result_D1__condition_Cell.line.tsv",
        "DESeq2_result_D2__condition_Cell.line.tsv",
        "DESeq2_result_D4__condition_Cell.line.tsv",
    ];

    fn verify_script(language: &str, source: &str) -> Result<()> {
        let db = Connection::open_in_memory()?;
        schema(&db)?;
        let paths: HashSet<String> = OUTPUTS.iter().map(|s| s.to_string()).collect();
        let mut env = Environment::default();
        analyze_source(&db, "analysis", source, 1, &paths, &mut env,
            &[], &mut HashMap::new(), &[], &[], language)?;
        let mut stmt = db.prepare("SELECT a.path, m.direction, m.evidence FROM file_matches m JOIN artifacts s ON s.id=m.source_id JOIN artifacts a ON a.id=m.artifact_id WHERE s.path='analysis'")?;
        let links: Vec<(String, String, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<_, _>>()?;
        for expected in OUTPUTS {
            assert!(links.iter().any(|(path, kind, evidence)|
                path == expected && kind == "writes" && evidence.contains("cell/line")),
                "{language}: no recorded write relation for {expected}; relations={links:?}");
        }
        Ok(())
    }

    #[test]
    fn r_paste_loop_write_table() -> Result<()> {
        verify_script("r", r#"
for (doi in names(table(samp2[,'Day.of.induction']))) {
    tmpSamp = samp2[which(samp2[,'Day.of.induction'] == doi),]
    fn = paste(sep="_", "DESeq2_result", doi, "_condition_Cell.line.tsv")
    write.table(result, file=fn, quote=F, sep="\t", row.names=F)
}
"#)
    }

    #[test]
    fn python_fstring_loop_write_csv() -> Result<()> {
        verify_script("python", r#"
for doi in days:
    fn = f"DESeq2_result_{doi}__condition_Cell.line.tsv"
    result.to_csv(fn, sep="\t", index=False)
"#)
    }

    #[test]
    fn bash_loop_redirect_outputs() -> Result<()> {
        // Shell parsing has a separate entry point; this test must exercise it.
        let db = Connection::open_in_memory()?;
        schema(&db)?;
        let paths: HashSet<String> = OUTPUTS.iter().map(|s| s.to_string()).collect();
        let script = r#"for doi in D1 D2 D4; do
  fn="DESeq2_result_${doi}__condition_Cell.line.tsv"
  printf '%s\n' 'result' > "$fn"
done"#;
        let lines: Vec<(usize, String)> = script.lines().enumerate()
            .map(|(i, line)| (i + 1, line.to_owned())).collect();
        analyze_lines(&db, "analysis.sh", &lines, &paths, true, &[], &[], &[])?;
        let count: i64 = db.query_row(
            "SELECT count(*) FROM file_matches WHERE direction='writes'", [], |r| r.get(0))?;
        assert_eq!(count, 3, "Bash loop should identify all three output files");
        Ok(())
    }
}

#[cfg(test)]
mod logic_module_regressions {
    use super::*;

    #[test]
    fn graphics_devices_produce_symbolic_write_links() {
        let env = Environment::default();
        let links = logic_links("png", "file=\"hclust.png\", width=600", &env, &[], "r");
        assert!(links.iter().any(|link| link.kind == LinkType::Writes
            && link.links.files.contains(&Variable::Exact("hclust.png".into()))));
    }

    #[test]
    fn generic_unknown_call_is_not_assumed_to_write() {
        let env = Environment::default();
        let links = logic_links("validate_file", "file='input.tsv'", &env, &[], "python");
        assert!(links.iter().any(|link| link.kind == LinkType::References));
        assert!(!links.iter().any(|link| link.kind == LinkType::Writes));
    }

    #[test]
    fn legacy_yaml_rules_return_variables_before_inventory_matching() {
        let rule = FunctionRule {
            name: "write_stats_tables".into(), language: "python".into(),
            directory_arg: Some(2), output_arg: None, default_output: None,
            files: vec![], patterns: vec![], source: String::new(),
        };
        let mut env = Environment::default();
        env.assign("output_dir", "'results'", "python");
        let links = logic_links("write_stats_tables", "adata, output_dir", &env, &[rule], "python");
        assert!(links.iter().any(|link| link.kind == LinkType::Writes
            && link.links.files.contains(&Variable::Exact("results".into()))));
    }

    #[test]
    fn six_r_pngs_create_actual_write_relations() -> Result<()> {
        let db = Connection::open_in_memory()?;
        schema(&db)?;
        let filenames = [
            "CombinedLinearAnalysis_DESeq2_is_that_what_we_want.png",
            "CombinedLinearAnalysis_rev_DESeq2_is_that_what_we_want.png",
            "TranscriptionFactors_InTheLinearTest.png",
            "EpigeneticFactors_InTheLinearTest.png",
            "CellSurfaceFactors_InTheLinearTest.png",
            "hclust.png",
        ];
        let paths: HashSet<String> = filenames.iter().map(|s| s.to_string()).collect();
        let source = filenames.iter().map(|name| format!(
            "png(file=\"{name}\", width=600)\nplot(hc)\ndev.off()\n"
        )).collect::<String>();
        let mut env = Environment::default();
        analyze_source(&db, "analysis", &source, 1, &paths, &mut env,
            &[], &mut HashMap::new(), &[], &[], "r")?;
        let mut stmt = db.prepare("SELECT a.path FROM file_matches m JOIN artifacts s ON s.id=m.source_id JOIN artifacts a ON a.id=m.artifact_id WHERE s.path='analysis' AND m.direction='writes'")?;
        let targets: HashSet<String> = stmt.query_map([], |row| row.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        for name in filenames { assert!(targets.contains(name), "missing write relation: {name}"); }
        Ok(())
    }
}

#[test]
fn matcher_registry_returns_one_match_with_symbolic_files() {
    let mut env = Environment::default();
    env.assign("ifiles", "glob.glob('*/*.h5')", "python");
    let registry = LinkerRegistry::new(&[], "python");
    let results = registry.matches("scanpy.read_10x_h5(ifiles)", &env);
    assert!(results.iter().any(|result| {
        result.kind == LinkType::Reads
            && result.links.files.iter().any(|file| {
                file.matches("adult-spring/example.h5")
            })
    }), "generic matcher should preserve symbolic H5 reads");
}

#[cfg(test)]
mod moved_data_integration_tests {
    use super::*;

    #[test]
    fn relocated_h5_enters_normal_investigation_pipeline() -> Result<()> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/unlinked_fastq_fixture");
        let rel = "analysis1/analyze.py";
        let mut inventory: HashSet<String> = [
            rel.to_owned(),
            "data/set1/set1.h5".to_owned(),
            "data/set2/set2.h5".to_owned(),
            "data/set3/set3.h5".to_owned(),
        ].into_iter().collect();
        let db = Connection::open_in_memory()?;
        schema(&db)?;
        parse(&db, &root, rel, &inventory)?;
        let candidates: i64 = db.query_row(
            "SELECT count(*) FROM file_matches m JOIN artifacts s ON s.id=m.source_id \
             WHERE s.path=?1 AND m.direction='reads' AND m.decision_id=0",
            [rel], |r| r.get(0))?;
        assert_eq!(candidates, 3, "relocated files must remain reviewable candidates");
        let confirmed: i64 = db.query_row(
            "SELECT count(*) FROM file_matches m JOIN artifacts s ON s.id=m.source_id \
             WHERE s.path=?1 AND m.direction='reads' AND m.decision_id<>0",
            [rel], |r| r.get(0))?;
        assert_eq!(confirmed, 0, "relocated files must not be silently decided");
        // Correctly located files take precedence over recovery.
        inventory.insert("analysis1/set1/set1.h5".to_owned());
        let second = Connection::open_in_memory()?;
        schema(&second)?;
        parse(&second, &root, rel, &inventory)?;
        let strict: i64 = second.query_row(
            "SELECT count(*) FROM file_matches m JOIN artifacts s ON s.id=m.source_id JOIN artifacts a ON a.id=m.artifact_id WHERE s.path=?1 AND a.path='analysis1/set1/set1.h5' AND m.direction='reads'",
            [rel], |r| r.get(0))?;
        assert_eq!(strict, 1);
        // With 51 possible relocated files, do not create an investigation
        // offering a huge extension-only candidate list.
        let crowded: HashSet<String> = (0..51)
            .map(|i| format!("data/set{i}/set{i}.h5"))
            .chain(std::iter::once(rel.to_owned()))
            .collect();
        let third = Connection::open_in_memory()?;
        schema(&third)?;
        parse(&third, &root, rel, &crowded)?;
        let suppressed: i64 = third.query_row(
            "SELECT count(*) FROM file_matches", [], |r| r.get(0))?;
        assert_eq!(suppressed, 0, "more than 50 relocated files must return no matches");
        Ok(())
    }
}

#[cfg(test)]
mod funky_match_tests {
    use super::*;

    #[test]
    fn table_driven_strict_and_funky() {
        let cases = [
            ("*/*.h5", "sample1/data.h5", true, true),
            ("*/*.h5", "data/sample1/data.h5", false, true),
            ("*/*.h5", "data/sample1/data.tsv", false, false),
            ("*/*.h5", "data.h5", false, true),
            ("results/sample_*.tsv", "archive/sample_01.tsv", false, true),
            ("results/sample_*.tsv", "archive/other.tsv", false, false),
            ("results/data.tsv", "archive/data.tsv", false, true),
        ];
        for (pattern, candidate, strict, funky) in cases {
            let mut parts = Vec::new();
            for (i, fragment) in pattern.split('*').enumerate() {
                if i > 0 { parts.push(PathPart::Wildcard); }
                if !fragment.is_empty() { parts.push(PathPart::Literal(fragment.into())); }
            }
            let path = FilePath::new(parts);
            assert_eq!(path.matches(candidate), strict, "strict: {pattern} -> {candidate}");
            assert_eq!(path.funky_match(candidate), funky, "funky: {pattern} -> {candidate}");
        }
    }

    #[test]
    fn strict_wins_and_funky_is_limited_to_50() {
        let pattern = FilePath::new(vec![PathPart::Wildcard, PathPart::Literal("/".into()),
            PathPart::Wildcard, PathPart::Literal(".h5".into())]);
        let moved: HashSet<String> = (1..=3).map(|i| format!("data/set{i}/set{i}.h5")).collect();
        assert!(matches!(pattern.find_matches("analysis1/analyze.py", &moved), FileMatches::Funky(v) if v.len()==3));
        let mut mixed = moved;
        mixed.insert("analysis1/set4/set4.h5".into());
        assert_eq!(pattern.find_matches("analysis1/analyze.py", &mixed),
            FileMatches::Strict(vec!["analysis1/set4/set4.h5".into()]));
        let fifty: HashSet<String> = (0..50).map(|i| format!("data/{i}/file.h5")).collect();
        assert!(matches!(pattern.find_matches("analysis1/analyze.py", &fifty), FileMatches::Funky(v) if v.len()==50));
        let mut fifty_one = fifty;
        fifty_one.insert("data/50/file.h5".into());
        assert_eq!(pattern.find_matches("analysis1/analyze.py", &fifty_one), FileMatches::None);
    }
    #[test]
    fn shifted_directory_keeps_full_pattern_before_basename_fallback() {
        let pattern = FilePath::new(vec![
            PathPart::Literal("results/sample_".into()), PathPart::Wildcard,
            PathPart::Literal(".tsv".into()),
        ]);
        let paths: HashSet<String> = [
            "archive/results/sample_01.tsv",
            "archive/results/sample_02.tsv",
            "other/sample_03.tsv",
            "archive/results/other.tsv",
        ].into_iter().map(str::to_owned).collect();
        assert_eq!(pattern.find_matches("analysis/analyze.py", &paths),
            FileMatches::Funky(vec![
                "archive/results/sample_01.tsv".into(),
                "archive/results/sample_02.tsv".into(),
            ]));
    }

}

#[test]
fn unified_yaml_cli_rule_parses() {
    let yaml = r#"operations:
  - name: sc-analyse
    language: bash
    subcommand: gex
    arguments:
      - {name: --data, role: input, kind: file, cardinality: many}
      - {name: --out, role: output, kind: directory, descendants: true}
"#;
    let rules: RuleSet = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(rules.operations.len(), 1);
    assert_eq!(rules.operations[0].arguments[0].cardinality.as_deref(), Some("many"));
}

#[test]
fn shell_cli_rule_links_multiple_inputs_and_output_directory() -> Result<()> {
    let db = Connection::open_in_memory()?;
    schema(&db)?;
    let paths: HashSet<String> = [
        "GSE156442/a.h5ad", "GSE156442/b.h5ad",
        "GSE156442/sc_analysis_combined/analysis.h5ad",
        "GSE156442/sc_analysis_combined/figures/umap.png",
    ].iter().map(|s| s.to_string()).collect();
    let yaml = r#"operations:
  - name: sc-analyse
    language: bash
    subcommand: gex
    arguments:
      - {name: --data, role: input, kind: file, cardinality: many}
      - {name: --sample-names, role: parameter, kind: value, cardinality: many}
      - {name: --out, role: output, kind: directory}
"#;
    let rules: RuleSet = serde_yaml::from_str(yaml)?;
    let shell = "sc-analyse gex --data a.h5ad b.h5ad --sample-names A B --out sc_analysis_combined";
    let lines = vec![(1, shell.to_owned())];
    analyze_lines(&db, "GSE156442/run.sh", &lines, &paths, true, &[], &[], &rules.operations)?;
    let reads: i64 = db.query_row("SELECT count(*) FROM file_matches WHERE direction='reads'", [], |r| r.get(0))?;
    let dirs: i64 = db.query_row("SELECT count(*) FROM file_matches WHERE direction='writes_directory'", [], |r| r.get(0))?;
    assert_eq!(reads, 2);
    assert_eq!(dirs, 1);
    let mut stmt = db.prepare("SELECT a.path FROM file_matches m JOIN artifacts a ON a.id=m.artifact_id WHERE m.direction='writes' ORDER BY a.path")?;
    let files: Vec<String> = stmt.query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    assert_eq!(files, vec![
        "GSE156442/sc_analysis_combined/analysis.h5ad",
        "GSE156442/sc_analysis_combined/figures/umap.png",
    ]);
    Ok(())
}

#[test]
fn unified_yaml_scientific_operations_and_shell_syntax() -> Result<()> {
    let rules: RuleSet = serde_yaml::from_str(include_str!("../../../rules/scientific_functions.yaml"))?;
    assert!(rules.operations.iter().any(|r| r.name == "scanpy.read_h5ad"));
    assert!(rules.operations.iter().any(|r| r.name == "Read10X"));
    let imports = python_imports("import scanpy as sc\n");
    assert!(operation_name_matches("scanpy.read_h5ad", "sc.read_h5ad", &imports, "python"));
    let (segments, redirects) = shell_pipeline("samtools view -b input.sam|samtools sort -o output.bam - 2>errors.log");
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0][0], "samtools");
    assert_eq!(segments[1][1], "sort");
    assert_eq!(redirects, vec![("2>".to_owned(), "errors.log".to_owned())]);
    let spec: OperationArgument = serde_yaml::from_str("position: 2\nthrough: last\nrole: input\nkind: file")?;
    assert_eq!(positional_bounds(&spec, 5), Some((1,5)));
    let last: OperationArgument = serde_yaml::from_str("position: last\nrole: input\nkind: file")?;
    assert_eq!(positional_bounds(&last, 5), Some((4,5)));
    Ok(())
}

#[test]
fn unified_python_r_rules_record_inputs_and_directory_outputs() -> Result<()> {
    let db = Connection::open_in_memory()?;
    schema(&db)?;
    let paths: HashSet<String> = [
        "analysis/input.h5ad", "analysis/result.h5ad",
        "analysis/tenx/matrix.mtx.gz", "analysis/tenx/barcodes.tsv.gz",
        "analysis/tenx/features.tsv.gz", "analysis/result.rds",
    ].iter().map(|s| s.to_string()).collect();
    let rules: RuleSet = serde_yaml::from_str(include_str!("../../../rules/scientific_functions.yaml"))?;
    let mut env = Environment::default();
    let mut imports = HashMap::new();
    let python = "import scanpy as sc\na = sc.read_h5ad('input.h5ad')\na.write_h5ad('result.h5ad')\nb = sc.read_10x_mtx('tenx')\n";
    analyze_source(&db,"analysis/run.py",python,1,&paths,&mut env,&[],&mut imports,&[],&rules.operations,"python")?;
    analyze_source(&db,"analysis/run.R","x <- Read10X(data.dir='tenx')\nsaveRDS(x, file='result.rds')",1,
        &paths,&mut Environment::default(),&[],&mut HashMap::new(),&[],&rules.operations,"r")?;
    let mut stmt = db.prepare("SELECT a.path, m.direction FROM file_matches m JOIN artifacts a ON a.id=m.artifact_id")?;
    let edges: Vec<(String,String)> = stmt.query_map([], |row| Ok((row.get(0)?,row.get(1)?)))?
        .collect::<std::result::Result<_,_>>()?;
    assert!(edges.iter().any(|(p,d)| p=="analysis/input.h5ad" && d=="reads"));
    assert!(edges.iter().any(|(p,d)| p=="analysis/result.h5ad" && d=="writes"));
    assert!(edges.iter().any(|(p,d)| p=="analysis/tenx" && d=="reads"));
    assert!(edges.iter().any(|(p,d)| p=="analysis/result.rds" && d=="writes"));
    Ok(())
}

#[test]
fn expected_files_restrict_directory_outputs_and_support_mid_filename_wildcards() -> Result<()> {
    let db = Connection::open_in_memory()?;
    schema(&db)?;
    let paths: HashSet<String> = [
        "project/stats/sampleA_counts.csv",
        "project/stats/sampleA_markers.csv",
        "project/stats/sampleB_counts.csv",
        "project/stats/notes.txt",
        "project/stats/nested/sampleA_nested.csv",
    ].into_iter().map(str::to_owned).collect();
    let rules: RuleSet = serde_yaml::from_str(r#"operations:
  - name: write_stats_tables
    language: python
    arguments:
      - {position: 2, role: output, kind: directory, descendants: true, expected_files: ['sampleA*.csv']}
"#)?;
    let mut env = Environment::default();
    analyze_source(&db, "project/analyze.py", "write_stats_tables(adata, 'stats')", 1,
        &paths, &mut env, &rules.functions, &mut HashMap::new(), &[], &rules.operations, "python")?;
    let mut stmt = db.prepare("SELECT a.path FROM file_matches m JOIN artifacts a ON a.id=m.artifact_id WHERE m.direction='writes' ORDER BY a.path")?;
    let written: Vec<String> = stmt.query_map([], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
    assert_eq!(written, vec!["project/stats/sampleA_counts.csv", "project/stats/sampleA_markers.csv"]);
    let broad: i64 = db.query_row("SELECT count(*) FROM file_matches WHERE direction='writes_directory'", [], |r| r.get(0))?;
    assert_eq!(broad, 0, "restricted outputs must not claim the entire directory");
    Ok(())
}

#[test]
fn operation_default_output_applies_only_when_argument_is_missing() -> Result<()> {
    let db = Connection::open_in_memory()?;
    schema(&db)?;
    let paths: HashSet<String> = ["project/top_genes.csv", "project/custom.csv"]
        .into_iter().map(str::to_owned).collect();
    let rules: RuleSet = serde_yaml::from_str(r#"operations:
  - name: write_top_genes
    language: python
    arguments:
      - {position: 2, role: output, kind: file, default: top_genes.csv}
"#)?;
    let mut env = Environment::default();
    analyze_source(&db, "project/analyze.py", "write_top_genes(adata)\nwrite_top_genes(adata, 'custom.csv')", 1,
        &paths, &mut env, &rules.functions, &mut HashMap::new(), &[], &rules.operations, "python")?;
    let mut stmt = db.prepare("SELECT a.path FROM file_matches m JOIN artifacts a ON a.id=m.artifact_id WHERE m.direction='writes' ORDER BY a.path")?;
    let written: Vec<String> = stmt.query_map([], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
    assert_eq!(written, vec!["project/custom.csv", "project/top_genes.csv"]);
    Ok(())
}

#[test]
fn expanded_shell_directory_paths_match_empty_inventory_directories() {
    let db = Connection::open_in_memory().unwrap();
    crate::core::engine::schema(&db).unwrap();
    let paths: HashSet<String> = [
        "20260220/nextflow_10X_ONT.sh",
        "20260220/TP53-capt_2",
        "20260220/TP53-capt_2_outdir",
    ].into_iter().map(str::to_owned).collect();
    let rel = "20260220/nextflow_10X_ONT.sh";
    for directory in ["20260220/TP53-capt_2", "20260220/TP53-capt_2_outdir"] {
        db.execute("INSERT INTO artifacts(path,kind) VALUES (?1,'directory')", [directory]).unwrap();
    }
    let input = LogicLinks {
        kind: LinkType::Reads,
        links: LinksToFiles { files: vec![Variable::Parts(FilePath::new(vec![PathPart::Literal("./TP53-capt_2/".into())]))] },
        expression: "$PATH_FASTQ".into(),
        function: "nextflow run [--fastq]".into(),
    };
    let output = LogicLinks {
        kind: LinkType::Writes,
        links: LinksToFiles { files: vec![Variable::Parts(FilePath::new(vec![PathPart::Literal("./TP53-capt_2_outdir/".into())]))] },
        expression: "$OUTDIR".into(),
        function: "nextflow run [--out_dir]".into(),
    };
    let count = record_logic_links(&db, rel, 34, &[input, output], &paths).unwrap();
    assert_eq!(count, 2);
    let mut stmt = db.prepare("SELECT a.path, m.direction FROM file_matches m JOIN artifacts a ON a.id=m.artifact_id ORDER BY a.path").unwrap();
    let links: Vec<(String, String)> = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(links, vec![
        ("20260220/TP53-capt_2".into(), "reads".into()),
        ("20260220/TP53-capt_2_outdir".into(), "writes_directory".into()),
    ]);
}

#[test]
fn pseudobulk_output_is_one_folder_in_wizard() -> Result<()> {
    use crate::core::history_db::{DbArchiveView, DbIssues};

    let db = Connection::open_in_memory()?;
    schema(&db)?;
    let script = "analysis/run.py";
    let folder = "analysis/pseudobulk_leiden_stats";
    let dotted_folder = "analysis/pseudobulk_leiden5_merged0.98_stats";
    let tables = [
        "analysis/pseudobulk_leiden_stats/leiden_cluster_6.csv",
        "analysis/pseudobulk_leiden_stats/nested/leiden_cluster_10.csv",
        "analysis/pseudobulk_leiden5_merged0.98_stats/leiden_cluster_6.csv",
    ];
    let paths: HashSet<String> = tables.into_iter()
        .chain([folder, dotted_folder]).map(str::to_owned).collect();
    let rules: RuleSet = serde_yaml::from_str(include_str!("../../../rules/scientific_functions.yaml"))?;

    analyze_source(
        &db, script,
        "functions.write_pseudobulk_stats_tables( adata,'pseudobulk_leiden_stats', 'leiden')",
        1, &paths, &mut Environment::default(), &rules.functions,
        &mut HashMap::new(), &rules.plotting, &rules.operations, "python",
    )?;

    analyze_source(
        &db, script,
        "functions.write_pseudobulk_stats_tables(adata, 'pseudobulk_leiden5_merged0.98_stats', 'leiden5_merged0.98')",
        2, &paths, &mut Environment::default(), &rules.functions,
        &mut HashMap::new(), &rules.plotting, &rules.operations, "python",
    )?;
    crate::core::classification::refresh(&db, std::path::Path::new("."), &paths)?;

    let wizard = db.issues(db.source_artifact_id(script)?)?;
    let outputs: Vec<_> = wizard.writes().flat_map(|group| &group.matches)
        .map(|item| (item.artifact.as_str(), item.direction.as_str(), item.role.as_deref()))
        .collect();
    assert_eq!(outputs.len(), 2);
    for path in [folder, dotted_folder] {
        assert!(outputs.contains(&(path, "writes_directory", Some("Statistical results"))));
    }

    let (_, relations, _) = db.archive_view()?;
    for table in tables {
        assert!(
            relations.iter().any(|edge| edge.source == script
                && edge.target == table && edge.kind == "writes"),
            "missing individual provenance for {table}"
        );
    }
    Ok(())
}
