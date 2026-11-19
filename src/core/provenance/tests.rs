use super::*;
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
            "SELECT target FROM relations WHERE source=?1 AND kind='reads'"
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
     let mut stmt = db.prepare("SELECT target FROM relations WHERE source='analysis.ipynb' AND kind='reads'")?;
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
        let mut env = Environment::default();
        analyze_source(&db, "analysis/export.R", "export2cellexalvr(obj, path='vr')", 1, &paths, &mut env, &rules, &mut HashMap::new(), &[], "r")?;
        analyze_source(&db, "analysis/export.py", "write_stats_tables(adata, 'stats')", 2, &paths, &mut env, &rules, &mut HashMap::new(), &[], "python")?;
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
            &[], &mut HashMap::new(), &[], language)?;
        let mut stmt = db.prepare("SELECT target, kind, evidence FROM relations WHERE source='analysis'")?;
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
        analyze_lines(&db, "analysis.sh", &lines, &paths, true, &[], &[])?;
        let count: i64 = db.query_row(
            "SELECT count(*) FROM relations WHERE kind='writes'", [], |r| r.get(0))?;
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
    fn yaml_rules_return_variables_before_inventory_matching() {
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
            &[], &mut HashMap::new(), &[], "r")?;
        let mut stmt = db.prepare("SELECT target FROM relations WHERE source='analysis' AND kind='writes'")?;
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
        let reads: i64 = db.query_row(
            "SELECT count(*) FROM relations WHERE source=?1 AND kind='reads'",
            [rel], |r| r.get(0))?;
        assert_eq!(reads, 0, "relocated files must not be silently confirmed");
        let unresolved: i64 = db.query_row(
            "SELECT count(*) FROM issues WHERE notebook=?1 AND filename='*/*.h5'",
            [rel], |r| r.get(0))?;
        assert!(unresolved > 0, "relocated wildcard must reach the normal investigation workflow");
        // Correctly located files take precedence over recovery.
        inventory.insert("analysis1/set1/set1.h5".to_owned());
        let second = Connection::open_in_memory()?;
        schema(&second)?;
        parse(&second, &root, rel, &inventory)?;
        let strict: i64 = second.query_row(
            "SELECT count(*) FROM relations WHERE source=?1 AND target='analysis1/set1/set1.h5' AND kind='reads'",
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
            "SELECT count(*) FROM issues WHERE notebook=?1 AND filename='*/*.h5'",
            [rel], |r| r.get(0))?;
        assert_eq!(suppressed, 0, "more than 50 relocated files must return nothing");
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
