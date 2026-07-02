use super::*;
use crate::parse::{
    annotate_ymls_with_format, lines_to_format_blocks, parse_task_sections, parse_varlen_rows,
    task_to_format_blocks, ArrayBlock, BoundRepr, FormatBlock, RandomTestSection, TaskSection,
    VarConstraint, VarType,
};

#[test]
fn parse_varlen_rows_with_fixed_prefixes() {
    let lines = vec![
        "P_1 C_1 F_{1,1} F_{1,2} \\ldots F_{1,C_1}".to_string(),
        "P_2 C_2 F_{2,1} F_{2,2} \\ldots F_{2,C_2}".to_string(),
        "\\vdots".to_string(),
        "P_N C_N F_{N,1} F_{N,2} \\ldots F_{N,C_N}".to_string(),
    ];
    let got = parse_varlen_rows(&lines, 0).expect("must parse");
    assert_eq!(got.0, vec!["P".to_string(), "C".to_string()]);
    assert_eq!(got.1, "c");
    assert_eq!(got.2, "F");
    assert_eq!(got.3, "n");
}

fn render_task(task: &TaskSection) -> String {
    let rt = task_to_format_blocks(task);
    render_section_from_format_blocks(&rt.format, &rt.vars).expect("render succeeds")
}

#[test]
fn render_section_handles_strictly_superior_block() {
    let task = TaskSection {
        letter: "D".to_string(),
        input_blocks: vec![vec![
            "N M".to_string(),
            "P _ 1 C _ 1 F _ {1,1} F _ {1,2} \\ldots F _ {1,C _ 1}".to_string(),
            "\\vdots".to_string(),
            "P _ N C _ N F _ {N,1} F _ {N,2} \\ldots F _ {N,C _ N}".to_string(),
        ]],
        constraints_items: vec![],
    };
    let rendered = render_task(&task);
    assert!(rendered.contains("fn main()"), "{}", rendered);
    assert!(rendered.contains("n: usize"), "{}", rendered);
    assert!(rendered.contains("m: usize"), "{}", rendered);
}

#[test]
fn render_section_handles_e_manga() {
    let task = TaskSection {
        letter: "E".to_string(),
        input_blocks: vec![vec!["N".to_string(), "a_1 \\ldots a_N".to_string()]],
        constraints_items: vec![
            "1 <= N <= 3 * 10^5".to_string(),
            "1 <= a_i <= 10^9".to_string(),
        ],
    };
    let rendered = render_task(&task);
    assert!(!rendered.contains("TODO"), "{}", rendered);
    assert!(rendered.contains("n: usize"), "{}", rendered);
    assert!(rendered.contains("a: [usize; n]"), "{}", rendered);
}

#[test]
fn render_section_handles_f_ladder() {
    let task = TaskSection {
        letter: "F".to_string(),
        input_blocks: vec![vec![
            "N".to_string(),
            "A_1 B_1".to_string(),
            "A_2 B_2".to_string(),
            "\\ldots".to_string(),
            "A_N B_N".to_string(),
        ]],
        constraints_items: vec![
            "1 <= N <= 2 * 10^5".to_string(),
            "1 <= A_i, B_i <= 10^9".to_string(),
        ],
    };
    let rendered = render_task(&task);
    assert!(!rendered.contains("TODO"), "{}", rendered);
    assert!(rendered.contains("n: usize"), "{}", rendered);
    assert!(rendered.contains("ab: [(usize, usize); n]"), "{}", rendered);
}

#[test]
fn render_section_handles_g_gravity() {
    let task = TaskSection {
        letter: "G".to_string(),
        input_blocks: vec![vec![
            "N W".to_string(),
            "X_1 Y_1".to_string(),
            "X_2 Y_2".to_string(),
            "\\vdots".to_string(),
            "X_N Y_N".to_string(),
            "Q".to_string(),
            "T_1 A_1".to_string(),
            "T_2 A_2".to_string(),
            "\\vdots".to_string(),
            "T_Q A_Q".to_string(),
        ]],
        constraints_items: vec![
            "1 <= N <= 2 * 10^5".to_string(),
            "1 <= W <= N".to_string(),
            "1 <= X_i <= W".to_string(),
            "1 <= Y_i <= 10^9".to_string(),
            "1 <= Q <= 2 * 10^5".to_string(),
            "1 <= T_j <= 10^9".to_string(),
            "1 <= A_j <= N".to_string(),
        ],
    };
    let rendered = render_task(&task);
    assert!(!rendered.contains("TODO"), "{}", rendered);
    assert!(rendered.contains("n: usize"), "{}", rendered);
    assert!(rendered.contains("w: usize"), "{}", rendered);
    assert!(rendered.contains("xy: [(usize, usize); n]"), "{}", rendered);
    assert!(rendered.contains("q: usize"), "{}", rendered);
    assert!(rendered.contains("ta: [(usize, usize); q]"), "{}", rendered);
}

#[test]
fn lines_to_format_blocks_scalar() {
    let lines = vec!["N".to_string()];
    let blocks = lines_to_format_blocks(&lines);
    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        FormatBlock::Scalars(sb) => assert_eq!(sb.vars, vec!["n".to_string()]),
        _ => panic!("expected Scalars"),
    }
}

#[test]
fn lines_to_format_blocks_array() {
    let lines = vec!["N".to_string(), "A_1 A_2 \\ldots A_N".to_string()];
    let blocks = lines_to_format_blocks(&lines);
    assert_eq!(blocks.len(), 2);
    match &blocks[1] {
        FormatBlock::Array(a) => {
            assert_eq!(a.base, "a");
            assert_eq!(a.len.as_deref(), Some("n"));
        }
        _ => panic!("expected Array"),
    }
}

#[test]
fn lines_to_format_blocks_rows() {
    let lines = vec![
        "N".to_string(),
        "x_1 y_1".to_string(),
        "\\vdots".to_string(),
        "x_N y_N".to_string(),
    ];
    let blocks = lines_to_format_blocks(&lines);
    let rows_block = blocks.iter().find_map(|b| {
        if let FormatBlock::Rows(r) = b {
            Some(r)
        } else {
            None
        }
    });
    let r = rows_block.expect("expected Rows block");
    assert_eq!(r.vars, vec!["x", "y"]);
    assert_eq!(r.len, "n");
}

#[test]
fn lines_to_format_blocks_rows_far_closing_line() {
    // Closing row sits 15 ellipsis lines below the opening row — beyond the
    // old hardcoded `idx + 12` lookahead cap. The structural break must still
    // resolve it correctly.
    let mut lines = vec!["N".to_string(), "x_1 y_1".to_string()];
    for _ in 0..15 {
        lines.push("\\vdots".to_string());
    }
    lines.push("x_N y_N".to_string());
    let blocks = lines_to_format_blocks(&lines);
    let r = blocks
        .iter()
        .find_map(|b| {
            if let FormatBlock::Rows(r) = b {
                Some(r)
            } else {
                None
            }
        })
        .expect("expected Rows block resolved past 12-line cap");
    assert_eq!(r.vars, vec!["x", "y"]);
    assert_eq!(r.len, "n");
}

#[test]
fn format_blocks_roundtrip_yml() {
    let blocks = vec![
        FormatBlock::Scalars(crate::parse::ScalarsBlock {
            vars: vec!["n".to_string(), "m".to_string()],
        }),
        FormatBlock::Array(ArrayBlock {
            base: "a".to_string(),
            len: Some("n".to_string()),
            height: None,
            count: None,
            jagged: false,
        }),
    ];
    let section = RandomTestSection {
        format: blocks,
        ..Default::default()
    };

    #[derive(serde::Serialize)]
    struct W<'a> {
        random_test: &'a RandomTestSection,
    }
    let yaml = serde_yaml::to_string(&W {
        random_test: &section,
    })
    .unwrap();
    // Must contain "random_test:" key
    assert!(yaml.contains("random_test:"));
    // Parse back
    let val: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
    let back: RandomTestSection = serde_yaml::from_value(val["random_test"].clone()).unwrap();
    assert_eq!(back.format.len(), 2);
}

#[test]
fn format_blocks_to_guess_result_scalar_and_array() {
    let vars = std::collections::BTreeMap::new();
    let blocks = vec![
        FormatBlock::Scalars(crate::parse::ScalarsBlock {
            vars: vec!["n".to_string()],
        }),
        FormatBlock::Array(ArrayBlock {
            base: "a".to_string(),
            len: Some("n".to_string()),
            height: None,
            count: None,
            jagged: false,
        }),
    ];
    let result = format_blocks_to_guess_result(&blocks, &vars);
    assert!(result.decls.iter().any(|d| d.contains("n: usize")));
    assert!(result.decls.iter().any(|d| d.contains("a: [usize; n]")));
}

#[test]
fn format_blocks_str_var_uses_chars() {
    // Single-string Chars (row 2 "Chars 単独") flows as Scalars + vars[s].type=Chars.
    // ArrayBlock is NOT used for a lone Chars variable.
    let mut vars = std::collections::BTreeMap::new();
    vars.insert(
        "s".to_string(),
        VarConstraint {
            r#type: VarType::Chars,
            len: Some(BoundRepr::Expr("17".into())),
            ..Default::default()
        },
    );
    let blocks = vec![
        FormatBlock::Scalars(crate::parse::ScalarsBlock {
            vars: vec!["n".to_string()],
        }),
        FormatBlock::Scalars(crate::parse::ScalarsBlock {
            vars: vec!["s".to_string()],
        }),
    ];
    let result = format_blocks_to_guess_result(&blocks, &vars);
    assert!(
        result.decls.iter().any(|d| d.contains("s: Chars,")),
        "single Chars var should be rendered as `s: Chars,`, got: {:?}",
        result.decls
    );
    let rendered = render_section_from_format_blocks(&blocks, &vars).unwrap();
    assert!(
        rendered.contains("use proconio::{input, fastout, marker::Chars};"),
        "rendered template should import Chars marker. got:\n{}",
        rendered,
    );
}

#[test]
fn format_blocks_jagged_outputs_proconio_jagged_syntax() {
    let vars = std::collections::BTreeMap::new();
    let blocks = vec![FormatBlock::Array(ArrayBlock {
        base: "a".to_string(),
        len: Some("l".to_string()),
        height: None,
        count: Some("n".to_string()),
        jagged: true,
    })];
    let result = format_blocks_to_guess_result(&blocks, &vars);
    assert!(
        result.decls.iter().any(|d| d.contains("a: [[usize]; n],")),
        "jagged array should emit proconio jagged syntax `[[usize]; n]`, got: {:?}",
        result.decls
    );
}

#[test]
fn format_blocks_chars_grid_2d() {
    let mut vars = std::collections::BTreeMap::new();
    vars.insert(
        "s".to_string(),
        VarConstraint {
            r#type: VarType::Chars,
            ..Default::default()
        },
    );
    let blocks = vec![FormatBlock::Array(ArrayBlock {
        base: "s".to_string(),
        len: None,
        height: None,
        count: Some("h".to_string()),
        jagged: false,
    })];
    let result = format_blocks_to_guess_result(&blocks, &vars);
    assert!(
        result.decls.iter().any(|d| d.contains("s: [Chars; h],")),
        "Chars 2D grid should be `[Chars; h]` with no inner width, got: {:?}",
        result.decls
    );
}

#[test]
fn format_blocks_chars_3d() {
    let mut vars = std::collections::BTreeMap::new();
    vars.insert(
        "s".to_string(),
        VarConstraint {
            r#type: VarType::Chars,
            ..Default::default()
        },
    );
    let blocks = vec![FormatBlock::Array(ArrayBlock {
        base: "s".to_string(),
        len: None,
        height: Some("h".to_string()),
        count: Some("f".to_string()),
        jagged: false,
    })];
    let result = format_blocks_to_guess_result(&blocks, &vars);
    assert!(
        result
            .decls
            .iter()
            .any(|d| d.contains("s: [[Chars; h]; f],")),
        "Chars 3D should be `[[Chars; h]; f]` (no inner w in the type), got: {:?}",
        result.decls
    );
}

#[test]
fn format_blocks_int_matrix_2d_uses_len_as_inner() {
    let vars = std::collections::BTreeMap::new();
    let blocks = vec![FormatBlock::Array(ArrayBlock {
        base: "a".to_string(),
        len: Some("w".to_string()),
        height: None,
        count: Some("h".to_string()),
        jagged: false,
    })];
    let result = format_blocks_to_guess_result(&blocks, &vars);
    assert!(
        result
            .decls
            .iter()
            .any(|d| d.contains("a: [[usize; w]; h],")),
        "int 2D matrix should be `[[usize; w]; h]`, got: {:?}",
        result.decls
    );
}

#[test]
fn format_blocks_int_3d() {
    let vars = std::collections::BTreeMap::new();
    let blocks = vec![FormatBlock::Array(ArrayBlock {
        base: "a".to_string(),
        len: Some("w".to_string()),
        height: Some("h".to_string()),
        count: Some("f".to_string()),
        jagged: false,
    })];
    let result = format_blocks_to_guess_result(&blocks, &vars);
    assert!(
        result
            .decls
            .iter()
            .any(|d| d.contains("a: [[[usize; w]; h]; f],")),
        "int 3D should be `[[[usize; w]; h]; f]`, got: {:?}",
        result.decls
    );
}

#[test]
fn render_queries_with_chars_branch_imports_chars() {
    use crate::parse::{QueriesBlock, QueryBranch, ScalarsBlock};

    let mut vars = std::collections::BTreeMap::new();
    vars.insert("q".to_string(), VarConstraint::default());
    vars.insert(
        "s".to_string(),
        VarConstraint {
            r#type: VarType::Chars,
            ..Default::default()
        },
    );
    vars.insert("k".to_string(), VarConstraint::default());

    // Outer: q queries, each query is one of 2 types:
    //   type 1: read S (Chars)
    //   type 2: read K (usize)
    let queries = QueriesBlock {
        count: "q".to_string(),
        discriminator: Some("qt".to_string()),
        types: vec![
            QueryBranch {
                id: "1".to_string(),
                format: vec![FormatBlock::Scalars(ScalarsBlock {
                    vars: vec!["s".to_string()],
                })],
            },
            QueryBranch {
                id: "2".to_string(),
                format: vec![FormatBlock::Scalars(ScalarsBlock {
                    vars: vec!["k".to_string()],
                })],
            },
        ],
    };
    let blocks = vec![
        FormatBlock::Scalars(ScalarsBlock {
            vars: vec!["q".to_string()],
        }),
        FormatBlock::Queries(queries),
    ];
    let rendered = render_section_from_format_blocks(&blocks, &vars).unwrap();
    assert!(
        rendered.contains("use proconio::{input, fastout, marker::Chars};"),
        "queries-with-Chars rendering should import Chars marker. got:\n{}",
        rendered,
    );
}

#[test]
fn annotate_and_generate_abc450_a() {
    use crate::shell::Shell;
    use camino::Utf8PathBuf;

    let contest_dir = Utf8PathBuf::from("/workspaces/atcoder-rust-devcontainer/src/contest/abc450");
    if !contest_dir.join("task.html").exists() {
        return; // skip in CI without the files
    }

    let yml_path = contest_dir.join("testcases").join("a.yml");

    // Read original yml
    let original_yml = std::fs::read_to_string(&yml_path).unwrap();

    // Annotate
    let mut shell = Shell::new();
    annotate_ymls_with_format(&contest_dir, &[yml_path.clone()], &mut shell).unwrap();

    let annotated = std::fs::read_to_string(&yml_path).unwrap();
    assert!(
        annotated.contains("random_test:"),
        "random_test: key not found in annotated yml"
    );
    assert!(
        annotated.contains("format:"),
        "format: key not found in annotated yml"
    );
    assert!(
        annotated.contains("scalars:"),
        "scalars block not found in annotated yml"
    );

    // Now generate template from yml
    let mut shell2 = Shell::new();
    let templates = generate_template(&contest_dir, &mut shell2)
        .unwrap()
        .unwrap();
    let src_path = contest_dir.join("src").join("bin").join("a.rs");
    let content = templates.get(&src_path).expect("a.rs template missing");
    assert!(content.contains("use proconio::{input, fastout};"));
    assert!(content.contains("n: usize"));
    assert!(content.contains("fn main()"));

    // Restore original yml (clean up)
    std::fs::write(&yml_path, original_yml).unwrap();
}

#[test]
fn compare_old_new_generation_for_all_contests() {
    use crate::shell::Shell;
    use camino::Utf8PathBuf;

    let contests = [
        "abc440", "abc441", "abc442", "abc443", "abc444", "abc445", "abc450", "abc452", "abc453",
        "abc455", "abc456",
    ];
    let base = Utf8PathBuf::from("/workspaces/atcoder-rust-devcontainer/src/contest");

    for contest in &contests {
        let contest_dir = base.join(contest);
        if !contest_dir.join("task.html").exists() {
            continue;
        }

        // Collect ymls
        let testcases_dir = contest_dir.join("testcases");
        let yml_paths: Vec<Utf8PathBuf> = ('a'..='g')
            .map(|c| testcases_dir.join(c.to_string()).with_extension("yml"))
            .filter(|p| p.exists())
            .collect();

        if yml_paths.is_empty() {
            continue;
        }

        // Backup ymls
        let backups: Vec<(Utf8PathBuf, String)> = yml_paths
            .iter()
            .map(|p| (p.clone(), std::fs::read_to_string(p).unwrap()))
            .collect();

        // Generate OLD templates (before annotation)
        let mut shell_old = Shell::new();
        let old_templates = match generate_template(&contest_dir, &mut shell_old) {
            Ok(Some(t)) => t,
            _ => {
                for (p, content) in &backups {
                    std::fs::write(p, content).unwrap();
                }
                continue;
            }
        };

        // Annotate ymls
        let mut shell_ann = Shell::new();
        annotate_ymls_with_format(&contest_dir, &yml_paths, &mut shell_ann).unwrap();

        // Generate NEW templates (from yml)
        let mut shell_new = Shell::new();
        let new_templates = match generate_template(&contest_dir, &mut shell_new) {
            Ok(Some(t)) => t,
            _ => {
                for (p, content) in &backups {
                    std::fs::write(p, content).unwrap();
                }
                panic!("{}: new generate_template failed", contest);
            }
        };

        // Compare
        for (path, old_content) in &old_templates {
            if let Some(new_content) = new_templates.get(path) {
                if old_content != new_content {
                    let letter = path.file_stem().unwrap_or("?");
                    println!("[{}] {}-{}: content differs", contest, contest, letter);
                    println!("  OLD:\n{}", old_content);
                    println!("  NEW:\n{}", new_content);
                }
                // Both should be valid Rust skeletons
                assert!(
                    new_content.contains("fn main()"),
                    "{}: {}: new template missing fn main()",
                    contest,
                    path
                );
                assert!(
                    new_content.contains("input!"),
                    "{}: {}: new template missing input!",
                    contest,
                    path
                );
            }
        }

        // Restore ymls
        for (p, content) in &backups {
            std::fs::write(p, content).unwrap();
        }
    }
}

#[test]
fn print_comparison_report() {
    use crate::parse::{annotate_ymls_with_format, task_to_format_blocks};
    use crate::shell::Shell;
    use camino::Utf8PathBuf;

    let contests = [
        "abc440", "abc441", "abc442", "abc443", "abc444", "abc445", "abc450", "abc452", "abc453",
        "abc455", "abc456",
    ];
    let base = Utf8PathBuf::from("/workspaces/atcoder-rust-devcontainer/src/contest");

    for contest in &contests {
        let contest_dir = base.join(contest);
        if !contest_dir.join("task.html").exists() {
            continue;
        }

        let html = std::fs::read_to_string(contest_dir.join("task.html")).unwrap();
        let sections = parse_task_sections(&html);

        let testcases_dir = contest_dir.join("testcases");
        let yml_paths: Vec<Utf8PathBuf> = ('a'..='g')
            .map(|c| testcases_dir.join(c.to_string()).with_extension("yml"))
            .filter(|p| p.exists())
            .collect();

        // Backup
        let backups: Vec<(Utf8PathBuf, String)> = yml_paths
            .iter()
            .map(|p| (p.clone(), std::fs::read_to_string(p).unwrap()))
            .collect();

        // OLD: direct from HTML via task_to_format_blocks + render
        println!("\n═══════════════════════════════════════");
        println!("  {}", contest);
        println!("═══════════════════════════════════════");

        for task in &sections {
            let letter = task.letter.to_ascii_lowercase();
            let rt_old = task_to_format_blocks(task);
            let old_rendered = render_section_from_format_blocks(&rt_old.format, &rt_old.vars);

            print!("[{}] OLD → ", letter);
            match &old_rendered {
                Ok(s) => {
                    let decls: Vec<&str> = s
                        .lines()
                        .filter(|l| {
                            l.contains(": ")
                                && !l.contains("fn main")
                                && !l.contains("use ")
                                && !l.contains("for ")
                                && !l.contains("match ")
                                && !l.contains("input!")
                                && !l.contains("/*")
                        })
                        .collect();
                    if decls.is_empty() {
                        // show input! lines
                        let inputs: Vec<&str> = s
                            .lines()
                            .filter(|l| l.trim().starts_with("input!"))
                            .collect();
                        println!("{}", inputs.join("; "));
                    } else {
                        println!(
                            "{}",
                            decls
                                .join(", ")
                                .replace("        ", "")
                                .replace(",  ", ", ")
                        );
                    }
                }
                Err(e) => println!("ERR: {e}"),
            }
            // skipped
            if !rt_old.skipped.is_empty() {
                println!("     SKIPPED: {:?}", rt_old.skipped);
            }
        }

        // Annotate ymls
        let mut shell = Shell::new();
        annotate_ymls_with_format(&contest_dir, &yml_paths, &mut shell).unwrap();

        // NEW: from annotated ymls
        let mut shell2 = Shell::new();
        if let Ok(Some(new_templates)) = generate_template(&contest_dir, &mut shell2) {
            for task in &sections {
                let letter = task.letter.to_ascii_lowercase();
                let src_path = contest_dir
                    .join("src")
                    .join("bin")
                    .join(format!("{}.rs", letter));
                if let Some(new_content) = new_templates.get(&src_path) {
                    let rt_old = task_to_format_blocks(task);
                    let old_rendered =
                        render_section_from_format_blocks(&rt_old.format, &rt_old.vars)
                            .unwrap_or_default();
                    if old_rendered == *new_content {
                        println!("[{}] NEW → same as OLD ✓", letter);
                    } else {
                        println!("[{}] NEW → DIFFERS:", letter);
                        for line in new_content.lines() {
                            println!("     {}", line);
                        }
                    }
                }
            }
        }

        // Restore
        for (p, content) in &backups {
            std::fs::write(p, content).unwrap();
        }
    }
}
