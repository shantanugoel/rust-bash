use crate::commands::{CommandContext, CommandResult, VirtualCommand};
use jaq_core::load::{Arena, File, Loader};
use jaq_core::{Ctx, Vars, data, unwrap_valr};
use jaq_json::Val;
use std::path::PathBuf;

pub struct JqCommand;

impl VirtualCommand for JqCommand {
    fn name(&self) -> &str {
        "jq"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        match execute_jq(args, ctx) {
            Ok(result) => result,
            Err(result) => result,
        }
    }
}

#[derive(Default)]
struct JqOptions {
    raw_output: bool,
    compact_output: bool,
    sort_keys: bool,
    join_output: bool,
    exit_status: bool,
    null_input: bool,
    raw_input: bool,
    slurp: bool,
    variables: Vec<(String, Val)>,
}

fn execute_jq(args: &[String], ctx: &CommandContext) -> Result<CommandResult, CommandResult> {
    let (opts, filter_str, files) = parse_args(args)?;

    // Compile the filter
    let var_names: Vec<String> = opts
        .variables
        .iter()
        .map(|(n, _)| format!("${n}"))
        .collect();
    let filter = compile_filter(&filter_str, &var_names)?;

    // Get input values
    let inputs = get_inputs(&files, &opts, ctx)?;

    // Run filter on each input
    let mut outputs: Vec<Val> = Vec::new();
    let mut stderr = String::new();
    let mut had_error = false;

    for input in inputs {
        let var_vals: Vec<Val> = opts.variables.iter().map(|(_, v)| v.clone()).collect();
        let vars = Vars::new(var_vals);
        let run_ctx = Ctx::<data::JustLut<Val>>::new(&filter.lut, vars);
        let results: Vec<_> = filter.id.run((run_ctx, input)).collect();
        for result in results {
            match unwrap_valr(result) {
                Ok(val) => outputs.push(val),
                Err(err) => {
                    stderr.push_str(&format!("jq: error: {err}\n"));
                    had_error = true;
                }
            }
        }
    }

    // Format outputs
    let stdout = format_outputs(&outputs, &opts);

    // Determine exit code
    let exit_code = if had_error {
        5
    } else if opts.exit_status {
        match outputs.last() {
            Some(Val::Bool(false)) | Some(Val::Null) => 1,
            None => 4,
            _ => 0,
        }
    } else {
        0
    };

    Ok(CommandResult {
        stdout,
        stderr,
        exit_code,
    })
}

fn parse_args(args: &[String]) -> Result<(JqOptions, String, Vec<String>), CommandResult> {
    let mut opts = JqOptions::default();
    let mut filter: Option<String> = None;
    let mut files = Vec::new();
    let mut i = 0;
    let mut end_of_opts = false;

    while i < args.len() {
        let arg = &args[i];

        if end_of_opts || !arg.starts_with('-') || arg == "-" {
            if filter.is_none() {
                filter = Some(arg.clone());
            } else {
                files.push(arg.clone());
            }
            i += 1;
            continue;
        }

        if arg == "--" {
            end_of_opts = true;
            i += 1;
            continue;
        }

        match arg.as_str() {
            "-r" | "--raw-output" => opts.raw_output = true,
            "-c" | "--compact-output" => opts.compact_output = true,
            "-S" | "--sort-keys" => opts.sort_keys = true,
            "-j" | "--join-output" => opts.join_output = true,
            "-e" | "--exit-status" => opts.exit_status = true,
            "-n" | "--null-input" => opts.null_input = true,
            "-R" | "--raw-input" => opts.raw_input = true,
            "-s" | "--slurp" => opts.slurp = true,
            "--arg" => {
                if i + 2 >= args.len() {
                    return Err(CommandResult {
                        stderr: "jq: --arg requires NAME VALUE\n".to_string(),
                        exit_code: 2,
                        ..Default::default()
                    });
                }
                let name = args[i + 1].clone();
                let value = Val::from(args[i + 2].clone());
                opts.variables.push((name, value));
                i += 2;
            }
            "--argjson" => {
                if i + 2 >= args.len() {
                    return Err(CommandResult {
                        stderr: "jq: --argjson requires NAME VALUE\n".to_string(),
                        exit_code: 2,
                        ..Default::default()
                    });
                }
                let name = args[i + 1].clone();
                let json_str = &args[i + 2];
                match jaq_json::read::parse_single(json_str.as_bytes()) {
                    Ok(val) => opts.variables.push((name, val)),
                    Err(e) => {
                        return Err(CommandResult {
                            stderr: format!("jq: invalid JSON for --argjson {name}: {e}\n"),
                            exit_code: 2,
                            ..Default::default()
                        });
                    }
                }
                i += 2;
            }
            _ => {
                // Try parsing combined short flags (e.g. -rc, -Scr)
                if arg.starts_with('-') && !arg.starts_with("--") && arg.len() > 1 {
                    let mut valid = true;
                    for ch in arg[1..].chars() {
                        match ch {
                            'r' | 'c' | 'S' | 'j' | 'e' | 'n' | 'R' | 's' => {}
                            _ => {
                                valid = false;
                                break;
                            }
                        }
                    }
                    if valid {
                        for ch in arg[1..].chars() {
                            match ch {
                                'r' => opts.raw_output = true,
                                'c' => opts.compact_output = true,
                                'S' => opts.sort_keys = true,
                                'j' => opts.join_output = true,
                                'e' => opts.exit_status = true,
                                'n' => opts.null_input = true,
                                'R' => opts.raw_input = true,
                                's' => opts.slurp = true,
                                _ => unreachable!(),
                            }
                        }
                    } else {
                        return Err(CommandResult {
                            stderr: format!("jq: Unknown option: {arg}\n"),
                            exit_code: 2,
                            ..Default::default()
                        });
                    }
                } else {
                    return Err(CommandResult {
                        stderr: format!("jq: Unknown option: {arg}\n"),
                        exit_code: 2,
                        ..Default::default()
                    });
                }
            }
        }
        i += 1;
    }

    let filter = filter.ok_or_else(|| CommandResult {
        stderr: "jq: no filter provided\n".to_string(),
        exit_code: 2,
        ..Default::default()
    })?;

    Ok((opts, filter, files))
}

fn compile_filter(
    filter_str: &str,
    var_names: &[String],
) -> Result<jaq_core::compile::Filter<jaq_core::Native<data::JustLut<Val>>>, CommandResult> {
    let defs = jaq_core::defs()
        .chain(jaq_std::defs())
        .chain(jaq_json::defs());
    let funs = jaq_core::funs()
        .chain(jaq_std::funs())
        .chain(jaq_json::funs());

    let loader = Loader::new(defs);
    let arena = Arena::default();
    let program = File {
        code: filter_str,
        path: (),
    };

    let modules = loader.load(&arena, program).map_err(|errs| {
        let msg = format_load_errors(&errs);
        CommandResult {
            stderr: format!("jq: compile error: {msg}\n"),
            exit_code: 3,
            ..Default::default()
        }
    })?;

    let mut compiler = jaq_core::Compiler::default().with_funs(funs);
    if !var_names.is_empty() {
        compiler = compiler.with_global_vars(var_names.iter().map(|v| v.as_str()));
    }

    compiler.compile(modules).map_err(|errs| {
        let msg = errs
            .into_iter()
            .map(|(file, undefs)| {
                let details: Vec<String> = undefs
                    .into_iter()
                    .map(|(name, kind)| format!("undefined {kind:?} '{name}'"))
                    .collect();
                format!("{}: {}", file.code, details.join(", "))
            })
            .collect::<Vec<_>>()
            .join("\n");
        CommandResult {
            stderr: format!("jq: compile error: {msg}\n"),
            exit_code: 3,
            ..Default::default()
        }
    })
}

fn format_load_errors(errs: &[(File<&str, ()>, jaq_core::load::Error<&str>)]) -> String {
    use jaq_core::load::Error;
    errs.iter()
        .map(|(file, err)| {
            let detail = match err {
                Error::Io(ios) => ios
                    .iter()
                    .map(|(path, msg)| format!("{path}: {msg}"))
                    .collect::<Vec<_>>()
                    .join("; "),
                Error::Lex(lex_errs) => format!("{} lex error(s)", lex_errs.len()),
                Error::Parse(parse_errs) => format!("{} parse error(s)", parse_errs.len()),
            };
            format!("{}: {detail}", file.code)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn get_inputs(
    files: &[String],
    opts: &JqOptions,
    ctx: &CommandContext,
) -> Result<Vec<Val>, CommandResult> {
    if opts.null_input {
        return Ok(vec![Val::Null]);
    }

    // Collect raw text sources (process files independently for correct semantics)
    let mut raw_texts: Vec<String> = Vec::new();

    if files.is_empty() {
        raw_texts.push(ctx.stdin.to_string());
    } else {
        for file in files {
            if file == "-" {
                raw_texts.push(ctx.stdin.to_string());
            } else {
                let path = resolve_path(file, ctx.cwd);
                match ctx.fs.read_file(&path) {
                    Ok(bytes) => {
                        raw_texts.push(String::from_utf8_lossy(&bytes).to_string());
                    }
                    Err(e) => {
                        return Err(CommandResult {
                            stderr: format!("jq: {file}: {e}\n"),
                            exit_code: 2,
                            ..Default::default()
                        });
                    }
                }
            }
        }
    }

    let mut all_vals: Vec<Val> = Vec::new();

    for raw_text in &raw_texts {
        if opts.raw_input {
            for line in raw_text.lines() {
                all_vals.push(Val::from(line.to_string()));
            }
        } else {
            let vals: Vec<Val> = jaq_json::read::parse_many(raw_text.as_bytes())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| CommandResult {
                    stderr: format!("jq: parse error (Invalid JSON): {e}\n"),
                    exit_code: 2,
                    ..Default::default()
                })?;

            if vals.is_empty() && !raw_text.trim().is_empty() {
                return Err(CommandResult {
                    stderr: "jq: parse error (Invalid JSON)\n".to_string(),
                    exit_code: 2,
                    ..Default::default()
                });
            }

            all_vals.extend(vals);
        }
    }

    if opts.slurp {
        Ok(vec![all_vals.into_iter().collect::<Val>()])
    } else {
        Ok(all_vals)
    }
}

fn format_outputs(outputs: &[Val], opts: &JqOptions) -> String {
    let mut result = String::new();
    for val in outputs {
        let formatted = format_single_val(val, opts);
        result.push_str(&formatted);
        if !opts.join_output {
            result.push('\n');
        }
    }
    result
}

fn format_single_val(val: &Val, opts: &JqOptions) -> String {
    if (opts.raw_output || opts.join_output)
        && let Some(bytes) = val_string_bytes(val)
    {
        return String::from_utf8_lossy(bytes).to_string();
    }

    let json = val_to_serde(val, opts.sort_keys);
    if opts.compact_output {
        serde_json::to_string(&json).unwrap_or_else(|_| format!("{val}"))
    } else {
        serde_json::to_string_pretty(&json).unwrap_or_else(|_| format!("{val}"))
    }
}

fn val_to_serde(val: &Val, sort_keys: bool) -> serde_json::Value {
    match val {
        Val::Null => serde_json::Value::Null,
        Val::Bool(b) => serde_json::Value::Bool(*b),
        Val::Num(n) => {
            let s = format!("{n}");
            if let Ok(i) = s.parse::<i64>() {
                serde_json::Value::Number(i.into())
            } else if let Ok(f) = s.parse::<f64>() {
                serde_json::Number::from_f64(f)
                    .map(serde_json::Value::Number)
                    .unwrap_or_else(|| {
                        // NaN/Infinity: fall back to serde_json parser or null
                        serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)
                    })
            } else {
                // BigInt/Dec: try serde_json's own parser to preserve precision
                serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)
            }
        }
        Val::TStr(s) | Val::BStr(s) => {
            serde_json::Value::String(String::from_utf8_lossy(s).to_string())
        }
        Val::Arr(arr) => {
            serde_json::Value::Array(arr.iter().map(|v| val_to_serde(v, sort_keys)).collect())
        }
        Val::Obj(map) => {
            let entries: Vec<(String, serde_json::Value)> = map
                .iter()
                .map(|(k, v)| {
                    let key = key_to_string(k);
                    (key, val_to_serde(v, sort_keys))
                })
                .collect();
            if sort_keys {
                let mut sorted = entries;
                sorted.sort_by(|a, b| a.0.cmp(&b.0));
                serde_json::Value::Object(sorted.into_iter().collect())
            } else {
                serde_json::Value::Object(entries.into_iter().collect())
            }
        }
    }
}

fn key_to_string(val: &Val) -> String {
    match val {
        Val::TStr(s) | Val::BStr(s) => String::from_utf8_lossy(s).to_string(),
        other => format!("{other}"),
    }
}

fn val_string_bytes(val: &Val) -> Option<&[u8]> {
    match val {
        Val::TStr(s) | Val::BStr(s) => Some(s),
        _ => None,
    }
}

fn resolve_path(path_str: &str, cwd: &str) -> PathBuf {
    if path_str.starts_with('/') {
        PathBuf::from(path_str)
    } else {
        PathBuf::from(cwd).join(path_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::CommandContext;
    use crate::interpreter::ExecutionLimits;
    use crate::network::NetworkPolicy;
    use crate::vfs::{InMemoryFs, VirtualFs};
    use std::collections::HashMap;

    fn run_jq(args: &[&str], stdin: &str) -> CommandResult {
        let fs = InMemoryFs::new();
        run_jq_with_fs(args, stdin, &fs)
    }

    fn run_jq_with_fs(args: &[&str], stdin: &str, fs: &InMemoryFs) -> CommandResult {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let env = HashMap::new();
        let limits = ExecutionLimits::default();
        let ctx = CommandContext {
            fs,
            cwd: "/",
            env: &env,
            stdin,
            limits: &limits,
            network_policy: &NetworkPolicy::default(),
            exec: None,
        };
        JqCommand.execute(&args, &ctx)
    }

    #[test]
    fn field_access() {
        let result = run_jq(&[".name"], r#"{"name": "alice"}"#);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), r#""alice""#);
    }

    #[test]
    fn nested_field_access() {
        let result = run_jq(&[".a.b"], r#"{"a": {"b": 42}}"#);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "42");
    }

    #[test]
    fn array_iterate_with_field() {
        let input = r#"[{"id": 1}, {"id": 2}, {"id": 3}]"#;
        let result = run_jq(&[".[] | .id"], input);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "1\n2\n3\n");
    }

    #[test]
    fn select_filter() {
        let input = r#"[{"age": 25}, {"age": 35}, {"age": 40}]"#;
        let result = run_jq(&["-c", ".[] | select(.age > 30)"], input);
        assert_eq!(result.exit_code, 0);
        let lines: Vec<&str> = result.stdout.trim().split('\n').collect();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn map_transform() {
        let input = r#"[{"name": "alice"}, {"name": "bob"}]"#;
        let result = run_jq(&["map(.name)"], input);
        assert_eq!(result.exit_code, 0);
        let parsed: serde_json::Value = serde_json::from_str(result.stdout.trim()).unwrap();
        assert_eq!(parsed, serde_json::json!(["alice", "bob"]));
    }

    #[test]
    fn raw_output() {
        let result = run_jq(&["-r", ".x"], r#"{"x": "hello"}"#);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "hello");
    }

    #[test]
    fn compact_output() {
        let input = r#"{"a": 1, "b": 2}"#;
        let result = run_jq(&["-c", "."], input);
        assert_eq!(result.exit_code, 0);
        let line = result.stdout.trim();
        assert!(!line.contains('\n'));
        assert!(line.contains("\"a\":1") || line.contains("\"a\": 1"));
    }

    #[test]
    fn slurp_mode() {
        let input = "1\n2\n3";
        let result = run_jq(&["-s", "."], input);
        assert_eq!(result.exit_code, 0);
        let parsed: serde_json::Value = serde_json::from_str(result.stdout.trim()).unwrap();
        assert_eq!(parsed, serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn arg_variable_injection() {
        let result = run_jq(&["--arg", "name", "alice", "$name"], "null");
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), r#""alice""#);
    }

    #[test]
    fn argjson_variable_injection() {
        let result = run_jq(&["--argjson", "val", "42", "$val"], "null");
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "42");
    }

    #[test]
    fn null_input() {
        let result = run_jq(&["-n", "null"], "");
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "null");
    }

    #[test]
    fn pipe_chain() {
        let input = r#"{"users": [{"name": "a", "active": true}, {"name": "b", "active": false}, {"name": "c", "active": true}]}"#;
        let result = run_jq(&[".users | map(select(.active)) | length"], input);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "2");
    }

    #[test]
    fn invalid_json_input() {
        let result = run_jq(&["."], "not json at all");
        assert_eq!(result.exit_code, 2);
        assert!(!result.stderr.is_empty());
    }

    #[test]
    fn invalid_filter() {
        let result = run_jq(&[".[invalid filter!!!"], r#"{"a":1}"#);
        assert_eq!(result.exit_code, 3);
        assert!(!result.stderr.is_empty());
    }

    #[test]
    fn keys_filter() {
        let input = r#"{"b": 2, "a": 1}"#;
        let result = run_jq(&["keys"], input);
        assert_eq!(result.exit_code, 0);
        let parsed: serde_json::Value = serde_json::from_str(result.stdout.trim()).unwrap();
        let arr = parsed.as_array().unwrap();
        assert!(arr.contains(&serde_json::json!("a")));
        assert!(arr.contains(&serde_json::json!("b")));
    }

    #[test]
    fn length_filter() {
        let result = run_jq(&["length"], r#"[1, 2, 3]"#);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "3");
    }

    #[test]
    fn type_filter() {
        let result = run_jq(&["type"], r#""hello""#);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), r#""string""#);
    }

    #[test]
    fn sort_keys_output() {
        let input = r#"{"c": 3, "a": 1, "b": 2}"#;
        let result = run_jq(&["-S", "."], input);
        assert_eq!(result.exit_code, 0);
        let output = result.stdout.trim();
        let a_pos = output.find("\"a\"").unwrap();
        let b_pos = output.find("\"b\"").unwrap();
        let c_pos = output.find("\"c\"").unwrap();
        assert!(a_pos < b_pos);
        assert!(b_pos < c_pos);
    }

    #[test]
    fn exit_status_false() {
        let result = run_jq(&["-e", ".x"], r#"{"x": false}"#);
        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn exit_status_null() {
        let result = run_jq(&["-e", ".x"], r#"{"x": null}"#);
        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn exit_status_truthy() {
        let result = run_jq(&["-e", ".x"], r#"{"x": 42}"#);
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn join_output() {
        let input = r#"["a", "b", "c"]"#;
        let result = run_jq(&["-j", ".[]"], input);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "abc");
    }

    #[test]
    fn raw_input_mode() {
        let result = run_jq(&["-R", "."], "line1\nline2");
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "\"line1\"\n\"line2\"\n");
    }

    #[test]
    fn multiple_inputs() {
        let input = "{\"a\":1}\n{\"b\":2}";
        let result = run_jq(&["."], input);
        assert_eq!(result.exit_code, 0);
        let lines: Vec<&str> = result.stdout.trim().split('\n').collect();
        assert!(lines.len() >= 2);
    }

    #[test]
    fn read_from_vfs_file() {
        let fs = InMemoryFs::new();
        fs.write_file(&PathBuf::from("/data.json"), br#"{"key": "value"}"#)
            .unwrap();
        let result = run_jq_with_fs(&[".key", "/data.json"], "", &fs);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), r#""value""#);
    }

    #[test]
    fn combined_short_flags() {
        let result = run_jq(&["-rc", ".x"], r#"{"x": "hello"}"#);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "hello");
    }

    #[test]
    fn identity_filter() {
        let input = r#"{"a": 1}"#;
        let result = run_jq(&["-c", "."], input);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), r#"{"a":1}"#);
    }

    #[test]
    fn array_index() {
        let result = run_jq(&[".[1]"], r#"[10, 20, 30]"#);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "20");
    }

    #[test]
    fn if_then_else() {
        let result = run_jq(&["if . > 5 then \"big\" else \"small\" end"], "10");
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), r#""big""#);
    }

    #[test]
    fn values_filter() {
        let input = r#"{"a": 1, "b": 2}"#;
        let result = run_jq(&["[.[] ] | sort"], input);
        assert_eq!(result.exit_code, 0);
        let parsed: serde_json::Value = serde_json::from_str(result.stdout.trim()).unwrap();
        assert_eq!(parsed, serde_json::json!([1, 2]));
    }

    #[test]
    fn no_filter_provided() {
        let result = run_jq(&[], "{}");
        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.contains("no filter"));
    }

    #[test]
    fn split_and_join() {
        let result = run_jq(&["-r", r#""a,b,c" | split(",") | join("-")"#], "null");
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "a-b-c");
    }

    #[test]
    fn reduce_filter() {
        let result = run_jq(&["reduce .[] as $x (0; . + $x)"], "[1, 2, 3, 4]");
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "10");
    }

    #[test]
    fn runtime_error_exit_code_5() {
        let result = run_jq(&[".foo"], r#""not an object""#);
        assert_eq!(result.exit_code, 5);
        assert!(!result.stderr.is_empty());
    }

    #[test]
    fn exit_status_no_output() {
        let result = run_jq(&["-e", "select(false)"], "1");
        assert_eq!(result.exit_code, 4);
    }

    #[test]
    fn double_dash_end_of_opts() {
        let result = run_jq(&["-r", "--", ".x"], r#"{"x": "hello"}"#);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "hello");
    }

    #[test]
    fn stdin_dash_as_file() {
        let fs = InMemoryFs::new();
        let result = run_jq_with_fs(&[".x", "-"], r#"{"x": 42}"#, &fs);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "42");
    }

    #[test]
    fn slurp_raw_input() {
        let result = run_jq(&["-Rs", "."], "line1\nline2\nline3");
        assert_eq!(result.exit_code, 0);
        // All lines become one array of strings, then slurped
        let parsed: serde_json::Value = serde_json::from_str(result.stdout.trim()).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 3);
    }

    #[test]
    fn argjson_invalid_json_error() {
        let result = run_jq(&["--argjson", "val", "not-json", "."], "null");
        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.contains("--argjson"));
    }

    #[test]
    fn multiple_vfs_files() {
        let fs = InMemoryFs::new();
        fs.write_file(&PathBuf::from("/a.json"), br#"{"v": 1}"#)
            .unwrap();
        fs.write_file(&PathBuf::from("/b.json"), br#"{"v": 2}"#)
            .unwrap();
        let result = run_jq_with_fs(&[".v", "/a.json", "/b.json"], "", &fs);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "1\n2\n");
    }

    #[test]
    fn has_key_filter() {
        let result = run_jq(&[r#"has("a")"#], r#"{"a": 1, "b": 2}"#);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "true");
    }

    #[test]
    fn to_entries_filter() {
        let result = run_jq(&["-c", "to_entries"], r#"{"a": 1}"#);
        assert_eq!(result.exit_code, 0);
        let parsed: serde_json::Value = serde_json::from_str(result.stdout.trim()).unwrap();
        assert_eq!(parsed, serde_json::json!([{"key": "a", "value": 1}]));
    }

    #[test]
    fn alternative_operator() {
        let result = run_jq(&[r#".missing // "default""#], r#"{"a": 1}"#);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), r#""default""#);
    }

    #[test]
    fn test_regex() {
        let result = run_jq(&[r#"test("^foo")"#], r#""foobar""#);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "true");
    }
}
