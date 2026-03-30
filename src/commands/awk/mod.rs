mod lexer;
mod parser;
mod runtime;

use super::{CommandContext, CommandMeta, CommandResult, VirtualCommand};
use lexer::Lexer;
use parser::Parser;
use runtime::AwkRuntime;
use std::path::PathBuf;

pub struct AwkCommand;

static AWK_META: CommandMeta = CommandMeta {
    name: "awk",
    synopsis: "awk [-F FS] [-v VAR=VALUE] [-f FILE] 'PROGRAM' [FILE ...]",
    description: "Pattern scanning and text processing language.",
    options: &[
        ("-F FS", "set the input field separator"),
        ("-v VAR=VALUE", "assign a value to a variable"),
        ("-f FILE", "read the awk program from FILE"),
    ],
    supports_help_flag: true,
};

impl VirtualCommand for AwkCommand {
    fn name(&self) -> &str {
        "awk"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&AWK_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        match run_awk(args, ctx) {
            Ok(result) => result,
            Err(e) => CommandResult {
                stdout: String::new(),
                stderr: format!("awk: {e}\n"),
                exit_code: 2,
            },
        }
    }
}

struct AwkOpts {
    field_separator: Option<String>,
    assignments: Vec<(String, String)>,
    program: String,
    prog_file: Option<String>,
    files: Vec<String>,
}

fn parse_args(args: &[String]) -> Result<AwkOpts, String> {
    let mut fs = None;
    let mut assignments = Vec::new();
    let mut program = None;
    let mut files = Vec::new();
    let mut prog_file = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "-F" {
            i += 1;
            if i >= args.len() {
                return Err("option -F requires an argument".to_string());
            }
            fs = Some(args[i].clone());
        } else if let Some(sep) = arg.strip_prefix("-F") {
            fs = Some(sep.to_string());
        } else if arg == "-v" {
            i += 1;
            if i >= args.len() {
                return Err("option -v requires an argument".to_string());
            }
            let assign = &args[i];
            if let Some((var, val)) = assign.split_once('=') {
                assignments.push((var.to_string(), val.to_string()));
            } else {
                return Err(format!("invalid -v assignment: {assign}"));
            }
        } else if let Some(rest) = arg.strip_prefix("-v") {
            if let Some((var, val)) = rest.split_once('=') {
                assignments.push((var.to_string(), val.to_string()));
            } else {
                return Err(format!("invalid -v assignment: {rest}"));
            }
        } else if arg == "-f" {
            i += 1;
            if i >= args.len() {
                return Err("option -f requires an argument".to_string());
            }
            prog_file = Some(args[i].clone());
        } else if arg == "--" {
            i += 1;
            break;
        } else if arg.starts_with('-') && program.is_none() && prog_file.is_none() {
            return Err(format!("unknown option: {arg}"));
        } else if program.is_none() && prog_file.is_none() {
            program = Some(arg.clone());
        } else {
            files.push(arg.clone());
        }
        i += 1;
    }

    // Remaining args are files
    while i < args.len() {
        files.push(args[i].clone());
        i += 1;
    }

    if let Some(pf) = prog_file {
        Ok(AwkOpts {
            field_separator: fs,
            assignments,
            program: String::new(),
            prog_file: Some(pf),
            files,
        })
    } else if let Some(prog) = program {
        Ok(AwkOpts {
            field_separator: fs,
            assignments,
            program: prog,
            prog_file: None,
            files,
        })
    } else {
        Err("no program text".to_string())
    }
}

fn resolve_path(path_str: &str, cwd: &str) -> PathBuf {
    if path_str.starts_with('/') {
        PathBuf::from(path_str)
    } else {
        PathBuf::from(cwd).join(path_str)
    }
}

fn run_awk(args: &[String], ctx: &CommandContext) -> Result<CommandResult, String> {
    let mut opts = parse_args(args)?;

    // Handle -f progfile
    if let Some(ref pf) = opts.prog_file {
        let path = resolve_path(pf, ctx.cwd);
        match ctx.fs.read_file(&path) {
            Ok(bytes) => {
                opts.program = String::from_utf8_lossy(&bytes).to_string();
            }
            Err(e) => return Err(format!("can't open source file '{pf}': {e}")),
        }
    }
    if opts.program.is_empty() {
        return Err("no program text".to_string());
    }

    // Tokenize
    let tokens = Lexer::new(&opts.program)
        .tokenize()
        .map_err(|e| format!("syntax error: {e}"))?;

    // Parse
    let program = Parser::new(tokens)
        .parse()
        .map_err(|e| format!("syntax error: {e}"))?;

    // Set up runtime
    let mut runtime = AwkRuntime::new();
    runtime.apply_limits(ctx.limits);

    // Apply -F
    if let Some(ref fs) = opts.field_separator {
        runtime.set_var("FS", fs);
    }

    // Apply -v assignments
    for (var, val) in &opts.assignments {
        runtime.set_var(var, val);
    }

    // Build ARGC/ARGV
    let mut argv_args = vec!["awk".to_string()];
    argv_args.extend(opts.files.clone());
    runtime.set_argc_argv(&argv_args);

    // Collect inputs
    let inputs = collect_inputs(&opts.files, ctx)?;

    // Execute
    let (exit_code, stdout, stderr) = runtime.execute(&program, &inputs);

    Ok(CommandResult {
        stdout,
        stderr,
        exit_code,
    })
}

fn collect_inputs(files: &[String], ctx: &CommandContext) -> Result<Vec<(String, String)>, String> {
    if files.is_empty() {
        // Read from stdin
        if ctx.stdin.is_empty() {
            return Ok(vec![]);
        }
        return Ok(vec![("".to_string(), ctx.stdin.to_string())]);
    }

    let mut inputs = Vec::new();
    for file in files {
        if file == "-" {
            inputs.push(("(standard input)".to_string(), ctx.stdin.to_string()));
        } else {
            let path = resolve_path(file, ctx.cwd);
            match ctx.fs.read_file(&path) {
                Ok(bytes) => {
                    inputs.push((file.clone(), String::from_utf8_lossy(&bytes).to_string()));
                }
                Err(e) => {
                    return Err(format!("can't open file '{file}': {e}"));
                }
            }
        }
    }
    Ok(inputs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpreter::ExecutionLimits;
    use crate::network::NetworkPolicy;
    use crate::vfs::{InMemoryFs, VirtualFs};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn run(program: &str, stdin: &str) -> CommandResult {
        let fs = Arc::new(InMemoryFs::new());
        let env = HashMap::new();
        let limits = ExecutionLimits::default();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin,
            limits: &limits,
            network_policy: &NetworkPolicy::default(),
            exec: None,
        };
        let args = vec![program.to_string()];
        AwkCommand.execute(&args, &ctx)
    }

    fn run_with_args(args: &[&str], stdin: &str) -> CommandResult {
        let fs = Arc::new(InMemoryFs::new());
        let env = HashMap::new();
        let limits = ExecutionLimits::default();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin,
            limits: &limits,
            network_policy: &NetworkPolicy::default(),
            exec: None,
        };
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        AwkCommand.execute(&args, &ctx)
    }

    fn run_with_files(program: &str, files: &[(&str, &str)]) -> CommandResult {
        let fs = Arc::new(InMemoryFs::new());
        for (name, content) in files {
            fs.write_file(&PathBuf::from(format!("/{name}")), content.as_bytes())
                .unwrap();
        }
        let env = HashMap::new();
        let limits = ExecutionLimits::default();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "",
            limits: &limits,
            network_policy: &NetworkPolicy::default(),
            exec: None,
        };
        let mut args: Vec<String> = vec![program.to_string()];
        for (name, _) in files {
            args.push(name.to_string());
        }
        AwkCommand.execute(&args, &ctx)
    }

    #[test]
    fn integration_print_first_field() {
        let r = run("{print $1}", "hello world\nfoo bar\n");
        assert_eq!(r.stdout, "hello\nfoo\n");
        assert_eq!(r.exit_code, 0);
    }

    #[test]
    fn integration_field_separator() {
        let r = run_with_args(&["-F:", "{print $1}"], "root:x:0:0\n");
        assert_eq!(r.stdout, "root\n");
    }

    #[test]
    fn integration_field_assignment() {
        let r = run("{$2 = \"X\"; print $0}", "a b c\n");
        assert_eq!(r.stdout, "a X c\n");
    }

    #[test]
    fn integration_regex_filter() {
        let r = run("/error/ {print}", "info: ok\nerror: fail\ninfo: done\n");
        assert_eq!(r.stdout, "error: fail\n");
    }

    #[test]
    fn integration_begin_end_sum() {
        let r = run("BEGIN{sum=0} {sum+=$1} END{print sum}", "10\n20\n30\n");
        assert_eq!(r.stdout, "60\n");
    }

    #[test]
    fn integration_variable() {
        let r = run_with_args(&["-v", "threshold=10", "$1 > threshold"], "5\n15\n8\n20\n");
        assert_eq!(r.stdout, "15\n20\n");
    }

    #[test]
    fn integration_uninitialized() {
        let r = run("{print x+0, x}", "line\n");
        assert_eq!(r.stdout, "0 \n");
    }

    #[test]
    fn integration_arithmetic() {
        let r = run("{print $1, $1*2}", "5\n10\n");
        assert_eq!(r.stdout, "5 10\n10 20\n");
    }

    #[test]
    fn integration_if_else() {
        let r = run(
            "{if ($1 > 10) print \"big\"; else print \"small\"}",
            "5\n15\n",
        );
        assert_eq!(r.stdout, "small\nbig\n");
    }

    #[test]
    fn integration_printf() {
        let r = run("{printf \"%-10s %5d\\n\", $1, $2}", "hello 42\n");
        assert_eq!(r.stdout, "hello         42\n");
    }

    #[test]
    fn integration_array_word_count() {
        let r = run(
            "{count[$1]++} END{for(k in count) print k, count[k]}",
            "a\nb\na\nc\nb\na\n",
        );
        assert!(r.stdout.contains("a 3"));
        assert!(r.stdout.contains("b 2"));
        assert!(r.stdout.contains("c 1"));
    }

    #[test]
    fn integration_string_functions() {
        let r = run("{print toupper($0)}", "hello\n");
        assert_eq!(r.stdout, "HELLO\n");
    }

    #[test]
    fn integration_multi_file() {
        let r = run_with_files(
            "{print FILENAME, FNR, NR}",
            &[("file1", "a\nb\n"), ("file2", "c\n")],
        );
        assert_eq!(r.stdout, "file1 1 1\nfile1 2 2\nfile2 1 3\n");
    }

    #[test]
    fn integration_range_pattern() {
        let r = run(
            "/start/,/end/ {print}",
            "before\nstart here\nmiddle\nend here\nafter\n",
        );
        assert_eq!(r.stdout, "start here\nmiddle\nend here\n");
    }

    #[test]
    fn integration_no_action_implicit_print() {
        let r = run("/hello/", "hello world\ngoodbye\nhello again\n");
        assert_eq!(r.stdout, "hello world\nhello again\n");
    }

    #[test]
    fn integration_empty_input() {
        let r = run("{print}", "");
        assert_eq!(r.stdout, "");
    }

    #[test]
    fn integration_empty_fs() {
        let r = run_with_args(&["-F", "", "{print $1, $2, $3}"], "abc\n");
        assert_eq!(r.stdout, "a b c\n");
    }

    #[test]
    fn integration_nr_nf() {
        let r = run("{print NR, NF}", "a b c\nx y\n");
        assert_eq!(r.stdout, "1 3\n2 2\n");
    }

    #[test]
    fn integration_progfile() {
        let fs = Arc::new(InMemoryFs::new());
        fs.write_file(&PathBuf::from("/prog.awk"), b"{print $1}")
            .unwrap();
        let env = HashMap::new();
        let limits = ExecutionLimits::default();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "hello world\n",
            limits: &limits,
            network_policy: &NetworkPolicy::default(),
            exec: None,
        };
        let args = vec!["-f".to_string(), "prog.awk".to_string()];
        let r = AwkCommand.execute(&args, &ctx);
        assert_eq!(r.stdout, "hello\n");
    }

    #[test]
    fn integration_match_function() {
        let r = run(
            "{if (match($0, /[0-9]+/)) print RSTART, RLENGTH}",
            "abc123def\n",
        );
        assert_eq!(r.stdout, "4 3\n");
    }

    #[test]
    fn integration_split_function() {
        let r = run(
            "{n=split($0, a, \":\"); for(i=1;i<=n;i++) print a[i]}",
            "a:b:c\n",
        );
        assert_eq!(r.stdout, "a\nb\nc\n");
    }

    #[test]
    fn integration_sub_gsub() {
        let r = run("{sub(/world/, \"earth\"); print}", "hello world\n");
        assert_eq!(r.stdout, "hello earth\n");

        let r = run("{gsub(/o/, \"0\"); print}", "foobar\n");
        assert_eq!(r.stdout, "f00bar\n");
    }

    #[test]
    fn integration_in_array() {
        let r = run("{a[$1]=1} END{print (\"x\" in a), (\"z\" in a)}", "x\ny\n");
        assert_eq!(r.stdout, "1 0\n");
    }

    #[test]
    fn integration_assignment_operators() {
        let r = run("BEGIN{x=10; x+=5; x-=3; print x}", "");
        assert_eq!(r.stdout, "12\n");
    }

    #[test]
    fn integration_do_while() {
        let r = run(
            "BEGIN{i=1; do { printf \"%d \", i; i++ } while(i<=3); print \"\"}",
            "",
        );
        assert_eq!(r.stdout, "1 2 3 \n");
    }

    #[test]
    fn integration_ternary() {
        let r = run("{print ($1 > 0) ? \"pos\" : \"neg\"}", "5\n-3\n");
        assert_eq!(r.stdout, "pos\nneg\n");
    }

    #[test]
    fn integration_pipe_stdin() {
        // Simulates `echo "hello world" | awk '{print $2}'`
        let r = run("{print $2}", "hello world\n");
        assert_eq!(r.stdout, "world\n");
    }

    #[test]
    fn integration_substr() {
        let r = run("{print substr($0, 7, 5)}", "hello world\n");
        assert_eq!(r.stdout, "world\n");
    }

    #[test]
    fn integration_index_func() {
        let r = run("{print index($0, \"world\")}", "hello world\n");
        assert_eq!(r.stdout, "7\n");
    }

    #[test]
    fn integration_sprintf() {
        let r = run("{print sprintf(\"%05d\", $1)}", "42\n");
        assert_eq!(r.stdout, "00042\n");
    }

    #[test]
    fn integration_power() {
        let r = run("BEGIN{print 2^10}", "");
        assert_eq!(r.stdout, "1024\n");
    }

    #[test]
    fn integration_int() {
        let r = run("BEGIN{print int(3.9)}", "");
        assert_eq!(r.stdout, "3\n");
    }

    #[test]
    fn integration_error_on_no_program() {
        let r = run_with_args(&[], "");
        assert_ne!(r.exit_code, 0);
    }

    #[test]
    fn integration_expression_pattern() {
        let r = run("NR > 1 {print}", "skip\nkeep1\nkeep2\n");
        assert_eq!(r.stdout, "keep1\nkeep2\n");
    }

    #[test]
    fn integration_regex_match_not_match() {
        let r = run("{if ($0 ~ /^[0-9]/) print}", "123\nabc\n456\n");
        assert_eq!(r.stdout, "123\n456\n");

        let r = run("{if ($0 !~ /^[0-9]/) print}", "123\nabc\n456\n");
        assert_eq!(r.stdout, "abc\n");
    }

    #[test]
    fn integration_delete_array() {
        let r = run("{a[$1]=1} END{delete a; print length(a)}", "x\ny\n");
        assert_eq!(r.stdout, "0\n");
    }

    #[test]
    fn integration_single_field() {
        let r = run("{print $1, NF}", "hello\n");
        assert_eq!(r.stdout, "hello 1\n");
    }

    #[test]
    fn integration_very_long_line() {
        let long = "a ".repeat(1000).trim().to_string();
        let input = format!("{long}\n");
        let r = run("{print NF}", &input);
        assert_eq!(r.stdout, "1000\n");
    }

    #[test]
    fn integration_begin_only() {
        let r = run("BEGIN{print \"hello\"}", "");
        assert_eq!(r.stdout, "hello\n");
    }

    #[test]
    fn integration_end_only() {
        let r = run("END{print \"done\"}", "some input\n");
        assert_eq!(r.stdout, "done\n");
    }

    #[test]
    fn integration_break_continue() {
        let r = run(
            "BEGIN{for(i=1;i<=10;i++){if(i==4) break; printf \"%d \",i}; print \"\"}",
            "",
        );
        assert_eq!(r.stdout, "1 2 3 \n");

        let r = run(
            "BEGIN{for(i=1;i<=5;i++){if(i==3) continue; printf \"%d \",i}; print \"\"}",
            "",
        );
        assert_eq!(r.stdout, "1 2 4 5 \n");
    }

    #[test]
    fn integration_next() {
        let r = run(
            "{if ($1 == \"skip\") next; print}",
            "keep\nskip\nalso keep\n",
        );
        assert_eq!(r.stdout, "keep\nalso keep\n");
    }

    #[test]
    fn integration_exit_code() {
        let r = run("{ if (NR==2) exit 42; print }", "a\nb\nc\n");
        assert_eq!(r.stdout, "a\n");
        assert_eq!(r.exit_code, 42);
    }

    #[test]
    fn integration_logical_ops() {
        let r = run("{print ($1 > 0 && $1 < 10)}", "5\n15\n");
        assert_eq!(r.stdout, "1\n0\n");

        let r = run("{print ($1 > 10 || $1 < 0)}", "5\n-3\n15\n");
        assert_eq!(r.stdout, "0\n1\n1\n");
    }

    #[test]
    fn integration_modulo() {
        let r = run("{print $1 % 3}", "10\n7\n");
        assert_eq!(r.stdout, "1\n1\n");
    }

    #[test]
    fn integration_implicit_concat() {
        let r = run("BEGIN{x = \"hello\" \" \" \"world\"; print x}", "");
        assert_eq!(r.stdout, "hello world\n");
    }

    #[test]
    fn integration_ofs() {
        let r = run_with_args(&["-v", "OFS=-", "{print $1, $2}"], "a b\n");
        assert_eq!(r.stdout, "a-b\n");
    }

    #[test]
    fn integration_length_func() {
        let r = run("{print length($0)}", "hello\n");
        assert_eq!(r.stdout, "5\n");
    }

    #[test]
    fn integration_pre_post_increment() {
        let r = run("BEGIN{x=5; print ++x; print x++; print x}", "");
        assert_eq!(r.stdout, "6\n6\n7\n");
    }
}
